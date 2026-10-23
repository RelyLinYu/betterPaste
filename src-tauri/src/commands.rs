//! The IPC surface consumed by the overlay and settings windows.
//!
//! Every `#[tauri::command]` here is mirrored 1:1 in `src/lib/api.ts` — keep the
//! parameter names in sync (Tauri maps them from the JS object keys).

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager, State};
use crate::app::{AppState, OVERLAY, SETTINGS};
use crate::clipboard;
use crate::config::AppConfig;
use crate::hotkey;
use crate::paste::{self, PasteOutcome};
use crate::store::ClipEntry;
use crate::{paths, Result};

fn outcome_str(o: PasteOutcome) -> &'static str {
    match o {
        PasteOutcome::Pasted => "pasted",
        PasteOutcome::ManualFallback => "manualFallback",
    }
}

// ------------------------------------------------------------------- entries

#[tauri::command]
pub fn list_entries(state: State<'_, std::sync::Arc<AppState>>, limit: u32) -> Result<Vec<ClipEntry>> {
    state.store.lock().list(limit.clamp(1, 5000))
}

#[tauri::command]
pub fn search_entries(
    state: State<'_, std::sync::Arc<AppState>>,
    query: String,
    limit: u32,
) -> Result<Vec<ClipEntry>> {
    state.store.lock().search(&query, limit.clamp(1, 5000))
}

#[tauri::command]
pub fn pin_entry(state: State<'_, std::sync::Arc<AppState>>, id: String, slot: Option<u8>) -> Result<()> {
    if let Some(s) = slot {
        if !(1..=9).contains(&s) {
            return Err(crate::Error::Other("置顶槽只能是 1~9".into()));
        }
    }
    state.store.lock().set_pinned(&id, slot)?;
    state.notify_changed();
    Ok(())
}

#[tauri::command]
pub fn delete_entry(state: State<'_, std::sync::Arc<AppState>>, id: String) -> Result<()> {
    state.store.lock().delete(&id)?;
    state.notify_changed();
    Ok(())
}

/// 批量删除（浮层勾选多条后一次删掉）。返回真正删掉的条数。
#[tauri::command]
pub fn delete_entries(state: State<'_, std::sync::Arc<AppState>>, ids: Vec<String>) -> Result<u32> {
    let n = state.store.lock().delete_many(&ids)?;
    state.notify_changed();
    Ok(n)
}

/// 槽位 1~9 当前置顶的条目（空的为 `null`），设置面板与浮层都用它。
#[tauri::command]
pub fn pinned_slots(state: State<'_, std::sync::Arc<AppState>>) -> Result<Vec<Option<ClipEntry>>> {
    let store = state.store.lock();
    (1..=9u8).map(|slot| store.get_pinned(slot)).collect()
}

#[tauri::command]
pub fn copy_entry(state: State<'_, std::sync::Arc<AppState>>, id: String) -> Result<()> {
    let entry = state.store.lock().get(&id)?.ok_or_else(|| crate::Error::Other("记录不存在".into()))?;
    clipboard::write_entry(&entry)?;
    // 防止 watcher 把这次写入当成一次新的复制
    paste::settle();
    state.suppress_seq.store(crate::win::clipboard_sequence(), Ordering::Relaxed);
    // 但用户主动「复制」本身就该在历史里留一条（否则列表看不出发生了什么）。
    // 只有和当前最新一条内容相同才跳过，保证最上面两条不会重样。
    // 先拿到结果、放掉 store 锁，再发变更事件（别在持锁时 emit）。
    let recorded = state.store.lock().record_again(&entry);
    match recorded {
        Ok(true) => state.notify_changed(),
        Ok(false) => {}
        Err(e) => log::warn!("记录「从浮层复制」的历史失败: {e}"),
    }
    Ok(())
}

/// 清空历史。`keep_pinned = true` 时保留置顶条目。返回删除条数。
#[tauri::command]
pub fn clear_entries(state: State<'_, std::sync::Arc<AppState>>, keep_pinned: bool) -> Result<u32> {
    // 旧实现是 list(5000) 后逐条 delete：第 5000 条之后清不掉，
    // 而且每条一个独立事务（各自 fsync），全程占着 store 互斥锁。
    let n = state.store.lock().clear(keep_pinned)?;
    state.notify_changed();
    Ok(n)
}

// --------------------------------------------------------------------- paste

#[tauri::command]
pub fn paste_entry(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    id: String,
) -> Result<String> {
    let entry = state.store.lock().get(&id)?.ok_or_else(|| crate::Error::Other("记录不存在".into()))?;
    do_paste(&app, state.inner(), &entry)
}

#[tauri::command]
pub fn paste_slot(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    slot: u8,
) -> Result<String> {
    let entry = state
        .store
        .lock()
        .get_pinned(slot)?
        .ok_or_else(|| crate::Error::Other(format!("槽位 {slot} 还没有内容")))?;
    do_paste(&app, state.inner(), &entry)
}

#[tauri::command]
pub fn paste_text(app: AppHandle, state: State<'_, std::sync::Arc<AppState>>, text: String) -> Result<String> {
    clipboard::write_text(&text)?;
    paste::settle();
    state.suppress_seq.store(crate::win::clipboard_sequence(), Ordering::Relaxed);
    let target = state.target_hwnd.load(Ordering::Relaxed) as isize;
    let outcome = paste::deliver(target);
    let label = outcome_str(outcome).to_string();
    finish_paste(&app, state.inner(), outcome);
    Ok(label)
}

fn do_paste(app: &AppHandle, state: &std::sync::Arc<AppState>, entry: &ClipEntry) -> Result<String> {
    clipboard::write_entry(entry)?;
    paste::settle();
    state.suppress_seq.store(crate::win::clipboard_sequence(), Ordering::Relaxed);
    let target = state.target_hwnd.load(Ordering::Relaxed) as isize;
    let outcome = paste::deliver(target);
    let label = outcome_str(outcome).to_string();
    finish_paste(app, state, outcome);
    Ok(label)
}

fn finish_paste(app: &AppHandle, state: &std::sync::Arc<AppState>, outcome: PasteOutcome) {
    if matches!(outcome, PasteOutcome::Pasted) && state.config().close_after_paste {
        hide_overlay_window(app);
    }
}

// -------------------------------------------------------------------- config

/// 退出程序。托盘图标被隐藏后，这是唯一的退出入口（设置面板底部按钮）。
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    crate::tray::quit(&app);
}

#[tauri::command]
pub fn get_config(state: State<'_, std::sync::Arc<AppState>>) -> AppConfig {
    state.config()
}

/// 保存配置并把热键、自启动等副作用一起落实。
#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    config: AppConfig,
) -> Result<AppConfig> {
    let old = state.config();
    let mut next = config;
    next.hotkey = hotkey::normalize(&next.hotkey);
    next.slot_hotkeys = next.slot_hotkeys.iter().map(|k| hotkey::normalize(k)).collect();

    // 危险组合（Ctrl+V 之类）注册后会抢走系统功能，直接拒绝并说明原因
    if let Some(reason) = hotkey::danger_reason(&next.hotkey) {
        return Err(crate::Error::Hotkey(reason));
    }
    for (i, k) in next.slot_hotkeys.iter().enumerate() {
        if let Some(reason) = hotkey::danger_reason(k) {
            return Err(crate::Error::Hotkey(format!("槽位 {}：{reason}", i + 1)));
        }
    }

    state.update_config(next.clone());

    if old.hotkey != next.hotkey || old.slot_hotkeys != next.slot_hotkeys {
        if let Err(e) = crate::rebind_hotkeys(&app, state.inner()) {
            // 注册失败不能把用户原来能用的热键一起弄丢（配置已落盘的话重启也回不来）：
            // 回滚内存配置并尽量把旧热键装回去，然后把错误抛给前端。
            state.update_config(old.clone());
            if let Err(back) = crate::rebind_hotkeys(&app, state.inner()) {
                log::error!("回滚热键也失败: {back}");
            }
            return Err(e);
        }
    }
    if let Err(e) = next.save() {
        log::warn!("配置保存失败，回滚内存配置: {e}");
        state.update_config(old);
        return Err(e);
    }
    if old.autostart != next.autostart {
        apply_autostart(&app, next.autostart);
    }
    // 托盘图标可以随时开关：关掉后靠热键 + 设置面板，退出按钮在设置里
    if old.show_tray_icon != next.show_tray_icon {
        crate::tray::set_visible(&app, next.show_tray_icon);
    }
    state.notify_changed();
    // 主题之类的改动要让已经打开的浮层也立刻跟上
    state.notify_config();
    Ok(next)
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    let res = if enabled { mgr.enable() } else { mgr.disable() };
    if let Err(e) = res {
        log::warn!("设置开机自启失败: {e}");
    }
}

// --------------------------------------------------------------- app status

#[tauri::command]
pub fn set_paused(state: State<'_, std::sync::Arc<AppState>>, paused: bool) -> Result<bool> {
    state.set_paused(paused);
    Ok(paused)
}

#[tauri::command]
pub fn get_paused(state: State<'_, std::sync::Arc<AppState>>) -> bool {
    state.paused.load(Ordering::Relaxed)
}

#[tauri::command]
pub fn data_dir() -> Result<String> {
    Ok(paths::data_dir()?.to_string_lossy().to_string())
}

// --------------------------------------------------------------- 数据目录

/// 启动提示（首次运行、托盘「使用提示」）。前端取一次就清空。
static STARTUP_HINT: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

pub fn set_startup_hint(text: impl Into<String>) {
    *STARTUP_HINT.lock() = Some(text.into());
}

#[tauri::command]
pub fn take_startup_hint() -> Option<String> {
    STARTUP_HINT.lock().take()
}

#[tauri::command]
pub fn open_data_dir(app: AppHandle) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = paths::data_dir()?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| crate::Error::Other(format!("打开数据目录失败: {e}")))?;
    Ok(())
}

/// 把现有数据复制到新目录并记下指针，**重启后生效**。返回新目录。
#[tauri::command]
pub fn migrate_data_dir(state: State<'_, std::sync::Arc<AppState>>, target: String) -> Result<String> {
    let store = state.store.lock();
    crate::config::migrate_data_dir(&store, &target)
}

/// 恢复默认数据目录（重启后生效）。
#[tauri::command]
pub fn reset_data_dir(state: State<'_, std::sync::Arc<AppState>>) -> Result<String> {
    let store = state.store.lock();
    crate::config::reset_data_dir(&store)
}

/// 设置面板开始/结束录制热键。录制期间全局热键不抢按键，
/// 否则"想把呼出热键改成 Ctrl+Shift+V"会先被自己的全局热键截胡。
#[tauri::command]
pub fn set_recording(state: State<'_, std::sync::Arc<AppState>>, recording: bool) -> Result<bool> {
    state.set_recording(recording);
    Ok(recording)
}

/// 延迟重启自己。单实例插件会抢在旧进程退出前把新进程顶掉，所以让新进程等 1.2 秒。
#[tauri::command]
pub fn restart_app(app: AppHandle) -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| crate::Error::Other(format!("找不到自身路径: {e}")))?;
    let cmd = format!(
        "Start-Sleep -Milliseconds 1200; Start-Process -FilePath '{}'",
        exe.to_string_lossy().replace('\'', "''")
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &cmd])
        .spawn()
        .map_err(|e| crate::Error::Other(format!("重启失败: {e}")))?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<()> {
    show_window(&app, SETTINGS);
    Ok(())
}

#[tauri::command]
pub fn hide_overlay(app: AppHandle) -> Result<()> {
    hide_overlay_window(&app);
    Ok(())
}

// ------------------------------------------------------------------- windows

pub fn show_window(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn hide_overlay_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(OVERLAY) {
        let _ = w.hide();
    }
}

/// 呼出浮层：先记住当前前台窗口（粘贴时要还焦回去），再显示浮层。
pub fn show_overlay(app: &AppHandle, state: &std::sync::Arc<AppState>) {
    let target = crate::win::foreground_window();
    if target != 0 {
        state.target_hwnd.store(target as i64, Ordering::Relaxed);
    }
    // 让浮层每次呼出都从新内容开始
    state.notify_changed();
    show_window(app, OVERLAY);
}
