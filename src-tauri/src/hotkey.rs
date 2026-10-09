//! Global hotkeys: parse the user's `"Ctrl+Shift+V"` style strings, register
//! them with the global-shortcut plugin, and expose rebinding.
//!
//! Internally a hotkey is `(modifiers, Code)` where `Code` comes from
//! `keyboard-types` (re-exported by the plugin) and is named after the W3C
//! `KeyboardEvent.code` value — the same vocabulary the settings UI records.

use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub code: Code,
}

impl Hotkey {
    pub fn canonical(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.super_key {
            parts.push("Super".into());
        }
        parts.push(friendly_code(self.code));
        parts.join("+")
    }
}

impl std::fmt::Display for Hotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Parse `"Ctrl+Shift+V"`, `"Alt+Space"`, `"Ctrl+Shift+1"`, `"Ctrl+Alt+F4"`, …
pub fn parse(s: &str) -> Result<Hotkey> {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut super_key = false;
    let mut main: Option<&str> = None;

    for raw in s.split('+') {
        let tok = raw.trim();
        if tok.is_empty() {
            continue;
        }
        match tok.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "ctl" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "super" | "win" | "meta" | "cmd" => super_key = true,
            _ => {
                if main.is_some() {
                    return Err(Error::Hotkey(format!("热键里出现了多个主键：{s}")));
                }
                main = Some(tok);
            }
        }
    }

    let main = main.ok_or_else(|| Error::Hotkey(format!("热键缺少主键：{s}")))?;
    if !(ctrl || alt || shift || super_key) {
        return Err(Error::Hotkey(format!("热键至少要带一个修饰键：{s}")));
    }

    Ok(Hotkey { ctrl, alt, shift, super_key, code: parse_code(main)? })
}

/// 规范化用户输入；解析失败就原样返回。
pub fn normalize(s: &str) -> String {
    parse(s).map(|h| h.canonical()).unwrap_or_else(|_| s.to_string())
}

/// 「注册了会把系统常用操作抢走」的组合。
///
/// 全局热键是抢占式的：把呼出浮层设成 `Ctrl+V` 之后，系统里任何地方按 Ctrl+V 都只会
/// 弹出 betterPaste，粘贴彻底失效（想粘回自己刚复制的历史条目都不行）。这类组合要么
/// 拒绝，要么至少明确警告。
pub fn danger_reason(raw: &str) -> Option<String> {
    let canonical = normalize(raw);
    let mut parts: Vec<&str> = canonical.split('+').map(|p| p.trim()).collect();
    let key = parts.pop()?.to_ascii_uppercase();
    let mods: Vec<String> = parts.iter().map(|m| m.to_ascii_lowercase()).collect();
    let only_ctrl = mods.iter().any(|m| m == "ctrl") && mods.len() == 1;
    if only_ctrl && matches!(key.as_str(), "V" | "C" | "X" | "Z" | "A" | "S" | "P" | "F") {
        return Some(format!(
            "Ctrl+{key} 是系统最常用的快捷键之一，注册后它会被浮层抢走（例如 Ctrl+V 再也粘贴不了东西），请换一个组合"
        ));
    }
    if canonical == "Ctrl+Shift+C" {
        return Some("Ctrl+Shift+C 在很多程序里是「复制并格式化」，建议换一个组合".into());
    }
    None
}

/// 接受 `V` / `v` / `KeyV` / `1` / `Digit1` / `Space` / `space` / `F4` / `PageUp`。
///
/// 注意：`keyboard-types::Code::from_str` 只认它自己的 CamelCase 全名
/// （`"Space"`、`"PageUp"`、`"KeyV"`…），全大写化会直接匹配失败，所以这里必须
/// 保留大小写，只做"首字母大写"这种宽容处理。
fn parse_code(s: &str) -> Result<Code> {
    let t = s.trim();
    if t.is_empty() {
        return Err(Error::Hotkey("热键缺少主键".into()));
    }

    // 单字符：V → KeyV，1 → Digit1
    if t.chars().count() == 1 {
        let c = t.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            return Code::from_str(&format!("Key{}", c.to_ascii_uppercase()))
                .map_err(|_| Error::Hotkey(format!("无法识别的主键：{s}")));
        }
        if c.is_ascii_digit() {
            return Code::from_str(&format!("Digit{c}"))
                .map_err(|_| Error::Hotkey(format!("无法识别的主键：{s}")));
        }
    }

    let candidates = [
        t.to_string(),                                     // 原样：Space / PageUp / KeyV / F4
        capitalize(t),                                     // space → Space，pageup → Pageup
        format!("Key{}", capitalize(t)),                   // v → KeyV
        format!("Digit{}", t.trim_start_matches("Digit")), // 数字兜底
        t.to_ascii_uppercase(),                            // f4 → F4
    ];
    for cand in candidates {
        if let Ok(code) = Code::from_str(&cand) {
            return Ok(code);
        }
    }
    Err(Error::Hotkey(format!("无法识别的主键：{s}")))
}

/// 首字母大写、其余保持原样：`space` → `Space`。
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// 回显给用户看的主键名：`KeyV` → `V`，`Digit1` → `1`，`Space` → `Space`。
fn friendly_code(code: Code) -> String {
    let raw = code.to_string();
    if let Some(rest) = raw.strip_prefix("Key") {
        return rest.to_string();
    }
    if let Some(rest) = raw.strip_prefix("Digit") {
        return rest.to_string();
    }
    raw
}

fn modifiers_of(hk: &Hotkey) -> Modifiers {
    let mut m = Modifiers::empty();
    if hk.ctrl {
        m |= Modifiers::CONTROL;
    }
    if hk.alt {
        m |= Modifiers::ALT;
    }
    if hk.shift {
        m |= Modifiers::SHIFT;
    }
    if hk.super_key {
        m |= Modifiers::SUPER;
    }
    m
}

fn plugin_shortcut(hk: &Hotkey) -> Shortcut {
    Shortcut::new(Some(modifiers_of(hk)), hk.code)
}

/// 注册浮层热键。回调在热键按下时触发。
/// 返回 `Err` 表示热键被别的程序占用了。
pub fn register_overlay_hotkey<F>(app: &AppHandle, hotkey: &str, handler: F) -> Result<()>
where
    F: Fn() + Send + Sync + 'static,
{
    let hk = parse(hotkey)?;
    let shortcut = plugin_shortcut(&hk);
    let gs = app.global_shortcut();

    gs.on_shortcut(shortcut, move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            handler();
        }
    })
    .map_err(|e| Error::Hotkey(format!("热键 {hotkey} 注册失败（可能被其他程序占用）：{e}")))
}

/// 注册置顶槽直贴热键 `Ctrl+Shift+1..9`。
///
/// 单个槽位失败（录成了重复组合、被别的程序占用）**不能**影响其它槽位：旧实现里
/// 循环体用了 `?`，一个冲突就让后面所有槽位都不再注册，而界面上只显示"已保存"。
pub fn register_slot_hotkeys<F>(app: &AppHandle, hotkeys: &[String], handler: F) -> Result<()>
where
    F: Fn(u8) + Send + Sync + 'static,
{
    let gs = app.global_shortcut();
    let handler = std::sync::Arc::new(handler);
    let mut failed: Vec<String> = Vec::new();
    for (i, raw) in hotkeys.iter().enumerate() {
        let slot = (i + 1) as u8;
        if slot > 9 {
            // 只支持 1~9；多余的表项会注册出"抢了键但永远取不到内容"的死热键
            failed.push(format!("槽位 {slot}（{raw}）超出 1~9，已忽略"));
            continue;
        }
        let hk = match parse(raw) {
            Ok(h) => h,
            Err(e) => {
                failed.push(format!("槽位 {slot}（{raw}）无效：{e}"));
                continue;
            }
        };
        let handler = handler.clone();
        let res = gs.on_shortcut(plugin_shortcut(&hk), move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                handler(slot);
            }
        });
        if let Err(e) = res {
            failed.push(format!("槽位 {slot}（{raw}）注册失败：{e}"));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::Hotkey(failed.join("；")))
    }
}

/// 清空所有已注册热键（换绑时先调它）。
pub fn unregister_all(app: &AppHandle) {
    if let Err(e) = app.global_shortcut().unregister_all() {
        log::warn!("注销热键失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_hotkey() {
        let hk = parse("Ctrl+Shift+V").unwrap();
        assert!(hk.ctrl && hk.shift && !hk.alt && !hk.super_key);
        assert_eq!(hk.code, Code::KeyV);
        assert_eq!(hk.canonical(), "Ctrl+Shift+V");
    }

    #[test]
    fn parses_digit_slots() {
        let expected = [
            Code::Digit1,
            Code::Digit2,
            Code::Digit3,
            Code::Digit4,
            Code::Digit5,
            Code::Digit6,
            Code::Digit7,
            Code::Digit8,
            Code::Digit9,
        ];
        for (i, code) in expected.iter().enumerate() {
            let hk = parse(&format!("Ctrl+Shift+{}", i + 1)).unwrap();
            assert_eq!(hk.code, *code);
            assert_eq!(hk.canonical(), format!("Ctrl+Shift+{}", i + 1));
        }
    }

    #[test]
    fn normalizes_aliases_and_case() {
        assert_eq!(normalize("control + shift + v"), "Ctrl+Shift+V");
        assert_eq!(normalize("Win+Space"), "Super+Space");
        assert_eq!(normalize("ALT+F4"), "Alt+F4");
        assert_eq!(normalize("Ctrl+Alt+KeyP"), "Ctrl+Alt+P");
    }

    #[test]
    fn rejects_bare_keys_and_bad_input() {
        assert!(parse("V").is_err(), "无修饰键必须被拒绝");
        assert!(parse("Ctrl+Shift+V+B").is_err());
        assert!(parse("Ctrl+Shift+").is_err());
        assert!(parse("Ctrl+Shift+NotAKey").is_err());
    }

    #[test]
    fn friendly_names_round_trip() {
        for k in ["V", "1", "F4", "Space", "Escape", "Backspace", "PageUp"] {
            let s = format!("Ctrl+{k}");
            let hk = parse(&s).unwrap();
            assert_eq!(hk.canonical(), s, "round trip failed for {s}");
        }
    }
}
