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

#[tauri::command]
pub fn copy_entry(state: State<'_, std::sync::Arc<AppState>>, id: String) -> Result<()> {
    let entry = state.store.lock().get(&id)?.ok_or_else(|| crate::Error::Other("记录不存在".into()))?;
    clipboard::write_entry(&entry)?;
    // 防止 watcher 把这次写入当成一次新的复制
    paste::settle();
    state.suppress_seq.store(crate::win::clipboard_sequence(), Ordering::Relaxed);
    Ok(())
}

/// 清空历史。`keep_pinned = true` 时保留置顶条目。返回删除条数。
#[tauri::command]
pub fn clear_entries(state: State<'_, std::sync::Arc<AppState>>, keep_pinned: bool) -> Result<u32> {
    let ids: Vec<String> = {
        let store = state.store.lock();
        store
            .list(5000)?
            .into_iter()
            .filter(|e| !(keep_pinned && e.pinned.is_some()))
            .map(|e| e.id)
            .collect()
    };
    let n = ids.len() as u32;
    {
        let mut store = state.store.lock();
        for id in ids {
            store.delete(&id)?;
        }
    }
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

    state.update_config(next.clone());
    next.save()?;

    if old.hotkey != next.hotkey || old.slot_hotkeys != next.slot_hotkeys {
        crate::rebind_hotkeys(&app, state.inner())?;
    }
    if old.autostart != next.autostart {
        apply_autostart(&app, next.autostart);
    }
    state.notify_changed();
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
