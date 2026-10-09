//! Application entry point and the wiring between all the modules.

pub mod app;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod paste;
pub mod paths;
pub mod store;
pub mod tray;
pub mod win;

pub use error::{Error, Result};

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::{Manager, WindowEvent};

use app::{AppState, OVERLAY, SETTINGS};
use config::AppConfig;

/// 注册浮层与置顶槽热键。
pub fn rebind_hotkeys(app: &tauri::AppHandle, state: &Arc<AppState>) -> Result<()> {
    hotkey::unregister_all(app);
    let cfg = state.config();

    let app_for_overlay = app.clone();
    let state_for_overlay = state.clone();
    hotkey::register_overlay_hotkey(app, &cfg.hotkey, move || {
        commands::show_overlay(&app_for_overlay, &state_for_overlay);
    })?;

    let state_for_slot = state.clone();
    // 单个槽位注册失败不应该拖垮整个换绑流程，逐个记录即可
    if let Err(e) = hotkey::register_slot_hotkeys(app, &cfg.slot_hotkeys, move |slot| {
        // 直贴：把焦点还给调用前的窗口，再模拟 Ctrl+V
        let entry = state_for_slot.store.lock().get_pinned(slot).ok().flatten();
        let Some(entry) = entry else {
            log::debug!("槽位 {slot} 是空的");
            return;
        };
        let target = win::foreground_window();
        state_for_slot.target_hwnd.store(target as i64, Ordering::Relaxed);
        if let Err(e) = clipboard::write_entry(&entry) {
            log::warn!("槽位直贴失败: {e}");
            return;
        }
        paste::settle();
        state_for_slot
            .suppress_seq
            .store(win::clipboard_sequence(), Ordering::Relaxed);
        let outcome = paste::deliver(target);
        if matches!(outcome, paste::PasteOutcome::ManualFallback) {
            log::warn!("槽位 {slot}：目标窗口无法激活，请手动 Ctrl+V");
        }
    }) {
        log::warn!("{e}");
    }

    Ok(())
}

/// 启动时希望立刻显示的窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartWindow {
    None,
    Overlay,
    Settings,
}

/// 启动核心：被 `main.rs` 和移动端入口共用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with(StartWindow::None)
}

pub fn run_with(start: StartWindow) {
    let store = store::Store::open().expect("无法打开剪贴板数据库");
    let cfg = AppConfig::load();

    tauri::Builder::default()
        // single-instance 必须第一个注册
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            use tauri::Manager as _;
            let state = app.state::<Arc<AppState>>().inner().clone();
            tray::focus_overlay(app, &state);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_log::Builder::new()
                // `.target()` 是"追加"，不指定 targets 时默认还会写 stdout，
                // 结果是每条日志出现两份。这里显式给唯一的文件 target。
                .clear_targets()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir { file_name: Some("clipstack".into()) },
                ))
                .level(log::LevelFilter::Info)
                .build(),
        )
        .setup(move |app| {
            let handle = app.handle().clone();
            let state = AppState::new(cfg.clone(), store);
            state.bind_app(handle.clone());
            app.manage(state.clone());

            // 开机自启的实际状态以配置为准
            {
                use tauri_plugin_autostart::ManagerExt;
                let mgr = handle.autolaunch();
                let res = if cfg.autostart { mgr.enable() } else { mgr.disable() };
                if let Err(e) = res {
                    log::warn!("同步开机自启失败: {e}");
                }
            }

            if let Err(e) = rebind_hotkeys(&handle, &state) {
                log::error!("{e}");
                // 热键被占用不致命：托盘和设置面板仍然可用
            }

            tray::build(&handle)?;
            clipboard::spawn_watcher(state.clone());

            // --settings / --overlay：启动后直接亮出对应的窗口
            match start {
                StartWindow::Overlay => commands::show_overlay(&handle, &state),
                StartWindow::Settings => commands::show_window(&handle, SETTINGS),
                StartWindow::None => {}
            }

            log::info!("clipstack 已启动，数据目录：{}", paths::data_dir()?.display());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // 浮层失焦自动收起；设置窗口点关闭只是隐藏（托盘仍在）
            WindowEvent::Focused(false) => {
                if window.label() == OVERLAY {
                    if let Some(state) = window.try_state::<Arc<AppState>>() {
                        if state.config().hide_on_blur {
                            let _ = window.hide();
                        }
                    }
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == SETTINGS {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_entries,
            commands::search_entries,
            commands::pin_entry,
            commands::delete_entry,
            commands::copy_entry,
            commands::clear_entries,
            commands::paste_entry,
            commands::paste_slot,
            commands::paste_text,
            commands::get_config,
            commands::set_config,
            commands::set_paused,
            commands::get_paused,
            commands::data_dir,
            commands::open_settings,
            commands::hide_overlay,
        ])
        .run(tauri::generate_context!())
        .expect("clipstack 启动失败");
}
