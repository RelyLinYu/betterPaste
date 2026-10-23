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
/// 已经在前台时也要给窗口一点时间（例如浮层刚隐藏）。
const ALREADY_FOCUSED_SETTLE_MS: u64 = 40;
/// 等用户松开 Ctrl/Shift/Alt 的上限（槽位直贴时这些键还按着）。
const MODIFIER_WAIT_MS: u64 = 600;

/// 还焦 + 模拟 Ctrl+V。调用前剪贴板内容必须已经写好。
pub fn deliver(target_hwnd: isize) -> PasteOutcome {
    if target_hwnd == 0 {
        return PasteOutcome::ManualFallback;
    }

    if win::foreground_window() == target_hwnd {
        // 已经在前台：不需要还焦（浮层自己在前台的情况由调用方纠正目标）
        std::thread::sleep(Duration::from_millis(ALREADY_FOCUSED_SETTLE_MS));
    } else {
        if !win::set_foreground(target_hwnd) {
            // 目标窗口无法被激活：可能是高权限窗口，或者窗口已经关了。
            return PasteOutcome::ManualFallback;
        }
        std::thread::sleep(Duration::from_millis(FOCUS_SETTLE_MS));
        // SetForegroundWindow 在前台锁（foreground lock）下会"返回成功但没真的激活"，
        // 只信返回值的话 Ctrl+V 会落进别的窗口（很可能就是浮层自己）。
        if win::foreground_window() != target_hwnd {
            if !win::set_foreground(target_hwnd) || win::foreground_window() != target_hwnd {
                return PasteOutcome::ManualFallback;
            }
            std::thread::sleep(Duration::from_millis(FOCUS_SETTLE_MS));
        }
    }

    // 热键刚按下时修饰键还按着，必须等松开，否则注入的是 Ctrl+Shift+V
    win::wait_modifiers_released(Duration::from_millis(MODIFIER_WAIT_MS));

    let injected = win::send_ctrl_v();
    std::thread::sleep(Duration::from_millis(CLIPBOARD_SETTLE_MS));

    if injected == 0 {
        return PasteOutcome::ManualFallback;
    }
    if injected < 4 {
        // 部分注入：补一次全键抬起，免得 Ctrl 卡住，然后走手动兜底
        win::release_modifiers();
        return PasteOutcome::ManualFallback;
    }
    PasteOutcome::Pasted
}

/// 写剪贴板之后给系统一点时间，避免目标程序读到旧内容。
pub fn settle() {
    std::thread::sleep(Duration::from_millis(CLIPBOARD_SETTLE_MS));
}
