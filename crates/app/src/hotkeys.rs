//! 全局热键映射管理与 Win32 热键注册。
//!
//! 负责把平台无关的 `HotkeyBinding` / `HotkeyAction` 转换为 Windows API
//! 所需的修饰键标志与虚拟键码（VK），并统一管理热键注册与冲突状态。

use winbosk_core::hotkey::{HotkeyAction, HotkeyBinding};
use winbosk_render::{HOTKEY_AUTO_ORGANIZE, HOTKEY_CONSOLE, HOTKEY_DESKTOP, HOTKEY_QUIT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

use crate::Runtime;

/// 将平台无关的热键绑定转换为 Win32 `(modifiers, vk)`。
///
/// 修饰键包含 `MOD_NOREPEAT`，防止长按时系统连发 `WM_HOTKEY`。
pub fn binding_to_win32(binding: &HotkeyBinding) -> Option<(u32, u32)> {
    let mut mods = MOD_NOREPEAT.0;
    if binding.ctrl {
        mods |= MOD_CONTROL.0;
    }
    if binding.alt {
        mods |= MOD_ALT.0;
    }
    if binding.shift {
        mods |= MOD_SHIFT.0;
    }
    if binding.win {
        mods |= MOD_WIN.0;
    }

    let vk = key_name_to_vk(&binding.key)?;
    Some((mods, vk))
}

/// 将键名转换为 Win32 虚拟键码（VK）。
pub fn key_name_to_vk(key: &str) -> Option<u32> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        // 控制键与导航键
        "space" => Some(0x20),                  // VK_SPACE
        "tab" => Some(0x09),                    // VK_TAB
        "enter" | "return" => Some(0x0D),       // VK_RETURN
        "esc" | "escape" => Some(0x1B),         // VK_ESCAPE
        "backspace" => Some(0x08),              // VK_BACK
        "del" | "delete" => Some(0x2E),         // VK_DELETE
        "ins" | "insert" => Some(0x2D),         // VK_INSERT
        "home" => Some(0x24),                   // VK_HOME
        "end" => Some(0x23),                    // VK_END
        "pgup" | "pageup" => Some(0x21),        // VK_PRIOR
        "pgdn" | "pagedown" => Some(0x22),      // VK_NEXT
        "left" => Some(0x25),                   // VK_LEFT
        "up" => Some(0x26),                     // VK_UP
        "right" => Some(0x27),                  // VK_RIGHT
        "down" => Some(0x28),                   // VK_DOWN
        "capslock" => Some(0x14),               // VK_CAPITAL
        "numlock" => Some(0x90),                // VK_NUMLOCK
        "scrolllock" => Some(0x91),             // VK_SCROLL
        "printscreen" | "prtscn" => Some(0x2C), // VK_SNAPSHOT
        "pause" => Some(0x13),                  // VK_PAUSE

        // 常用符号键
        "`" | "~" => Some(0xC0),           // VK_OEM_3
        "-" | "_" | "minus" => Some(0xBD), // VK_OEM_MINUS
        "=" | "+" | "plus" => Some(0xBB),  // VK_OEM_PLUS
        "[" | "{" => Some(0xDB),           // VK_OEM_4
        "]" | "}" => Some(0xDD),           // VK_OEM_6
        "\\" | "|" => Some(0xDC),          // VK_OEM_5
        ";" | ":" => Some(0xBA),           // VK_OEM_1
        "'" | "\"" => Some(0xDE),          // VK_OEM_7
        "," | "<" => Some(0xBC),           // VK_OEM_COMMA
        "." | ">" => Some(0xBE),           // VK_OEM_PERIOD
        "/" | "?" => Some(0xBF),           // VK_OEM_2

        _ => {
            // F1 ~ F24 功能键
            if lower.starts_with('f') && lower.len() > 1 {
                if let Ok(num) = lower[1..].parse::<u8>() {
                    if (1..=24).contains(&num) {
                        return Some(0x70 + (num as u32 - 1)); // VK_F1 = 0x70
                    }
                }
            }

            // 单个字符：A..=Z, 0..=9
            let mut chars = trimmed.chars();
            if let Some(c) = chars.next() {
                if chars.next().is_none() {
                    if c.is_ascii_alphabetic() {
                        return Some(c.to_ascii_uppercase() as u32);
                    }
                    if c.is_ascii_digit() {
                        return Some(c as u32);
                    }
                }
            }

            None
        }
    }
}

/// 将键盘消息中的虚拟键码（VK）转换为规范化键名字符串。
pub fn vk_to_key_name(vk: u32) -> Option<&'static str> {
    match vk {
        // 字母 'A'..='Z' (0x41 ..= 0x5A)
        0x41 => Some("A"),
        0x42 => Some("B"),
        0x43 => Some("C"),
        0x44 => Some("D"),
        0x45 => Some("E"),
        0x46 => Some("F"),
        0x47 => Some("G"),
        0x48 => Some("H"),
        0x49 => Some("I"),
        0x4A => Some("J"),
        0x4B => Some("K"),
        0x4C => Some("L"),
        0x4D => Some("M"),
        0x4E => Some("N"),
        0x4F => Some("O"),
        0x50 => Some("P"),
        0x51 => Some("Q"),
        0x52 => Some("R"),
        0x53 => Some("S"),
        0x54 => Some("T"),
        0x55 => Some("U"),
        0x56 => Some("V"),
        0x57 => Some("W"),
        0x58 => Some("X"),
        0x59 => Some("Y"),
        0x5A => Some("Z"),

        // 数字 '0'..='9' (0x30 ..= 0x39)
        0x30 => Some("0"),
        0x31 => Some("1"),
        0x32 => Some("2"),
        0x33 => Some("3"),
        0x34 => Some("4"),
        0x35 => Some("5"),
        0x36 => Some("6"),
        0x37 => Some("7"),
        0x38 => Some("8"),
        0x39 => Some("9"),

        // 小键盘数字 0x60..=0x69 统一映射为 "0".."9"
        0x60 => Some("0"),
        0x61 => Some("1"),
        0x62 => Some("2"),
        0x63 => Some("3"),
        0x64 => Some("4"),
        0x65 => Some("5"),
        0x66 => Some("6"),
        0x67 => Some("7"),
        0x68 => Some("8"),
        0x69 => Some("9"),
        0x6A => Some("*"),
        0x6B => Some("+"),
        0x6D => Some("-"),
        0x6E => Some("."),
        0x6F => Some("/"),

        // 功能键 F1..=F24 (0x70 ..= 0x87)
        0x70 => Some("F1"),
        0x71 => Some("F2"),
        0x72 => Some("F3"),
        0x73 => Some("F4"),
        0x74 => Some("F5"),
        0x75 => Some("F6"),
        0x76 => Some("F7"),
        0x77 => Some("F8"),
        0x78 => Some("F9"),
        0x79 => Some("F10"),
        0x7A => Some("F11"),
        0x7B => Some("F12"),
        0x7C => Some("F13"),
        0x7D => Some("F14"),
        0x7E => Some("F15"),
        0x7F => Some("F16"),
        0x80 => Some("F17"),
        0x81 => Some("F18"),
        0x82 => Some("F19"),
        0x83 => Some("F20"),
        0x84 => Some("F21"),
        0x85 => Some("F22"),
        0x86 => Some("F23"),
        0x87 => Some("F24"),

        // 控制与编辑键
        0x20 => Some("Space"),
        0x09 => Some("Tab"),
        0x0D => Some("Enter"),
        0x1B => Some("Esc"),
        0x08 => Some("Backspace"),
        0x2E => Some("Delete"),
        0x2D => Some("Insert"),
        0x24 => Some("Home"),
        0x23 => Some("End"),
        0x21 => Some("PageUp"),
        0x22 => Some("PageDown"),
        0x25 => Some("Left"),
        0x26 => Some("Up"),
        0x27 => Some("Right"),
        0x28 => Some("Down"),

        // 常用 OEM 符号键
        0xC0 => Some("`"),
        0xBD => Some("-"),
        0xBB => Some("="),
        0xDB => Some("["),
        0xDD => Some("]"),
        0xDC => Some("\\"),
        0xBA => Some(";"),
        0xDE => Some("'"),
        0xBC => Some(","),
        0xBE => Some("."),
        0xBF => Some("/"),

        _ => None,
    }
}

/// 将热键动作映射为唯一的 Win32 热键标识 ID。
pub fn action_to_hotkey_id(action: HotkeyAction) -> i32 {
    match action {
        HotkeyAction::Quit => HOTKEY_QUIT,
        HotkeyAction::ConsoleToggle => HOTKEY_CONSOLE,
        HotkeyAction::DesktopToggle => HOTKEY_DESKTOP,
        HotkeyAction::AutoOrganize => HOTKEY_AUTO_ORGANIZE,
    }
}

/// 遍历所有热键动作并应用到底层 Overlay 窗口。
///
/// 成功注册的热键会清除冲突提示；若因系统快捷键或第三方软件占用而失败，
/// 会在 `rt.hotkey_conflicts` 中记录友好提示。
pub fn apply_all_hotkeys(rt: &mut Runtime) {
    for action in HotkeyAction::ALL {
        let id = action_to_hotkey_id(action);
        let binding_str = rt.desk.settings.hotkeys.get_action(action);

        match binding_str {
            Some(s) => match HotkeyBinding::parse(s) {
                Some(binding) => match binding_to_win32(&binding) {
                    Some((mods, vk)) => {
                        let _ = rt.overlay().unregister_hotkey_raw(id);
                        match rt.overlay().register_hotkey_raw(id, mods, vk) {
                            Ok(()) => {
                                rt.hotkey_conflicts.remove(&action);
                            }
                            Err(e) => {
                                tracing::warn!(?action, ?binding, "注册热键失败: {e}");
                                rt.hotkey_conflicts
                                    .insert(action, "热键已被系统或其它程序占用".to_string());
                            }
                        }
                    }
                    None => {
                        let _ = rt.overlay().unregister_hotkey_raw(id);
                        rt.hotkey_conflicts
                            .insert(action, "热键包含不支持的按键".to_string());
                    }
                },
                None => {
                    let _ = rt.overlay().unregister_hotkey_raw(id);
                    rt.hotkey_conflicts
                        .insert(action, "热键格式无效".to_string());
                }
            },
            None => {
                let _ = rt.overlay().unregister_hotkey_raw(id);
                rt.hotkey_conflicts.remove(&action);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_to_hotkey_id() {
        assert_eq!(action_to_hotkey_id(HotkeyAction::Quit), 1);
        assert_eq!(action_to_hotkey_id(HotkeyAction::ConsoleToggle), 2);
        assert_eq!(action_to_hotkey_id(HotkeyAction::DesktopToggle), 3);
        assert_eq!(action_to_hotkey_id(HotkeyAction::AutoOrganize), 4);
    }

    #[test]
    fn test_binding_to_win32() {
        let b1 = HotkeyBinding::new(true, true, false, false, "T");
        let (mods, vk) = binding_to_win32(&b1).unwrap();
        assert_eq!(mods & MOD_CONTROL.0, MOD_CONTROL.0);
        assert_eq!(mods & MOD_ALT.0, MOD_ALT.0);
        assert_eq!(mods & MOD_SHIFT.0, 0);
        assert_eq!(mods & MOD_WIN.0, 0);
        assert_eq!(mods & MOD_NOREPEAT.0, MOD_NOREPEAT.0);
        assert_eq!(vk, 0x54);

        let b2 = HotkeyBinding::new(true, false, true, false, "F10");
        let (mods, vk) = binding_to_win32(&b2).unwrap();
        assert_eq!(mods & MOD_CONTROL.0, MOD_CONTROL.0);
        assert_eq!(mods & MOD_SHIFT.0, MOD_SHIFT.0);
        assert_eq!(vk, 0x79); // VK_F10 = 0x79

        let b3 = HotkeyBinding::new(false, false, false, true, "D");
        let (mods, vk) = binding_to_win32(&b3).unwrap();
        assert_eq!(mods & MOD_WIN.0, MOD_WIN.0);
        assert_eq!(vk, 0x44);
    }

    #[test]
    fn test_vk_to_key_name_and_roundtrip() {
        assert_eq!(vk_to_key_name(0x54), Some("T"));
        assert_eq!(vk_to_key_name(0x79), Some("F10"));
        assert_eq!(vk_to_key_name(0x20), Some("Space"));
        assert_eq!(vk_to_key_name(0x0D), Some("Enter"));
        assert_eq!(vk_to_key_name(0x1B), Some("Esc"));
        assert_eq!(vk_to_key_name(0x2E), Some("Delete"));
        assert_eq!(vk_to_key_name(0xBA), Some(";"));
        assert_eq!(vk_to_key_name(0xC0), Some("`"));
        assert_eq!(vk_to_key_name(0x9999), None);

        // 验证往返互认
        let test_vks = [
            0x41, 0x5A, 0x30, 0x39, 0x70, 0x79, 0x20, 0x0D, 0x1B, 0xBA, 0xBD,
        ];
        for vk in test_vks {
            let name = vk_to_key_name(vk).unwrap();
            let mapped_vk = key_name_to_vk(name).unwrap();
            assert_eq!(mapped_vk, vk);
        }
    }
}
