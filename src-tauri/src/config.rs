use serde::{Deserialize, Serialize};

use crate::{paths, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    /// 历史条数上限（0 = 不限制）
    pub max_entries: u32,
    /// 保留天数（0 = 不限制）
    pub max_age_days: u32,
    /// 单条体积上限（MB）
    pub max_item_mb: u32,
    /// 展示用历史条数
    pub history_limit: u32,

    pub capture_text: bool,
    pub capture_images: bool,

    /// 浮层热键，例如 "Ctrl+Shift+V"
    pub hotkey: String,
    /// 置顶槽直贴热键，例如 "Ctrl+Shift+1"
    pub slot_hotkeys: Vec<String>,

    pub theme: ThemeMode,

    /// 粘贴后是否关闭浮层
    pub close_after_paste: bool,
    /// 失去焦点时自动关闭浮层
    pub hide_on_blur: bool,
    /// 开机自启
    pub autostart: bool,
    /// 记录时忽略来自这些进程的内容（可留空）
    pub ignore_source_apps: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            max_entries: 500,
            max_age_days: 30,
            max_item_mb: 10,
            history_limit: 500,
            capture_text: true,
            capture_images: true,
            hotkey: "Ctrl+Shift+V".into(),
            slot_hotkeys: (1..=9).map(|i| format!("Ctrl+Shift+{i}")).collect(),
            theme: ThemeMode::System,
            close_after_paste: true,
            hide_on_blur: true,
            autostart: false,
            ignore_source_apps: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        match paths::config_path() {
            Ok(p) if p.exists() => match std::fs::read_to_string(&p) {
                Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
                Err(_) => Self::default(),
            },
            _ => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = paths::config_path()?;
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(p, raw)?;
        Ok(())
    }

    /// 条数上限换算成清理阈值（0 = 不限制 → None）
    pub fn entry_limit(&self) -> Option<u32> {
        (self.max_entries > 0).then_some(self.max_entries)
    }

    pub fn age_limit_days(&self) -> Option<u32> {
        (self.max_age_days > 0).then_some(self.max_age_days)
    }

    pub fn size_limit_bytes(&self) -> u64 {
        u64::from(self.max_item_mb.max(1)) * 1024 * 1024
    }
}

/// `clipstack --probe [query] [--bench N]`：不开 UI，直接打印最近的历史。
/// 带 `--bench` 时把查询重复 N 次并打印耗时——用来核对「500 条下搜索 ≤100ms」。
pub fn probe(query: Option<String>, bench: usize) {
    match crate::store::Store::open() {
        Ok(store) => {
            println!("数据目录: {}", crate::paths::data_dir().map(|p| p.display().to_string()).unwrap_or_default());
            println!("数据库  : {}", crate::paths::db_path().map(|p| p.display().to_string()).unwrap_or_default());
            let total = store.count().unwrap_or(0);
            // 打印用：最多 20 行
            let entries = match &query {
                Some(q) => store.search(q, 20),
                None => store.list(20),
            };
            // 统计用：不受打印上限影响，避免"数打印行数"这种错误测量
            let matches = match &query {
                Some(q) => store.search(q, 100_000).map(|v| v.len()).unwrap_or(0),
                None => total as usize,
            };

            if bench > 0 {
                // 计时口径：单次 Store::search（含 SQL 准备与行映射），不含进程启动
                let q = query.clone().unwrap_or_else(|| "a".into());
                let mut samples = Vec::with_capacity(bench);
                for _ in 0..bench {
                    let t = std::time::Instant::now();
                    let _ = store.search(&q, 500);
                    samples.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let avg: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
                println!(
                    "基准测试: query={q:?} 库内 {total} 条 / 重复 {} 次 → min {:.2}ms  中位 {:.2}ms  平均 {:.2}ms  max {:.2}ms",
                    samples.len(),
                    samples[0],
                    samples[samples.len() / 2],
                    avg,
                    samples[samples.len() - 1]
                );
            }

            match entries {
                Ok(list) => {
                    println!("共 {total} 条记录，匹配 {matches} 条，列出 {} 条：", list.len());
                    for e in list {
                        let head: String = e
                            .content
                            .clone()
                            .unwrap_or_else(|| format!("[图片 {}x{}]", e.width.unwrap_or(0), e.height.unwrap_or(0)))
                            .chars()
                            .take(50)
                            .collect();
                        println!(
                            "  #{} {:<5} {:>8}B pinned={:<4} {}",
                            e.created_at,
                            e.kind.as_str(),
                            e.bytes,
                            e.pinned.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                            head.replace('\n', "\\n").replace('\t', "\\t")
                        );
                    }
                }
                Err(e) => println!("查询失败: {e}"),
            }
        }
        Err(e) => println!("无法打开数据库: {e}"),
    }
}

/// `clipstack --ctl pin <slot> <substring>` / `--ctl unpin <slot>`：
/// 不开 UI 直接操作置顶槽，便于脚本化验证与故障排查。
pub fn ctl_pin(slot: u8, needle: &str) -> bool {
    if !(1..=9).contains(&slot) {
        println!("槽位必须是 1~9");
        return false;
    }
    match crate::store::Store::open() {
        Ok(mut store) => match store.search(needle, 50) {
            Ok(hits) => match hits.first() {
                Some(e) => match store.set_pinned(&e.id, Some(slot)) {
                    Ok(()) => {
                        println!("已把「{}」置顶到槽 {slot}", preview(e.content.as_deref()));
                        true
                    }
                    Err(err) => {
                        println!("置顶失败: {err}");
                        false
                    }
                },
                None => {
                    println!("没有匹配「{needle}」的记录");
                    false
                }
            },
            Err(e) => {
                println!("查询失败: {e}");
                false
            }
        },
        Err(e) => {
            println!("无法打开数据库: {e}");
            false
        }
    }
}

pub fn ctl_unpin(slot: u8) -> bool {
    match crate::store::Store::open() {
        Ok(mut store) => match store.get_pinned(slot) {
            Ok(Some(e)) => match store.set_pinned(&e.id, None) {
                Ok(()) => {
                    println!("已取消槽 {slot} 的置顶");
                    true
                }
                Err(err) => {
                    println!("取消失败: {err}");
                    false
                }
            },
            Ok(None) => {
                println!("槽 {slot} 本来就是空的");
                true
            }
            Err(e) => {
                println!("读取槽位失败: {e}");
                false
            }
        },
        Err(e) => {
            println!("无法打开数据库: {e}");
            false
        }
    }
}

fn preview(text: Option<&str>) -> String {
    text.unwrap_or("[图片]").chars().take(30).collect::<String>().replace('\n', "\\n")
}
