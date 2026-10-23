import { getConfig } from "./api";
import type { ThemeMode } from "./api";

/** Applies the user's theme choice to <html data-theme>. */
export async function applyTheme(): Promise<ThemeMode> {
  let mode: ThemeMode = "system";
  try {
    const cfg = await getConfig();
    mode = cfg.theme;
  } catch {
    /* not running inside Tauri (e.g. plain `vite dev`) — stay on system */
  }
  if (mode === "system") {
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", mode);
  }
  return mode;
}
