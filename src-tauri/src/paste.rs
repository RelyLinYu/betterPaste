//! The paste executor — the "focus killer".
//!
//! Sequence (matching §2.4 of the design doc):
//!   1. the caller records the foreground window A when the hotkey fires
//!   2. the overlay is shown *without* stealing focus
//!   3. the user picks an entry and presses Enter
//!   4. we write the clip to the clipboard and wait for it to take effect
//!   5. `SetForegroundWindow(A)` gives focus back
//!   6. `SendInput` types `Ctrl+V`
//!   7. the overlay is closed
//!   8. if A is an elevated window the simulated keystroke cannot land, so we
//!      fall back to "already copied — press Ctrl+V yourself"

use std::time::Duration;

use crate::win;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteOutcome {
    /// 正常粘贴
    Pasted,
    /// 未能还焦（大概率是管理员窗口），内容已放进剪贴板
    ManualFallback,
}

const FOCUS_SETTLE_MS: u64 = 60;
const CLIPBOARD_SETTLE_MS: u64 = 25;

/// 还焦 + 模拟 Ctrl+V。调用前剪贴板内容必须已经写好。
pub fn deliver(target_hwnd: isize) -> PasteOutcome {
    let already_focused = win::foreground_window() == target_hwnd;
    if !already_focused && !win::set_foreground(target_hwnd) {
        // 目标窗口无法被激活：可能是高权限窗口，或者窗口已经关了。
        return PasteOutcome::ManualFallback;
    }
    std::thread::sleep(Duration::from_millis(FOCUS_SETTLE_MS));

    let injected = win::send_ctrl_v();
    std::thread::sleep(Duration::from_millis(CLIPBOARD_SETTLE_MS));

    if injected == 0 {
        PasteOutcome::ManualFallback
    } else {
        PasteOutcome::Pasted
    }
}

/// 写剪贴板之后给系统一点时间，避免目标程序读到旧内容。
pub fn settle() {
    std::thread::sleep(Duration::from_millis(CLIPBOARD_SETTLE_MS));
}
