/**
 * The IPC contract with the Rust side. Keep this in sync with
 * `src-tauri/src/commands.rs` — every function here maps to one `#[tauri::command]`.
 */
import { invoke } from "@tauri-apps/api/core";

export type ClipKind = "text" | "image";

export interface ClipEntry {
  id: string;
  kind: ClipKind;
  /** Text payload; `null` for image entries. */
  content: string | null;
  /** Absolute path of the full-size PNG (images only). */
  imagePath: string | null;
  /** Absolute path of the thumbnail PNG (images only). */
  thumbPath: string | null;
  width: number | null;
  height: number | null;
  bytes: number;
  /** 0-9 when pinned to a slot, otherwise `null`. */
  pinned: number | null;
  sourceApp: string | null;
  createdAt: number;
}

export type ThemeMode = "system" | "light" | "dark";

export interface AppConfig {
  maxEntries: number;
  maxAgeDays: number;
  maxItemMb: number;
  historyLimit: number;
  captureText: boolean;
  captureImages: boolean;
  hotkey: string;
  slotHotkeys: string[];
  theme: ThemeMode;
  closeAfterPaste: boolean;
  hideOnBlur: boolean;
  autostart: boolean;
  ignoreSourceApps: string[];
}

export type PasteResult = "pasted" | "manualFallback";

/* ------------------------------------------------------------------ entries */

export const listEntries = (limit: number) => invoke<ClipEntry[]>("list_entries", { limit });

export const searchEntries = (query: string, limit: number) =>
  invoke<ClipEntry[]>("search_entries", { query, limit });

export const pinEntry = (id: string, slot: number | null) =>
  invoke<void>("pin_entry", { id, slot });

export const deleteEntry = (id: string) => invoke<void>("delete_entry", { id });

/** 批量删除（浮层多选删除用），返回真正删掉的条数。 */
export const deleteEntries = (ids: string[]) => invoke<number>("delete_entries", { ids });

/** 槽位 1~9 当前置顶的条目（没有就是 null）。 */
export const pinnedSlots = () => invoke<(ClipEntry | null)[]>("pinned_slots");

export const copyEntry = (id: string) => invoke<void>("copy_entry", { id });

export const clearEntries = (keepPinned = false) =>
  invoke<number>("clear_entries", { keepPinned });

/* ------------------------------------------------------------------- paste */

export const pasteEntry = (id: string) => invoke<PasteResult>("paste_entry", { id });

export const pasteSlot = (slot: number) => invoke<PasteResult>("paste_slot", { slot });

export const pasteText = (text: string) => invoke<PasteResult>("paste_text", { text });

/* ------------------------------------------------------------------ config */

export const getConfig = () => invoke<AppConfig>("get_config");

export const setConfig = (config: AppConfig) => invoke<AppConfig>("set_config", { config });

/* --------------------------------------------------------------- app status */

export const setPaused = (paused: boolean) => invoke<boolean>("set_paused", { paused });

export const getPaused = () => invoke<boolean>("get_paused");

/** 开始/结束录制热键：录制期间全局热键不抢按键（否则录 Ctrl+Shift+V 会被浮层截胡）。 */
export const setRecording = (recording: boolean) => invoke<boolean>("set_recording", { recording });

export const openSettings = () => invoke<void>("open_settings");

export const hideOverlay = () => invoke<void>("hide_overlay");

export const dataDir = () => invoke<string>("data_dir");

/** 在资源管理器里打开数据目录。 */
export const openDataDir = () => invoke<void>("open_data_dir");

/** 把现有数据（库 + 图片 + 配置）复制到新目录，并记下指针；重启后生效。 */
export const migrateDataDir = (target: string) => invoke<string>("migrate_data_dir", { target });

/** 恢复默认数据目录（同样是重启后生效）。 */
export const resetDataDir = () => invoke<string>("reset_data_dir");

/** 延迟重启自己（用于让数据目录变更生效）。 */
export const restartApp = () => invoke<void>("restart_app");

/** 启动提示（首次运行 / 托盘"使用提示"），取一次就清空。 */
export const takeStartupHint = () => invoke<string | null>("take_startup_hint");

/** 后端配置变更（主题等）推送。 */
export const EVENT_CONFIG_CHANGED = "clipstack://config";

/* ----------------------------------------------------------------- helpers */

export const isImage = (e: ClipEntry) => e.kind === "image";

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

export function formatTime(ms: number): string {
  const d = new Date(ms);
  const now = Date.now();
  const diff = now - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
