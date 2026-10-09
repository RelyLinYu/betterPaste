//! System tray: 打开 / 暂停记录 / 设置 / 退出。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::app::{AppState, OVERLAY, SETTINGS};
use crate::commands::{hide_overlay_window, show_window};

pub const ID_SHOW: &str = "clipstack.show";
pub const ID_PAUSE: &str = "clipstack.pause";
pub const ID_SETTINGS: &str = "clipstack.settings";
pub const ID_QUIT: &str = "clipstack.quit";

pub const TRAY_ID: &str = "clipstack";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .item(&MenuItemBuilder::with_id(ID_SHOW, "打开剪贴板历史").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id(ID_PAUSE, "暂停 / 恢复记录").build(app)?)
        .item(&MenuItemBuilder::with_id(ID_SETTINGS, "设置…").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id(ID_QUIT, "退出 clipstack").build(app)?)
        .build()?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // 右键出菜单；双击呼出浮层，避免和菜单抢左键
        .show_menu_on_left_click(false)
        .tooltip(tooltip(false))
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                let app = tray.app_handle().clone();
                let state = app.state::<Arc<AppState>>().inner().clone();
                crate::commands::show_overlay(&app, &state);
            }
        })
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            let state = app.state::<Arc<AppState>>().inner().clone();
            match id.as_str() {
                ID_SHOW => crate::commands::show_overlay(app, &state),
                ID_SETTINGS => show_window(app, SETTINGS),
                ID_PAUSE => {
                    let paused = state.toggle_paused();
                    sync_pause_state(app, paused);
                }
                ID_QUIT => {
                    state.shutdown.store(true, Ordering::Relaxed);
                    // 给 watcher 一轮时间收尾，然后把 WAL 落回主库——之前直接 exit(0)，
                    // 主库长期停在 4 KiB、-wal 一直膨胀。
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    if let Err(e) = state.store.lock().checkpoint() {
                        log::warn!("退出前 checkpoint 失败: {e}");
                    }
                    app.global_shortcut().unregister_all().ok();
                    app.exit(0);
                }
                _ => {}
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    let tray = builder.build(app)?;
    let _ = tray.set_visible(true);
    Ok(())
}

/// 把「暂停 / 记录中」反映到托盘提示上（设置面板里切换时也会调用）。
pub fn sync_pause_state(app: &AppHandle, paused: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip(paused)));
    }
}

fn tooltip(paused: bool) -> String {
    if paused {
        "clipstack — 已暂停记录".to_string()
    } else {
        "clipstack — 剪贴板历史".to_string()
    }
}

/// 单实例回调：第二次启动时把浮层切出来。
pub fn focus_overlay(app: &AppHandle, state: &Arc<AppState>) {
    if let Some(w) = app.get_webview_window(OVERLAY) {
        if w.is_visible().unwrap_or(false) {
            hide_overlay_window(app);
            return;
        }
    }
    crate::commands::show_overlay(app, state);
}
