//! Clipboard reading/writing + the background watcher thread.
//!
//! Reading rules (design doc §2.4):
//!   * 纯文本 → 记录
//!   * 位图   → 记录（PNG 落盘 + 缩略图）
//!   * 文件列表 / 富文本 / HTML → 忽略

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use parking_lot::Mutex;

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

// ------------------------------------------------------------------ 自写入抑制

/// 我们自己刚写进剪贴板的内容指纹。
///
/// 只靠序号（`suppress_seq`）挡不住：写剪贴板与记账之间有 25 ms 空窗，watcher 每
/// 60 ms 轮询一次，落在窗口里的那一次会先把新序号登记成"待捕获"，而 `decide()` 的
/// pending 分支根本不看 `suppress_seq` → 于是"从历史里粘贴一条旧内容"会把它自己
/// 再记一遍（实测：库里出现两行完全相同的内容）。按内容指纹比对就没有这个竞态。
enum Fingerprint {
    Text(String),
    Image(u64),
}

static SUPPRESS: Mutex<Option<(Fingerprint, Instant)>> = Mutex::new(None);

/// 指纹的有效期：超过它就当用户自己的复制。
const SUPPRESS_WINDOW: Duration = Duration::from_secs(8);

fn mark_suppressed(fp: Fingerprint) {
    *SUPPRESS.lock() = Some((fp, Instant::now()));
}

/// 这次读到的文本是不是我们自己刚写进去的那一条（是则消费掉指纹）。
fn suppressed_text(text: &str) -> bool {
    let mut guard = SUPPRESS.lock();
    match guard.as_ref() {
        Some((_, at)) if at.elapsed() >= SUPPRESS_WINDOW => {
            *guard = None;
            false
        }
        Some((Fingerprint::Text(t), _)) if t == text => {
            *guard = None;
            true
        }
        _ => false,
    }
}

fn suppressed_image(png: &[u8]) -> bool {
    // 和 store 的去重共用同一个哈希实现，别让两处逻辑漂移
    let h = store::Store::image_hash(png) as u64;
    let mut guard = SUPPRESS.lock();
    match guard.as_ref() {
        Some((_, at)) if at.elapsed() >= SUPPRESS_WINDOW => {
            *guard = None;
            false
        }
        Some((Fingerprint::Image(x), _)) if *x == h => {
            *guard = None;
            true
        }
        _ => false,
    }
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
        .ok_or_else(|| Error::Clipboard("写入剪贴板失败（可能被其他程序占用）".into()))?;
    // 记账放在写入之后、且在返回之前：watcher 若在这之后才轮询到，就靠指纹挡住
    mark_suppressed(Fingerprint::Text(text.to_string()));
    Ok(())
}

/// 把图片同时以 PNG 与 DIB 放进剪贴板——不同程序偏好不同格式。
pub fn write_image(png_bytes: &[u8]) -> Result<()> {
    let img = image::load_from_memory(png_bytes)?;
    // into_rgba8 直接消耗解码结果（to_rgba8 会再复制一份）；下面的 clone 是因为
    // with_clipboard 的回调是 Fn（可能重试两次），拿不出所有权。
    let rgba = img.into_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    with_clipboard(|cb| {
        cb.set_image(arboard::ImageData {
            width: w,
            height: h,
            bytes: std::borrow::Cow::Owned(rgba.clone().into_raw()),
        })
    })
    .ok_or_else(|| Error::Clipboard("写入图片失败（可能被其他程序占用）".into()))?;
    mark_suppressed(Fingerprint::Image(store::Store::image_hash(png_bytes) as u64));
    Ok(())
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
        .name("betterPaste-watcher".into())
        .spawn(move || watch_loop(state))
        .expect("failed to spawn clipboard watcher thread");
}

/// 读失败后等待重试的时间窗（覆盖别的程序"打开-清空-写入-关闭"的间隙）。
const CAPTURE_RETRY_WINDOW: Duration = Duration::from_millis(1200);

/// 一条内容最多重试这么多次就放弃（推进序号）。
///
/// 之前 `Ok(false)`（剪贴板忙 / 图片解不开）分支**没有任何超时**，`expired` 还被
/// `let _ = expired;` 丢掉：剪贴板里只要有一个解不开的位图，就会每 60 ms 重开一次
/// 剪贴板无限重试，`last_seq` 永不推进。
const CAPTURE_MAX_ATTEMPTS: u32 = 6;

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
    let mut attempts: u32 = 0;

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
            Decision::CaptureNew(s) | Decision::Retarget(s) => {
                pending = Some((s, std::time::Instant::now() + CAPTURE_RETRY_WINDOW));
                attempts = 0;
            }
            Decision::RetryPending(s) => {
                attempts += 1;
                // 采集里出现的任何 panic（例如新的边界 bug）都不该让"记录"永久停摆：
                // 兜住它、记日志、跳过这一轮，下一轮照常工作。
                let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| capture_once(&state)));
                let outcome = match captured {
                    Ok(r) => r,
                    Err(_) => {
                        log::error!("捕获剪贴板时 panic，已跳过这一轮（记录继续）");
                        pending = None;
                        last_seq = s;
                        continue;
                    }
                };
                match outcome {
                    Ok(true) => {
                        last_seq = s;
                        pending = None;
                        attempts = 0;
                    }
                    Ok(false) => {
                        // 剪贴板还忙：保持 pending，但重试有上限
                        if expired || attempts >= CAPTURE_MAX_ATTEMPTS {
                            log::warn!("剪贴板内容连续 {attempts} 次没读出来，放弃这一条（序号 {s}）");
                            last_seq = s;
                            pending = None;
                            attempts = 0;
                        }
                    }
                    Err(e) => {
                        log::warn!("捕获剪贴板失败: {e}");
                        // 真错误（数据库等）不该无限重试：超时就放弃
                        if expired || attempts >= CAPTURE_MAX_ATTEMPTS {
                            last_seq = s;
                            pending = None;
                            attempts = 0;
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
    // 我们自己刚写进剪贴板的内容（粘贴历史条目 / copy_entry）不算新复制
    if suppressed_text(text) {
        log::debug!("跳过自己写入的文本（{} 字节）", text.len());
        return Ok(());
    }
    let clipped = store::clipped(text);
    {
        let mut store = state.store.lock();
        // 内容和最新一条完全相同就不重复入库。
        // 比较必须用**截断后**的文本：库里存的是截断版，拿原文比会让超长文本永远"不同"。
        if store.recent_content()?.as_deref() == Some(clipped.as_str()) {
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
    if suppressed_image(&png) {
        log::debug!("跳过自己写入的图片");
        return Ok(());
    }
    {
        let mut store = state.store.lock();
        // 连续同一张图（截图工具一次复制常见两次剪贴板变化）只留一条
        if store.insert_image(png, w, h, source_app)?.is_none() {
            log::debug!("同一张图紧接着又出现，跳过");
            return Ok(());
        }
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
/// Windows 上「复制富文本」会同时放 `HTML Format` 与 `CF_UNICODETEXT`，后者就是标记语言
/// 源码；本期只做纯文本，所以把这种内容丢掉。
///
/// 两个曾经的坑都在这里：
///   * `head[..head.len().min(4096)]` 是**按字节**切 UTF-8 字符串，复制一段长中文
///     （第 4096 字节落在字符中间，4096 % 3 = 1）直接 panic，把采集线程打死；
///   * 判定用的是 `contains`，正文里只要出现 `<body>` / `<html` 就整条丢弃，
///     普通代码和笔记被误杀。
pub fn looks_like_markup(text: &str) -> bool {
    let head = text.trim_start();
    // 取开头 4 KiB，且永远切在 char 边界上
    let end = head
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&e| e <= 4096)
        .last()
        .unwrap_or(0);
    let lower = head[..end].trim_start().to_ascii_lowercase();
    const SIGNATURES: [&str; 6] = [
        "<!doctype html",
        "<html",
        "<?xml",
        "<head>",
        "<body",
        "<meta ",
    ];
    // 只有"文档开头就是标记语言"才算源码；正文里提到这些标签的正常文本必须留下
    SIGNATURES.iter().any(|s| lower.starts_with(s))
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
        assert!(looks_like_markup("<meta charset=\"utf-8\">"));
    }

    /// 回归：复制 >4096 字节的中文曾让采集线程 panic
    /// （`head[..min(len,4096)]` 按字节切成两半），进程还在但永久不再记录。
    #[test]
    fn long_multibyte_text_never_panics() {
        assert!(!looks_like_markup(&"中".repeat(4000)));
        assert!(!looks_like_markup(&format!("ab{}", "中".repeat(3000))));
        assert!(!looks_like_markup(&format!("{}x", "😀".repeat(2000))));
        // 开头就是 HTML 的长文本仍然要被识别
        assert!(looks_like_markup(&format!("<html>{}", "中".repeat(4000))));
    }

    /// 回归：正文里提到 `<body>` / `<html>` 的正常文本不能被当成网页源码丢掉。
    #[test]
    fn markup_in_the_middle_of_prose_is_kept() {
        assert!(!looks_like_markup("function render() { return '<body>' + x; }"));
        assert!(!looks_like_markup("# notes about <html> tags"));
        assert!(!looks_like_markup("今天天气不错"));
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
