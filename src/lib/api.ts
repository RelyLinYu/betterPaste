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

export const openSettings = () => invoke<void>("open_settings");

export const hideOverlay = () => invoke<void>("hide_overlay");

export const dataDir = () => invoke<string>("data_dir");

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
