use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use tauri::{AppHandle, Emitter};

use crate::config::AppConfig;
use crate::store::Store;

pub const OVERLAY: &str = "overlay";
pub const SETTINGS: &str = "settings";

/// 前端收到这个事件就去重新拉一次列表。
pub const EVENT_CHANGED: &str = "clipstack://changed";
/// 前端收到这个事件就同步暂停状态。
pub const EVENT_PAUSED: &str = "clipstack://paused";
/// 配置变了（主题等），浮层收到要重新套用主题。
pub const EVENT_CONFIG: &str = "clipstack://config";

pub struct AppState {
    pub config: RwLock<AppConfig>,
    pub store: Mutex<Store>,
    /// true = 已暂停记录
    pub paused: AtomicBool,
    /// 浮层弹出前的前台窗口，粘贴时要还焦给它
    pub target_hwnd: AtomicI64,
    /// 我们自己写进剪贴板时产生的序号，避免重复入库
    pub suppress_seq: AtomicU32,
    pub shutdown: AtomicBool,
    /// 由 `bind_app` 在 setup 阶段填入；watcher 线程也要用它发事件。
    app: OnceLock<AppHandle>,
}

impl AppState {
    pub fn new(config: AppConfig, store: Store) -> Arc<Self> {
        Arc::new(Self {
            config: RwLock::new(config),
            store: Mutex::new(store),
            paused: AtomicBool::new(false),
            target_hwnd: AtomicI64::new(0),
            suppress_seq: AtomicU32::new(0),
            shutdown: AtomicBool::new(false),
            app: OnceLock::new(),
        })
    }

    pub fn bind_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    pub fn app(&self) -> Option<&AppHandle> {
        self.app.get()
    }

    pub fn config(&self) -> AppConfig {
        self.config.read().clone()
    }

    pub fn update_config(&self, cfg: AppConfig) {
        *self.config.write() = cfg;
    }

    // ------------------------------------------------------------ 录制开关

    pub fn is_capturing(&self) -> bool {
        !self.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
        if let Some(app) = self.app() {
            crate::tray::sync_pause_state(app, paused);
        }
        self.emit(EVENT_PAUSED, paused);
    }

    pub fn toggle_paused(&self) -> bool {
        let next = !self.paused.load(Ordering::Relaxed);
        self.set_paused(next);
        next
    }

    fn emit<S: serde::Serialize + Clone>(&self, event: &str, payload: S) {
        if let Some(app) = self.app() {
            let _ = app.emit(event, payload);
        }
    }

    // ------------------------------------------------------------ 变更通知

    /// 入库之后：跑一次清理，并通知前端刷新。
    pub fn after_write(&self) {
        let cfg = self.config();
        {
            let mut store = self.store.lock();
            if let Err(e) = store.cleanup(&cfg) {
                log::warn!("清理失败: {e}");
            }
        }
        self.notify_changed();
    }

    pub fn notify_changed(&self) {
        self.emit(EVENT_CHANGED, ());
    }

    /// 配置变更后通知所有窗口（浮层要重新套用主题）。
    pub fn notify_config(&self) {
        self.emit(EVENT_CONFIG, ());
    }
}
