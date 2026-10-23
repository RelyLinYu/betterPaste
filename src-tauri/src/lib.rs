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

/// 关掉 WebView2 自带的浏览器快捷键。
///
/// 默认情况下 WebView2 会把 `Ctrl+P`（打印）、`F5`/`Ctrl+R`（刷新）、`Ctrl+F`（查找）等
/// 当成浏览器行为先处理掉，页面里的 `keydown` 根本收不到——浮层里 `Ctrl+P = 置顶`
/// 因此完全失效（还会弹出一个打印对话框）。关掉之后这些按键才回到我们自己手里。
#[cfg(windows)]
fn tame_webview_accelerators(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows62::core::Interface;
    let label = window.label().to_string();
    let outer_label = label.clone();
    if let Err(e) = window.with_webview(move |webview| {
        let controller = webview.controller();
        unsafe {
            let core = match controller.CoreWebView2() {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("取 CoreWebView2 失败（{label}）: {e}");
                    return;
                }
            };
            let settings = match core.Settings() {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("取 WebView2 设置失败（{label}）: {e}");
                    return;
                }
            };
            match settings.cast::<ICoreWebView2Settings3>() {
                Ok(s3) => {
                    if let Err(e) = s3.SetAreBrowserAcceleratorKeysEnabled(false) {
                        log::warn!("关闭浏览器快捷键失败（{label}）: {e}");
                    }
                }
                Err(e) => log::warn!("WebView2 版本过旧，无法关闭浏览器快捷键（{label}）: {e}"),
            }
        }
    }) {
        log::warn!("with_webview 调用失败（{outer_label}）: {e}");
    }
}

#[cfg(not(windows))]
fn tame_webview_accelerators(_window: &tauri::WebviewWindow) {}

/// 注册浮层与置顶槽热键。
pub fn rebind_hotkeys(app: &tauri::AppHandle, state: &Arc<AppState>) -> Result<()> {
    hotkey::unregister_all(app);
    let cfg = state.config();

    let app_for_overlay = app.clone();
    let state_for_overlay = state.clone();
    let overlay_res = hotkey::register_overlay_hotkey(app, &cfg.hotkey, move || {
        // 设置面板正在录热键：这次按键是要被录进去的，不能拿去弹浮层
        if state_for_overlay.is_recording() {
            return;
        }
        commands::show_overlay(&app_for_overlay, &state_for_overlay);
    });

    let app_for_slot = app.clone();
    let state_for_slot = state.clone();
    // 槽位热键单独注册：浮层热键被别的程序占用时，1~9 直贴不该跟着一起废掉
    let slot_res = hotkey::register_slot_hotkeys(app, &cfg.slot_hotkeys, move |slot| {
        if state_for_slot.is_recording() {
            return;
        }
        let entry = state_for_slot.store.lock().get_pinned(slot).ok().flatten();
        let Some(entry) = entry else {
            log::debug!("槽位 {slot} 是空的");
            return;
        };
        // 目标窗口 = 触发时最前面的那个窗口；但浮层开着时它自己是前台窗口，
        // 这时必须用 show_overlay 在弹出**之前**记下的那个目标，否则 Ctrl+V
        // 会打进浮层自己的搜索框（用户什么都没粘到）。
        let fg = win::foreground_window();
        let target = if fg != 0 && win::window_process_id(fg) == std::process::id() {
            let stored = state_for_slot.target_hwnd.load(Ordering::Relaxed) as isize;
            if stored != 0 && win::window_process_id(stored) != std::process::id() {
                stored
            } else {
                fg
            }
        } else {
            fg
        };
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
        if state_for_slot.config().close_after_paste {
            commands::hide_overlay_window(&app_for_slot);
        }
    });

    match (overlay_res, slot_res) {
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e),
        _ => Ok(()),
    }
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
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            use tauri::Manager as _;
            let state = app.state::<Arc<AppState>>().inner().clone();
            // 第二实例的命令行必须被尊重：之前 argv 被整个丢掉，
            // `betterPaste.exe --settings` 只会切出浮层。
            if argv.iter().any(|a| a == "--settings") {
                commands::show_window(app, SETTINGS);
            } else if argv.iter().any(|a| a == "--overlay") {
                commands::show_overlay(app, &state);
            } else {
                tray::focus_overlay(app, &state);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // 开机自启时带上这个参数：启动就不再弹浮层打扰（手动双击才弹）
            Some(vec!["--autostart"]),
        ))
        .plugin(
            tauri_plugin_log::Builder::new()
                // `.target()` 是"追加"，不指定 targets 时默认还会写 stdout，
                // 结果是每条日志出现两份。这里显式给唯一的文件 target。
                .clear_targets()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir { file_name: Some("betterPaste".into()) },
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

            // 图片预览走 asset 协议：scope 在运行时放开（数据目录是可配置的）
            if let Ok(dir) = paths::data_dir() {
                if let Err(e) = app.asset_protocol_scope().allow_directory(&dir, true) {
                    log::warn!("asset 协议目录授权失败: {e}");
                }
            }

            // 两个窗口都不要 WebView2 的浏览器快捷键（打印/刷新/查找…）
            for label in [OVERLAY, SETTINGS] {
                if let Some(w) = app.get_webview_window(label) {
                    tame_webview_accelerators(&w);
                }
            }

            // 手动启动（双击 exe）时给点反馈：直接亮出浮层 + 一句话，
            // 否则用户"双击之后什么都没发生"，只有托盘里多了一个图标。
            // 开机自启会带 --autostart，那就不打扰。
            let autostarted = std::env::args().any(|a| a == "--autostart");
            if !autostarted {
                let first_run = paths::config_path().map(|p| !p.exists()).unwrap_or(false);
                let tray_on = cfg.show_tray_icon;
                let hint = if first_run {
                    format!(
                        "第一次使用：按 {} 呼出历史，右上角 ⚙ 是设置；关掉这个窗口后 betterPaste 仍会在后台继续记录{}",
                        cfg.hotkey,
                        if tray_on { "（托盘图标可右键）" } else { "（没有托盘图标，退出在设置面板里）" }
                    )
                } else if tray_on {
                    format!(
                        "betterPaste 已在托盘里运行（右键托盘图标有菜单）· 按 {} 呼出历史",
                        cfg.hotkey
                    )
                } else {
                    format!(
                        "betterPaste 已在后台运行（不占托盘）· 按 {} 呼出历史；要退出请到设置面板",
                        cfg.hotkey
                    )
                };
                commands::set_startup_hint(hint);
                // 保护期：双击启动时刚弹出的提示别被"抢焦点"秒收掉（实测 1 秒内就没了）
                state.set_launch_guard(4000);
                commands::show_overlay(&handle, &state);
                // 启动提示只当"打过招呼"：6 秒后自动收起，避免出现
                // "浮层既没焦点又赖在屏幕上、Esc 也送不进去"的卡死状态。
                // 窗口操作必须在主线程执行（跨线程直接调 hide 不会生效）。
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(6000));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        if let Some(w) = h2.get_webview_window(OVERLAY) {
                            let _ = w.hide();
                        }
                    });
                });
            }

            // --settings / --overlay：启动后直接亮出对应的窗口
            match start {
                StartWindow::Overlay => commands::show_overlay(&handle, &state),
                StartWindow::Settings => commands::show_window(&handle, SETTINGS),
                StartWindow::None => {}
            }

            log::info!("betterPaste 已启动，数据目录：{}", paths::data_dir()?.display());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // 浮层失焦自动收起；设置窗口点关闭只是隐藏（托盘仍在）
            WindowEvent::Focused(false) => {
                if window.label() == OVERLAY {
                    if let Some(state) = window.try_state::<Arc<AppState>>() {
                        // 启动保护期内不收起：双击启动的提示条要看得见（实测会 1 秒内被收掉）
                        if state.config().hide_on_blur && !state.launch_guard_active() {
                            let _ = window.hide();
                        }
                    }
                } else if window.label() == SETTINGS {
                    // 兜底：设置窗口一旦失焦就退出"录制热键"状态，
                    // 否则全局热键会一直处于被吞掉的状态
                    if let Some(state) = window.try_state::<Arc<AppState>>() {
                        state.set_recording(false);
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
            commands::delete_entries,
            commands::pinned_slots,
            commands::copy_entry,
            commands::clear_entries,
            commands::paste_entry,
            commands::paste_slot,
            commands::paste_text,
            commands::get_config,
            commands::set_config,
            commands::quit_app,
            commands::set_paused,
            commands::get_paused,
            commands::set_recording,
            commands::data_dir,
            commands::open_data_dir,
            commands::migrate_data_dir,
            commands::reset_data_dir,
            commands::restart_app,
            commands::take_startup_hint,
            commands::open_settings,
            commands::hide_overlay,
        ])
        .run(tauri::generate_context!())
        .expect("betterPaste 启动失败");
}
