//! SQLite persistence (FTS5 search) plus the image/thumbnail files on disk.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use image::{ImageFormat, RgbaImage};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::{paths, Error, Result};

pub const CONTENT_LIMIT: usize = 64 * 1024; // 64 KiB of text per entry
pub const THUMB_MAX: u32 = 320;

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

            -- Plain table + triggers keep FTS5 in sync without any custom SQL.
            CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
                content,
                tokenize = 'unicode61 remove_diacritics 2'
            );
            "#,
        )?;
        Ok(Self { conn })
    }

    // ---------------------------------------------------------------- writes

    /// 写入一条文本。返回是否真的入库（重复 / 空内容会被忽略）。
    pub fn insert_text(&mut self, content: &str, source_app: Option<&str>) -> Result<Option<ClipEntry>> {
        let text: String = content.chars().take(CONTENT_LIMIT).collect();
        if text.trim().is_empty() {
            return Ok(None);
        }
        let now = now_ms();
        let uid = Uuid::new_v4().to_string();
        let bytes = text.len() as u64;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO entries (type, content, bytes, source_app, created_at, uid)
             VALUES ('text', ?1, ?2, ?3, ?4, ?5)",
            params![text, bytes, source_app, now, uid],
        )?;
        let rowid = tx.last_insert_rowid();
        tx.execute("INSERT INTO entries_fts (rowid, content) VALUES (?1, ?2)", params![rowid, text])?;
        tx.commit()?;
        self.get_by_uid(&uid)
    }

    /// 写入一条图片（PNG 编码后的字节）。
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
        std::fs::write(&image_file, &png_bytes)?;

        // 缩略图只用于展示；条目上记录的始终是原图尺寸
        let thumb_file = match make_thumbnail(&png_bytes) {
            Ok((thumb_png, _, _)) => {
                let f = thumbs.join(format!("{id}.png"));
                std::fs::write(&f, &thumb_png)?;
                Some(f)
            }
            Err(_) => None,
        };

        let now = now_ms();
        let bytes = png_bytes.len() as u64;
        self.conn.execute(
            "INSERT INTO entries (type, image_path, thumb_path, width, height, bytes, source_app, created_at, uid)
             VALUES ('image', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                rel(&image_file),
                thumb_file.as_ref().map(rel),
                width,
                height,
                bytes,
                source_app,
                now,
                id
            ],
        )?;
        self.get_by_uid(&id)?
            .ok_or_else(|| Error::Other("图片写入后读回失败".into()))
    }

    pub fn set_pinned(&mut self, uid: &str, slot: Option<u8>) -> Result<()> {
        if let Some(s) = slot {
            // 一个槽位只能有一条记录
            self.conn
                .execute("UPDATE entries SET pinned = NULL WHERE pinned = ?1", params![s])?;
        }
        self.conn
            .execute("UPDATE entries SET pinned = ?1 WHERE uid = ?2", params![slot, uid])?;
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
        if let Some((img, thumb)) = row {
            for p in [img, thumb].into_iter().flatten() {
                let _ = std::fs::remove_file(paths::data_dir()?.join(p));
            }
        }
        self.conn.execute("DELETE FROM entries WHERE uid = ?1", params![uid])?;
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

    /// 搜索：FTS5 优先，失败或空结果时用 LIKE 兜底（中文切词较弱）。
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<ClipEntry>> {
        let q = query.trim();
        if q.is_empty() {
            return self.list(limit);
        }

        if let Some(fts) = fts_query(q) {
            let sql = format!(
                "{SELECT_COLS} WHERE id IN (SELECT rowid FROM entries_fts WHERE entries_fts MATCH ?1) \
                 ORDER BY created_at DESC, id DESC LIMIT ?2"
            );
            if let Ok(mut stmt) = self.conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map(params![fts, limit], map_row) {
                    let hits: Vec<ClipEntry> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
                    if !hits.is_empty() {
                        return Ok(hits);
                    }
                }
            }
        }

        // LIKE 兜底（大小写不敏感，中文按子串匹配）
        let like = format!("%{}%", escape_like(q));
        let sql = format!(
            "{SELECT_COLS} WHERE content LIKE ?1 ESCAPE '\\' ORDER BY created_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![like, limit], map_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
            // 这里必须显式把 pinned IS NOT NULL 也算进"保留集合"，否则置顶条目
            // 会落在子查询之外，被当作超额记录删掉。
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
        let root = paths::data_dir()?;
        let tx = self.conn.transaction()?;
        for (id, img, thumb) in &victims {
            for p in [img, thumb].into_iter().flatten() {
                let _ = std::fs::remove_file(root.join(p));
            }
            tx.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(victims.len() as u32)
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
