//! Clipboard reading/writing + the background watcher thread.
//!
//! Reading rules (design doc §2.4):
//!   * 纯文本 → 记录
//!   * 位图   → 记录（PNG 落盘 + 缩略图）
//!   * 文件列表 / 富文本 / HTML → 忽略

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use arboard::Clipboard;

use crate::app::AppState;
use crate::store::{self, ClipEntry, ClipKind};
use crate::win;
use crate::{Error, Result};

/// 轮询周期。Windows 上没有"剪贴板已更新"的回调可用（那要自己建消息窗口 +
/// `AddClipboardFormatListener`），所以用序号轮询。
///
/// 这个值直接决定能采到多快的连续复制：60ms 能跟上绝大多数人工操作，同时
/// `GetClipboardSequenceNumber` 是纯计数器查询、无需打开剪贴板（实测对
/// `Set-Clipboard` 零失败率），所以密一点没有副作用。
pub const POLL_INTERVAL_MS: u64 = 60;

/// 打开剪贴板失败后，在原地只做极短的一次退避再试。
///
/// Windows 剪贴板是**独占**资源，别的程序正在写时 `OpenClipboard` 会失败。
/// 但**绝不能在这里长时间死等**：那样"剪贴板越忙 → 单次读取阻塞越久 → 轮询线程
/// 越跟不上"会形成正反馈，反而大面积漏采（实测把退避拉到 48ms 时，40ms 节奏下
/// 只能采到 3/40）。真正的重试由 `watch_loop` 的 60ms 轮询承担。
///
/// arboard 自身在 get/set 内部已经带了 5 次 × 5ms 的重试，所以这里一次即可。
const ACQUIRE_ATTEMPTS: u32 = 2;
const ACQUIRE_BACKOFF_MS: u64 = 2;

thread_local! {
    static CLIP: RefCell<Option<Clipboard>> = const { RefCell::new(None) };
}

/// 用一次剪贴板（打开 → 操作 → 释放）。
///
/// 注意：`arboard::Clipboard::new()` 是空操作，真正的剪贴板句柄只在每次
/// get/set 期间存在，用完立刻释放——所以缓存这个对象不会长期霸占剪贴板。
fn with_clipboard<T>(f: impl Fn(&mut Clipboard) -> std::result::Result<T, arboard::Error>) -> Option<T> {
    for attempt in 0..ACQUIRE_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(ACQUIRE_BACKOFF_MS));
        }
        let opened = CLIP.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                *slot = Clipboard::new().ok();
            }
            let mut guard = slot.take()?;
            let result = f(&mut guard);
            *slot = Some(guard);
            Some(result)
        })?;
        match opened {
            Ok(v) => return Some(v),
            Err(e) => {
                log::debug!("剪贴板操作失败（第 {} 次）: {e}", attempt + 1);
                if attempt + 1 == ACQUIRE_ATTEMPTS {
                    return None;
                }
            }
        }
    }
    None
}

// ------------------------------------------------------------------ reading

/// 读取剪贴板文本（无文本则 `None`）。
pub fn read_text() -> Option<String> {
    with_clipboard(|cb| cb.get_text()).filter(|t| !t.is_empty())
}

/// 读取剪贴板图片，返回 `(PNG 字节, 宽, 高)`。
pub fn read_image_png() -> Option<(Vec<u8>, u32, u32)> {
    let img = with_clipboard(|cb| cb.get_image())?;
    let (w, h) = (img.width as u32, img.height as u32);
    let rgba = image::RgbaImage::from_raw(w, h, img.bytes.into_owned())?;
    let png = store::encode_png(&rgba).ok()?;
    Some((png, w, h))
}

// ------------------------------------------------------------------ writing

pub fn write_text(text: &str) -> Result<()> {
    with_clipboard(|cb| cb.set_text(text.to_string()))
        .ok_or_else(|| Error::Clipboard("写入剪贴板失败（可能被其他程序占用）".into()))
}

/// 把图片同时以 PNG 与 DIB 放进剪贴板——不同程序偏好不同格式。
pub fn write_image(png_bytes: &[u8]) -> Result<()> {
    let img = image::load_from_memory(png_bytes)?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    with_clipboard(|cb| {
        cb.set_image(arboard::ImageData {
            width: w,
            height: h,
            bytes: std::borrow::Cow::Owned(rgba.clone().into_raw()),
        })
    })
    .ok_or_else(|| Error::Clipboard("写入图片失败（可能被其他程序占用）".into()))
}

/// 把一条历史写回剪贴板，供粘贴使用。
pub fn write_entry(entry: &ClipEntry) -> Result<()> {
    match entry.kind {
        ClipKind::Text => write_text(entry.content.as_deref().unwrap_or_default()),
        ClipKind::Image => {
            let path = entry
                .image_path
                .as_ref()
                .ok_or_else(|| Error::Clipboard("图片条目缺少文件路径".into()))?;
            let bytes = std::fs::read(path)?;
            write_image(&bytes)
        }
    }
}

// ------------------------------------------------------------------ watcher

/// 后台常驻：轮询剪贴板序号，序号变化 = 有新复制。
pub fn spawn_watcher(state: Arc<AppState>) {
    std::thread::Builder::new()
        .name("clipstack-watcher".into())
        .spawn(move || watch_loop(state))
        .expect("failed to spawn clipboard watcher thread");
}

/// 读失败后等待重试的时间窗（覆盖别的程序"打开-清空-写入-关闭"的间隙）。
const CAPTURE_RETRY_WINDOW: Duration = Duration::from_millis(1200);

/// 一次轮询该怎么处理。抽成纯函数，因为"读失败却推进了序号"正是最初丢数据的 bug。
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// 没有新内容
    Skip,
    /// 新内容，抓它（并记住这个序号）
    CaptureNew(u32),
    /// 之前登记的内容还没抓到，但还在重试窗口内 → 再试
    RetryPending(u32),
    /// 期间又来了一次更新的复制 → 改追新的
    Retarget(u32),
    /// 我们自己写进去的，记账但不入库
    Suppress,
}

/// `pending` 为 `None` 时传 0；`last_seq` 为已确认捕获的序号。
///
/// 这里刻意**没有**"超时放弃"分支：即使过了软超时也要再给一次捕获机会，"放弃"
/// 由调用方在 capture 真正失败之后决定。否则一次瞬时占用就会丢掉用户的内容。
fn decide(
    last_seq: u32,
    pending: Option<u32>,
    seq: u32,
    suppress_seq: u32,
    expired: bool,
) -> Decision {
    let _ = expired;
    match pending {
        None => {
            if seq == last_seq {
                Decision::Skip
            } else if seq == suppress_seq {
                Decision::Suppress
            } else {
                Decision::CaptureNew(seq)
            }
        }
        Some(target) => {
            if seq == last_seq {
                // 序号回到已捕获状态（序号单调递增，正常不会发生；只作兜底，
                // 避免把一条已经处理过的内容再抓一遍）
                Decision::Skip
            } else if seq == target {
                // 还是那条待捕获的内容 → 继续重试
                Decision::RetryPending(target)
            } else {
                // 期间又来了更新的复制 → 改追新的
                Decision::Retarget(seq)
            }
        }
    }
}

fn watch_loop(state: Arc<AppState>) {
    let mut last_seq = win::clipboard_sequence();
    let mut was_paused = !state.is_capturing();
    // 待确认的序号 + 重试截止时间：读失败时不能推进 last_seq，否则内容会被永久丢掉
    let mut pending: Option<(u32, std::time::Instant)> = None;

    while !state.shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));

        let paused = !state.is_capturing();
        if paused {
            was_paused = true;
            pending = None;
            continue;
        }
        if was_paused {
            // 从暂停恢复：已经过去的变化不补录
            was_paused = false;
            pending = None;
            last_seq = win::clipboard_sequence();
            state.suppress_seq.store(last_seq, Ordering::Relaxed);
            continue;
        }

        let seq = win::clipboard_sequence();
        let suppress = state.suppress_seq.load(Ordering::Relaxed);
        let expired = pending.map(|(_, d)| std::time::Instant::now() >= d).unwrap_or(false);

        match decide(last_seq, pending.map(|(s, _)| s), seq, suppress, expired) {
            Decision::Skip => {}
            Decision::Suppress => {
                // 这是我们自己写进去的那一次，记账并清掉标记。
                // 不清掉的话，下次真实复制若恰好又撞上同一个序号（序号会回绕）会被误吞。
                last_seq = seq;
                state.suppress_seq.store(0, Ordering::Relaxed);
            }
            Decision::CaptureNew(s) => {
                pending = Some((s, std::time::Instant::now() + CAPTURE_RETRY_WINDOW));
            }
            Decision::Retarget(s) => {
                pending = Some((s, std::time::Instant::now() + CAPTURE_RETRY_WINDOW));
            }
            Decision::RetryPending(s) => {
                match capture_once(&state) {
                    Ok(true) => {
                        last_seq = s;
                        pending = None;
                    }
                    Ok(false) => { /* 剪贴板还忙：保持 pending，下一轮再试 */ }
                    Err(e) => {
                        log::warn!("捕获剪贴板失败: {e}");
                        // 真错误（数据库等）不该无限重试：超时就放弃
                        if expired {
                            last_seq = s;
                            pending = None;
                        }
                    }
                }
            }
        }
    }
}

/// 尝试捕获当前剪贴板内容。
/// `Ok(true)` = 读到了内容（已入库或已判定为无需入库）；
/// `Ok(false)` = 这一次没读到（剪贴板忙 / 为空），值得重试。
fn capture_once(state: &Arc<AppState>) -> Result<bool> {
    // 文件 / 文件夹：本期不支持，直接忽略
    if win::has_file_list() {
        return Ok(true);
    }

    let cfg = state.config();
    let source_app = win::foreground_process_name();

    if cfg.capture_text {
        if let Some(text) = read_text() {
            // 富文本 / HTML 会被系统同时以纯文本形式暴露出来，丢弃这种标记语言源码
            if looks_like_markup(&text) {
                return Ok(true);
            }
            let size = text.len() as u64;
            if size > cfg.size_limit_bytes() {
                log::warn!("文本超过体积上限，已忽略（{} 字节）", size);
                return Ok(true);
            }
            store_text(state, &text, source_app.as_deref())?;
            return Ok(true);
        }
    }

    if cfg.capture_images && win::has_image() {
        if let Some((png, w, h)) = read_image_png() {
            if png.len() as u64 > cfg.size_limit_bytes() {
                log::warn!("图片超过体积上限，已忽略（{} 字节）", png.len());
                return Ok(true);
            }
            store_image(state, png, w, h, source_app.as_deref())?;
            return Ok(true);
        }
        // 有图片格式但没读到：剪贴板那一刻还忙，值得重试
        return Ok(false);
    }

    // 文本/图片都没读到，且确实有文本格式存在 → 多半是被短暂占用，值得重试
    if cfg.capture_text && win::has_text() {
        return Ok(false);
    }

    Ok(true)
}

fn store_text(state: &Arc<AppState>, text: &str, source_app: Option<&str>) -> Result<()> {
    // 内容和最新一条完全相同就不重复入库
    {
        let mut store = state.store.lock();
        if store.recent_content()?.as_deref() == Some(text) {
            return Ok(());
        }
        store.insert_text(text, source_app)?;
    }
    state.after_write();
    Ok(())
}

fn store_image(
    state: &Arc<AppState>,
    png: Vec<u8>,
    w: u32,
    h: u32,
    source_app: Option<&str>,
) -> Result<()> {
    {
        let mut store = state.store.lock();
        store.insert_image(png, w, h, source_app)?;
    }
    state.after_write();
    Ok(())
}

/// 用来打断「暂停 → 恢复」期间的误捕获。
pub struct SuppressGuard(pub Arc<AppState>);

impl SuppressGuard {
    pub fn new(state: Arc<AppState>) -> Self {
        Self(state)
    }
}

impl Drop for SuppressGuard {
    fn drop(&mut self) {
        self.0.suppress_seq.store(win::clipboard_sequence(), Ordering::Relaxed);
    }
}

/// 判断剪贴板里的文本是不是 HTML / XML 源码。
///
/// Windows 上「复制富文本」通常会在同一个剪贴板里同时放 `HTML Format` 和
/// `CF_UNICODETEXT`，后者就是标记语言源码。本期只做纯文本，所以把这种明显是
/// 网页源码的内容丢掉，其余文本照常记录。
pub fn looks_like_markup(text: &str) -> bool {
    let head = text.trim_start();
    let lower = head[..head.len().min(4096)].to_ascii_lowercase();
    const SIGNATURES: [&str; 6] = [
        "<!doctype html",
        "<html",
        "<?xml",
        "<head>",
        "<body",
        "<meta ",
    ];
    SIGNATURES.iter().any(|s| lower.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_interval_is_snappy_enough_for_200ms_budget() {
        assert!(POLL_INTERVAL_MS <= 100, "轮询太慢会漏掉快速连续复制");
    }

    // ---- 时序状态机：这些用例锁住的是「读失败不能丢内容」这个回归 ----

    #[test]
    fn idle_clipboard_is_skipped() {
        assert_eq!(decide(10, None, 10, 0, false), Decision::Skip);
    }

    #[test]
    fn a_new_sequence_is_captured() {
        assert_eq!(decide(10, None, 11, 0, false), Decision::CaptureNew(11));
    }

    #[test]
    fn our_own_write_is_suppressed() {
        assert_eq!(decide(10, None, 11, 11, false), Decision::Suppress);
    }

    #[test]
    fn failed_read_keeps_retrying_instead_of_dropping_the_entry() {
        // 登记 11 → 下一轮剪贴板仍忙（读失败）→ 必须继续重试，而不是推进序号
        assert_eq!(decide(10, Some(11), 11, 0, false), Decision::RetryPending(11));
        // 即使已经过了软超时，也要再给一次机会（由调用方决定是否放弃）
        assert_eq!(decide(10, Some(11), 11, 0, true), Decision::RetryPending(11));
    }

    #[test]
    fn a_burst_of_copies_retargets_to_the_newest() {
        // 还没抓到 11，剪贴板已经变成 12 → 改追 12，不丢最新的
        assert_eq!(decide(10, Some(11), 12, 0, false), Decision::Retarget(12));
    }

    #[test]
    fn a_capture_in_flight_is_not_retargeted_by_its_own_sequence() {
        // 待捕获 12，当前序号仍是 12 → 老实重试，别当成"更新的内容"
        assert_eq!(decide(10, Some(12), 12, 0, false), Decision::RetryPending(12));
    }

    #[test]
    fn a_regressed_sequence_is_never_reprocessed() {
        // 理论上不会发生（序号单调递增）：只作兜底，避免重复抓取已处理内容。
        // 待捕获 12、已捕获 10，而当前序号又回到 10 —— 该跳过而不是改追 10。
        assert_eq!(decide(10, Some(12), 10, 0, false), Decision::Skip);
    }

    #[test]
    fn detects_html_sources() {
        assert!(looks_like_markup("<!DOCTYPE html><html><body>hi</body></html>"));
        assert!(looks_like_markup("  <html lang=\"en\">"));
        assert!(looks_like_markup("<?xml version=\"1.0\"?>"));
        assert!(looks_like_markup("<div><meta charset=\"utf-8\"></div>"));
    }

    #[test]
    fn keeps_ordinary_code_and_prose() {
        assert!(!looks_like_markup("fn main() { println!(\"hi\"); }"));
        assert!(!looks_like_markup("今天天气不错"));
        assert!(!looks_like_markup("<T> generic type参数"));
        assert!(!looks_like_markup(""));
        assert!(!looks_like_markup("a < b && c > d"));
    }
}
