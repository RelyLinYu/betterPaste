//! SQLite persistence (FTS5 search) plus the image/thumbnail files on disk.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use image::{ImageFormat, RgbaImage};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::{paths, Error, Result};

/// 单条文本的兜底上限（字节）。
///
/// 真正的体积拦截按配置的 `单条体积上限`（默认 10 MB）在 `clipboard.rs` 里做；
/// 这里只是最后一道保险。之前这里的值是 64 KiB 且按**字符**截断，于是 100 KB 的文本
/// 会被静默砍掉一半（实测：尾部内容整段丢失，粘贴出来是残文）。
pub const CONTENT_LIMIT: usize = 10 * 1024 * 1024;
pub const THUMB_MAX: u32 = 320;

/// 按字节上限截断，且永远切在 char 边界上。
fn clip_to(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let end = text
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&e| e <= limit)
        .last()
        .unwrap_or(0);
    &text[..end]
}

/// 截断文本（正常情况下原样返回）。
pub fn clipped(text: &str) -> String {
    if text.len() <= CONTENT_LIMIT {
        return text.to_string();
    }
    log::warn!("文本 {} 字节超过兜底上限 {}，已截断", text.len(), CONTENT_LIMIT);
    clip_to(text, CONTENT_LIMIT).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Text,
    Image,
}

impl ClipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClipKind::Text => "text",
            ClipKind::Image => "image",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "image" {
            ClipKind::Image
        } else {
            ClipKind::Text
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEntry {
    pub id: String,
    pub kind: ClipKind,
    /// Text payload, or `None` for image entries.
    pub content: Option<String>,
    /// Absolute path of the full-size PNG (images only).
    pub image_path: Option<String>,
    /// Absolute path of the thumbnail PNG (images only).
    pub thumb_path: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: u64,
    /// 0-9 when pinned to a slot, otherwise `None`.
    pub pinned: Option<u8>,
    pub source_app: Option<String>,
    pub created_at: i64,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> Result<Self> {
        let conn = Connection::open(paths::db_path()?)?;
        Self::init(conn)
    }

    /// In-memory store, used by unit tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // 常驻 App 与 `--probe/--ctl` 是两个进程、两个连接：没有 busy handler 时
        // 写冲突会立刻返回 SQLITE_BUSY（"database is locked"），内容就悄悄丢了。
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS entries (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                type        TEXT    NOT NULL,
                content     TEXT,
                image_path  TEXT,
                thumb_path  TEXT,
                width       INTEGER,
                height      INTEGER,
                bytes       INTEGER NOT NULL DEFAULT 0,
                pinned      INTEGER,
                source_app  TEXT,
                created_at  INTEGER NOT NULL,
                uid         TEXT    NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_entries_pinned  ON entries(pinned);

            CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
                content,
                tokenize = 'unicode61 remove_diacritics 2'
            );

            -- FTS5 与 entries 的同步**真的**靠触发器。
            -- 之前这里只有一句"triggers keep FTS5 in sync"的注释，触发器一条都没有，
            -- 删除路径也不清理 FTS，于是删掉/清空的内容永远留在索引里（磁盘只增不减，
            -- 而且删掉的文本仍能整段读出来）。
            CREATE TRIGGER IF NOT EXISTS entries_fts_ai AFTER INSERT ON entries
            WHEN new.type = 'text' AND new.content IS NOT NULL
            BEGIN
                INSERT INTO entries_fts(rowid, content) VALUES (new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS entries_fts_ad AFTER DELETE ON entries
            BEGIN
                DELETE FROM entries_fts WHERE rowid = old.id;
            END;

            CREATE TRIGGER IF NOT EXISTS entries_fts_au AFTER UPDATE OF content ON entries
            BEGIN
                DELETE FROM entries_fts WHERE rowid = old.id;
                INSERT INTO entries_fts(rowid, content)
                    SELECT new.id, new.content WHERE new.content IS NOT NULL;
            END;
            "#,
        )?;

        // 触发器生效之前留下的幽灵索引行，清一次。
        match conn.execute("DELETE FROM entries_fts WHERE rowid NOT IN (SELECT id FROM entries)", []) {
            Ok(n) if n > 0 => log::info!("清理了 {n} 行 FTS 幽灵索引"),
            Ok(_) => {}
            Err(e) => log::warn!("清理 FTS 幽灵索引失败: {e}"),
        }
        Ok(Self { conn })
    }

    // ---------------------------------------------------------------- writes

    /// 写入一条文本。返回是否真的入库（重复 / 空内容会被忽略）。
    pub fn insert_text(&mut self, content: &str, source_app: Option<&str>) -> Result<Option<ClipEntry>> {
        let text: String = clipped(content);
        if text.trim().is_empty() {
            return Ok(None);
        }
        let now = now_ms();
        let uid = Uuid::new_v4().to_string();
        let bytes = text.len() as u64;
        let tx = self.conn.transaction()?;
        // entries_fts 由 AFTER INSERT 触发器同步，这里不再手工插入
        tx.execute(
            "INSERT INTO entries (type, content, bytes, source_app, created_at, uid)
             VALUES ('text', ?1, ?2, ?3, ?4, ?5)",
            params![text, bytes, source_app, now, uid],
        )?;
        tx.commit()?;
        self.get_by_uid(&uid)
    }

    /// 写入一条图片（PNG 编码后的字节）。
    ///
    /// 文件先落盘才能拿到路径，所以任何一步失败都必须把已经写下的文件删掉，
    /// 否则就是永远没人引用的孤儿 PNG（数据库里没有对应行，清理任务也扫不到）。
    pub fn insert_image(
        &mut self,
        png_bytes: Vec<u8>,
        width: u32,
        height: u32,
        source_app: Option<&str>,
    ) -> Result<ClipEntry> {
        let id = Uuid::new_v4().to_string();
        let images = paths::images_dir()?;
        let thumbs = paths::thumbs_dir()?;

        let image_file = images.join(format!("{id}.png"));
        let thumb_file = thumbs.join(format!("{id}.png"));

        // 原图必须写成功；缩略图只用于展示，失败就降级为"没有缩略图"
        if let Err(e) = std::fs::write(&image_file, &png_bytes) {
            return Err(e.into());
        }
        let thumb_ok = match make_thumbnail(&png_bytes) {
            Ok((thumb_png, _, _)) => match std::fs::write(&thumb_file, &thumb_png) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("缩略图写盘失败（降级为无缩略图）: {e}");
                    false
                }
            },
            Err(e) => {
                log::warn!("缩略图生成失败（降级为无缩略图）: {e}");
                false
            }
        };
        let thumb: Option<PathBuf> = thumb_ok.then(|| thumb_file.clone());

        let cleanup = |image_file: &PathBuf, thumb: &Option<PathBuf>| {
            let _ = std::fs::remove_file(image_file);
            if let Some(t) = thumb {
                let _ = std::fs::remove_file(t);
            }
        };

        let now = now_ms();
        let bytes = png_bytes.len() as u64;
        if let Err(e) = self.conn.execute(
            "INSERT INTO entries (type, image_path, thumb_path, width, height, bytes, source_app, created_at, uid)
             VALUES ('image', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                rel(&image_file),
                thumb.as_ref().map(rel),
                width,
                height,
                bytes,
                source_app,
                now,
                id
            ],
        ) {
            cleanup(&image_file, &thumb);
            return Err(e.into());
        }

        match self.get_by_uid(&id)? {
            Some(entry) => Ok(entry),
            None => {
                cleanup(&image_file, &thumb);
                Err(Error::Other("图片写入后读回失败".into()))
            }
        }
    }

    /// 把某条记录钉到槽位（`None` = 取消置顶）。
    ///
    /// 两条 UPDATE 必须在一个事务里：旧写法先清空槽位、再写新记录，两次隐式提交，
    /// 第二条失败（uid 不存在 / SQLITE_BUSY）时槽位原主人已经被清掉了，调用方却拿到 `Ok`。
    pub fn set_pinned(&mut self, uid: &str, slot: Option<u8>) -> Result<()> {
        let tx = self.conn.transaction()?;
        if let Some(s) = slot {
            // 一个槽位只能有一条记录
            tx.execute("UPDATE entries SET pinned = NULL WHERE pinned = ?1", params![s])?;
        }
        let affected = tx.execute("UPDATE entries SET pinned = ?1 WHERE uid = ?2", params![slot, uid])?;
        if affected == 0 {
            // 事务未提交 → 自动回滚，槽位不会被清空
            return Err(Error::Other("记录不存在（可能已被清理）".into()));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_pinned(&self, slot: u8) -> Result<Option<ClipEntry>> {
        let row = self
            .conn
            .query_row(
                &format!("{SELECT_COLS} WHERE pinned = ?1 LIMIT 1"),
                params![slot],
                map_row,
            )
            .optional()?;
        Ok(row)
    }

    pub fn delete(&mut self, uid: &str) -> Result<()> {
        let row = self
            .conn
            .query_row(
                "SELECT image_path, thumb_path FROM entries WHERE uid = ?1",
                params![uid],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        // 先落库、后删文件：反过来的话 commit 失败就剩"有记录无文件"的条目
        self.conn.execute("DELETE FROM entries WHERE uid = ?1", params![uid])?;
        if let Some((img, thumb)) = row {
            remove_files(paths::data_dir()?, [img, thumb]);
        }
        Ok(())
    }

    /// 清空历史（`keep_pinned` 时保留置顶条目）。单事务批量删除。
    /// 返回删除条数。
    pub fn clear(&mut self, keep_pinned: bool) -> Result<u32> {
        // 旧实现是 list(5000) 后逐条 delete：5000 条之后清不掉，而且每条都是
        // 一个独立事务 + 一次 fsync，期间一直占着 store 互斥锁。
        let where_sql = if keep_pinned { "pinned IS NULL" } else { "1=1" };
        self.delete_where(where_sql, [])
    }

    /// 把 WAL 落回主库（退出前调用；否则 -wal 会一直膨胀）。
    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    // ---------------------------------------------------------------- reads

    pub fn get(&self, uid: &str) -> Result<Option<ClipEntry>> {
        self.get_by_uid(uid)
    }

    fn get_by_uid(&self, uid: &str) -> Result<Option<ClipEntry>> {
        let row = self
            .conn
            .query_row(&format!("{SELECT_COLS} WHERE uid = ?1"), params![uid], map_row)
            .optional()?;
        Ok(row)
    }

    /// 最近历史（倒序）。
    pub fn list(&self, limit: u32) -> Result<Vec<ClipEntry>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_COLS} ORDER BY created_at DESC, id DESC LIMIT ?1"))?;
        let rows = stmt.query_map(params![limit], map_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn recent_content(&self) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row(
                "SELECT content FROM entries WHERE type = 'text' ORDER BY created_at DESC, id DESC LIMIT 1",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(v.flatten())
    }

    /// 搜索：FTS5 前缀命中 **并集** LIKE 子串命中。
    ///
    /// 旧实现是"FTS 有命中就直接返回"，于是 `cargo build` 与 `mycargo wrapper` 都在库里时
    /// 搜 `cargo` 只回一条；同一个词能不能搜到取决于库里有没有别的 token 恰好前缀命中。
    /// 另外 FTS 的失败不再向上抛（含 NUL 的查询会在这里被清掉），一个坏查询不该让整个
    /// 搜索界面报错，也不该让正常文本搜不到。
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<ClipEntry>> {
        // 控制字符（含 \0）会让 FTS5 抛 unterminated string，先清掉
        let cleaned: String = query.chars().filter(|c| !c.is_control()).collect();
        let q = cleaned.trim();
        if q.is_empty() {
            return self.list(limit);
        }

        let mut hits: Vec<ClipEntry> = Vec::new();

        if let Some(fts) = fts_query(q) {
            let sql = format!(
                "{SELECT_COLS} WHERE id IN (SELECT rowid FROM entries_fts WHERE entries_fts MATCH ?1) \
                 ORDER BY created_at DESC, id DESC LIMIT ?2"
            );
            match self.conn.prepare(&sql) {
                Ok(mut stmt) => match stmt.query_map(params![fts, limit], map_row) {
                    Ok(rows) => {
                        for row in rows {
                            match row {
                                Ok(e) => hits.push(e),
                                Err(e) => log::warn!("FTS 行读取失败（跳过）: {e}"),
                            }
                        }
                    }
                    Err(e) => log::warn!("FTS 查询失败，改用 LIKE: {e}"),
                },
                Err(e) => log::warn!("FTS 语句准备失败，改用 LIKE: {e}"),
            }
        }

        // 子串语义兜底（大小写不敏感，中文按子串匹配）
        if hits.len() < limit as usize {
            let like = format!("%{}%", escape_like(q));
            let sql = format!(
                "{SELECT_COLS} WHERE content LIKE ?1 ESCAPE '\\' ORDER BY created_at DESC, id DESC LIMIT ?2"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![like, limit], map_row)?;
            for row in rows {
                let e = row?;
                if !hits.iter().any(|h| h.id == e.id) {
                    hits.push(e);
                }
            }
        }

        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
        hits.truncate(limit as usize);
        Ok(hits)
    }

    pub fn count(&self) -> Result<u32> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?)
    }

    // -------------------------------------------------------------- cleanup

    /// 按条数 / 天数上限清理，删除的记录连同图片文件一起清掉。
    /// 返回删除条数。
    ///
    /// 注意：**置顶条目不受条数上限影响**（设计文档 §1「槽内容可配」的隐含要求），
    /// 也不受天数上限影响——用户手动钉住的东西不该被后台任务悄悄删掉。
    pub fn cleanup(&mut self, cfg: &AppConfig) -> Result<u32> {
        let mut removed = 0u32;

        if let Some(days) = cfg.age_limit_days() {
            let cutoff = now_ms() - i64::from(days) * 86_400_000;
            removed += self.delete_where("created_at < ?1 AND pinned IS NULL", params![cutoff])?;
        }

        if let Some(limit) = cfg.entry_limit() {
            // 保留：所有置顶条目 + 最新的 (limit) 条未置顶条目。
            // 外层 `pinned IS NULL` 已经保证置顶条目不在删除范围内（子查询只用来
            // 挑出"最新的 limit 条未置顶记录"）。
            let sql = "pinned IS NULL AND id NOT IN (
                 SELECT id FROM entries WHERE pinned IS NULL
                 ORDER BY created_at DESC, id DESC LIMIT ?1
               )";
            removed += self.delete_where(sql, params![limit])?;
        }

        Ok(removed)
    }

    fn delete_where(&mut self, where_sql: &str, args: impl rusqlite::Params) -> Result<u32> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, image_path, thumb_path FROM entries WHERE {where_sql}"
        ))?;
        let victims: Vec<(i64, Option<String>, Option<String>)> = stmt
            .query_map(args, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        if victims.is_empty() {
            return Ok(0);
        }
        let removed = victims.len() as u32;
        let root = paths::data_dir()?;
        let tx = self.conn.transaction()?;
        for (id, _img, _thumb) in &victims {
            tx.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        // 事务提交成功之后再删文件（顺序反了的话 commit 失败会留下"有记录无文件"）
        for (_id, img, thumb) in victims {
            remove_files(root.clone(), [img, thumb]);
        }
        Ok(removed)
    }
}

// -------------------------------------------------------------------- helpers

const SELECT_COLS: &str = "SELECT id, uid, type, content, image_path, thumb_path, width, height, \
                           bytes, pinned, source_app, created_at FROM entries";

#[allow(clippy::type_complexity)]
fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClipEntry> {
    let type_: String = r.get(2)?;
    let image_path: Option<String> = r.get(4)?;
    let thumb_path: Option<String> = r.get(5)?;
    let root = paths::data_dir().unwrap_or_default();
    Ok(ClipEntry {
        id: r.get(1)?,
        kind: ClipKind::parse(&type_),
        content: r.get(3)?,
        image_path: image_path.map(|p| root.join(p).to_string_lossy().to_string()),
        thumb_path: thumb_path.map(|p| root.join(p).to_string_lossy().to_string()),
        width: r.get(6)?,
        height: r.get(7)?,
        bytes: r.get::<_, i64>(8)? as u64,
        pinned: r.get::<_, Option<u8>>(9)?,
        source_app: r.get(10)?,
        created_at: r.get(11)?,
    })
}

/// 存进数据库的始终是相对 `%APPDATA%\clipstack\` 的路径。
fn rel(p: &PathBuf) -> String {
    let root = paths::data_dir().unwrap_or_default();
    p.strip_prefix(&root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

/// 删除记录对应的图片 / 缩略图文件。
///
/// 旧写法是 `let _ = std::fs::remove_file(...)`：文件被查看器/杀软占用时失败被完全吞掉，
/// 记录照样删掉，于是磁盘上留下永远查不到的孤儿 PNG。现在至少留一条日志。
fn remove_files(root: PathBuf, paths: [Option<String>; 2]) {
    for p in paths.into_iter().flatten() {
        let full = root.join(p);
        if let Err(e) = std::fs::remove_file(&full) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("删除图片文件失败 {}: {e}", full.display());
            }
        }
    }
}

/// 生成缩略图 PNG。返回 `(png, 宽, 高)`。
pub fn make_thumbnail(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(png_bytes)?;
    let (w, h) = (img.width(), img.height());
    let thumb = if w > THUMB_MAX || h > THUMB_MAX {
        img.resize(THUMB_MAX, THUMB_MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (tw, th) = (thumb.width(), thumb.height());
    let mut out = Vec::new();
    thumb.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)?;
    Ok((out, tw, th))
}

/// 编码 RGBA 像素为 PNG（供剪切板图片落盘使用）。
pub fn encode_png(rgba: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)?;
    Ok(out)
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 把用户输入转成安全的 FTS5 查询串：按空白切成前缀词并加引号，避免语法错误。
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[cfg(test)]
    impl Store {
        fn fts_rows(&self) -> u32 {
            self.conn
                .query_row("SELECT COUNT(*) FROM entries_fts", [], |r| r.get(0))
                .unwrap()
        }
    }

    #[test]
    fn insert_and_list_newest_first() {
        let mut s = mem();
        s.insert_text("first", None).unwrap();
        s.insert_text("second", None).unwrap();
        let list = s.list(10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].content.as_deref(), Some("second"));
    }

    #[test]
    fn blank_text_is_ignored() {
        let mut s = mem();
        assert!(s.insert_text("   \n\t ", None).unwrap().is_none());
        assert_eq!(s.count().unwrap(), 0);
    }

    #[test]
    fn search_finds_substring_via_like_fallback() {
        let mut s = mem();
        s.insert_text("cargo build --release", None).unwrap();
        s.insert_text("npm run dev", None).unwrap();
        let hits = s.search("release", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.as_deref().unwrap().contains("release"));
    }

    #[test]
    fn search_handles_chinese() {
        let mut s = mem();
        s.insert_text("今天天气不错，适合写代码", None).unwrap();
        s.insert_text("hello world", None).unwrap();
        let hits = s.search("天气", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn fts_syntax_characters_do_not_error() {
        let mut s = mem();
        s.insert_text("a-b c*d (e)", None).unwrap();
        for q in ["a-b", "c*d", "(", "\"quote", "AND OR"] {
            let _ = s.search(q, 10).unwrap();
        }
    }

    #[test]
    fn pin_is_exclusive_per_slot() {
        let mut s = mem();
        let a = s.insert_text("alpha", None).unwrap().unwrap();
        let b = s.insert_text("beta", None).unwrap().unwrap();
        s.set_pinned(&a.id, Some(1)).unwrap();
        s.set_pinned(&b.id, Some(1)).unwrap();
        let slot = s.get_pinned(1).unwrap().unwrap();
        assert_eq!(slot.id, b.id);
        assert!(s.get(&a.id).unwrap().unwrap().pinned.is_none());
    }

    #[test]
    fn cleanup_respects_entry_limit_and_keeps_pinned() {
        let mut s = mem();
        let mut cfg = AppConfig::default();
        cfg.max_entries = 3;
        cfg.max_age_days = 0;
        for i in 0..6 {
            s.insert_text(&format!("entry {i}"), None).unwrap();
        }
        // 把最旧的一条钉到槽 2——它必须活过条数上限
        let pinned = s.list(10).unwrap().last().unwrap().clone();
        s.set_pinned(&pinned.id, Some(2)).unwrap();
        let removed = s.cleanup(&cfg).unwrap();

        let left = s.list(10).unwrap();
        assert!(removed > 0, "cleanup should have removed something");
        assert!(
            left.iter().any(|e| e.id == pinned.id),
            "pinned entry must survive the entry-count limit (left = {:?})",
            left.iter().map(|e| e.content.clone()).collect::<Vec<_>>()
        );
        // 其余保留的是最新的几条未置顶记录
        assert!(left.len() <= 4, "left = {}", left.len());
        assert!(left.iter().any(|e| e.content.as_deref() == Some("entry 5")));
    }

    #[test]
    fn cleanup_age_limit_also_spares_pinned_entries() {
        let mut s = mem();
        let mut cfg = AppConfig::default();
        cfg.max_entries = 0; // 不限制条数
        cfg.max_age_days = 1;
        let old = s.insert_text("ancient", None).unwrap().unwrap();
        // 手工把时间戳推回两天前
        s.conn
            .execute(
                "UPDATE entries SET created_at = ?1 WHERE uid = ?2",
                params![now_ms() - 2 * 86_400_000, old.id],
            )
            .unwrap();
        s.set_pinned(&old.id, Some(1)).unwrap();

        let removed = s.cleanup(&cfg).unwrap();
        assert_eq!(removed, 0, "置顶条目不该被天数上限清掉");
        assert!(s.get(&old.id).unwrap().is_some());
    }

    #[test]
    fn delete_removes_row() {
        let mut s = mem();
        let a = s.insert_text("gone", None).unwrap().unwrap();
        s.delete(&a.id).unwrap();
        assert!(s.get(&a.id).unwrap().is_none());
    }

    // ---- 回归：这些用例锁住的是 2026-10-09 bug 测试里实测到的问题 ----

    #[test]
    fn deleting_a_row_also_drops_its_fts_index_entry() {
        let mut s = mem();
        let a = s.insert_text("unique-needle-aaa", None).unwrap().unwrap();
        assert_eq!(s.fts_rows(), 1);
        s.delete(&a.id).unwrap();
        assert_eq!(s.fts_rows(), 0, "删除记录必须同时清掉 FTS 索引（旧实现留下幽灵行，磁盘只增不减）");
    }

    #[test]
    fn cleanup_keeps_the_fts_index_in_sync() {
        let mut s = mem();
        let mut cfg = AppConfig::default();
        cfg.max_entries = 2;
        cfg.max_age_days = 0;
        for i in 0..6 {
            s.insert_text(&format!("entry {i}"), None).unwrap();
        }
        let removed = s.cleanup(&cfg).unwrap();
        assert!(removed > 0);
        assert_eq!(s.count().unwrap(), 2);
        assert_eq!(s.fts_rows(), 2, "清理删掉的记录不该在 FTS 里留副本");
    }

    #[test]
    fn search_unions_fts_and_substring_hits() {
        let mut s = mem();
        s.insert_text("cargo build", None).unwrap();
        s.insert_text("mycargo wrapper", None).unwrap();
        let hits = s.search("cargo", 10).unwrap();
        assert_eq!(hits.len(), 2, "子串命中不能因为 FTS 已有命中而被丢掉");
    }

    #[test]
    fn search_survives_control_characters() {
        let mut s = mem();
        s.insert_text("hello", None).unwrap();
        assert!(s.search("a\u{0}b", 10).is_ok(), "含 NUL 的查询不该让整个搜索报错");
        assert!(s.search("\u{1}\u{2}", 10).is_ok());
    }

    #[test]
    fn pinning_a_missing_uid_does_not_clear_the_slot() {
        let mut s = mem();
        let a = s.insert_text("alpha", None).unwrap().unwrap();
        s.set_pinned(&a.id, Some(1)).unwrap();
        assert!(s.set_pinned("no-such-uid", Some(1)).is_err());
        assert!(s.get_pinned(1).unwrap().is_some(), "置顶失败不能把原槽位清空");
    }

    #[test]
    fn clip_is_byte_based_and_char_safe() {
        assert_eq!(clip_to(&"x".repeat(100), 10).len(), 10);
        // 中文 3 字节/字：4096 不是 3 的倍数，必须切回边界
        assert_eq!(clip_to(&"中".repeat(4000), 4096), "中".repeat(1365));
        // 兜底上限之内原样返回（100 KB 文本不再被砍半）
        let big = "x".repeat(100_000);
        assert_eq!(clipped(&big).len(), 100_000);
    }

    #[test]
    fn thumbnail_fits_the_box_but_keeps_aspect_ratio() {
        let src = image::RgbaImage::from_pixel(1000, 500, image::Rgba([200, 60, 60, 255]));
        let png = encode_png(&src).unwrap();
        let (thumb, tw, th) = make_thumbnail(&png).unwrap();
        assert_eq!((tw, th), (THUMB_MAX, THUMB_MAX / 2), "缩略图应等比缩放进 {THUMB_MAX} 方框");
        assert!(!thumb.is_empty());

        // 小图不放大
        let small = image::RgbaImage::from_pixel(80, 40, image::Rgba([10, 10, 10, 255]));
        let (_, sw, sh) = make_thumbnail(&encode_png(&small).unwrap()).unwrap();
        assert_eq!((sw, sh), (80, 40));
    }
}
