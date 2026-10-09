//! Thin, safe-ish wrappers around the Win32 calls the app needs.
//!
//! Everything here is Windows-only; the rest of the crate stays portable enough
//! to reason about (and to unit test) without touching these functions.

#![allow(dead_code)]

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::DataExchange::{GetClipboardSequenceNumber, IsClipboardFormatAvailable};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};

pub const CF_HDROP: u32 = 15;
pub const CF_BITMAP: u32 = 2;
pub const CF_DIB: u32 = 8;
pub const CF_DIBV5: u32 = 17;
pub const CF_UNICODETEXT: u32 = 13;

/// 剪切板变化序号——序号变化即代表有新复制。
pub fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn format_available(fmt: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(fmt).is_ok() }
}

/// 剪切板里是否是「文件 / 文件夹」内容（本期不做，直接忽略）。
pub fn has_file_list() -> bool {
    format_available(CF_HDROP)
}

pub fn has_text() -> bool {
    format_available(CF_UNICODETEXT)
}

pub fn has_image() -> bool {
    [CF_DIB, CF_DIBV5, CF_BITMAP].iter().any(|f| format_available(*f))
}

// ------------------------------------------------------------------ focus

/// 当前前台窗口。粘贴链路第 1 步记录它，第 5 步还给它。
pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// 把焦点还给目标窗口。返回是否成功——失败通常意味着对方是高权限窗口。
pub fn set_foreground(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { SetForegroundWindow(HWND(hwnd as *mut core::ffi::c_void)).as_bool() }
}

pub fn current_thread_id() -> u32 {
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

/// 前台窗口所属进程的可执行文件名（不含路径、不含扩展名）。
pub fn foreground_process_name() -> Option<String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let res = unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    let _ = unsafe { CloseHandle(handle) };
    res.ok()?;
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit(['\\', '/'])
        .next()
        .map(|f| f.trim_end_matches(".exe").to_string())
}

// ------------------------------------------------------------------ input

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 模拟一次 `Ctrl+V`。返回实际注入的事件数。
pub fn send_ctrl_v() -> u32 {
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) }
}
