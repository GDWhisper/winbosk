//! 平台无关的快捷键与热键解析模型。
//!
//! 本模块零 Win32 / OS 依赖，仅包含动作枚举、键位绑定及字符串解析/格式化纯逻辑。

use serde::{Deserialize, Serialize};

/// 全局热键动作枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotkeyAction {
    /// 打开/关闭控制中心
    ConsoleToggle,
    /// 切换原生桌面 / 栅栏模式
    DesktopToggle,
    /// 一键整理桌面图标
    AutoOrganize,
    /// 全局紧急安全退出
    Quit,
}

impl HotkeyAction {
    /// 所有支持的热键动作列表。
    pub const ALL: [HotkeyAction; 4] = [
        HotkeyAction::ConsoleToggle,
        HotkeyAction::DesktopToggle,
        HotkeyAction::AutoOrganize,
        HotkeyAction::Quit,
    ];

    /// 动作的人类可读简短名称。
    pub fn label(&self) -> &'static str {
        match self {
            HotkeyAction::ConsoleToggle => "控制中心",
            HotkeyAction::DesktopToggle => "切换桌面",
            HotkeyAction::AutoOrganize => "一键整理",
            HotkeyAction::Quit => "紧急退出",
        }
    }

    /// 动作的详细功能描述。
    pub fn description(&self) -> &'static str {
        match self {
            HotkeyAction::ConsoleToggle => "打开或关闭控制中心",
            HotkeyAction::DesktopToggle => "在原生桌面与栅栏之间切换",
            HotkeyAction::AutoOrganize => "按栅栏规则自动收纳整理桌面图标",
            HotkeyAction::Quit => "全局安全退出 WinBosk 并恢复原生桌面",
        }
    }
}

/// 平台中立的热键绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// 规范化的大写键名，如 "T", "F10", "D", "O", "Space"
    pub key: String,
}

impl HotkeyBinding {
    /// 构造新的热键绑定，键名会自动进行规范化。
    pub fn new(ctrl: bool, alt: bool, shift: bool, win: bool, key: impl Into<String>) -> Self {
        let raw = key.into();
        let key = normalize_key(&raw).unwrap_or(raw);
        Self {
            ctrl,
            alt,
            shift,
            win,
            key,
        }
    }

    /// 解析热键字符串（如 "Ctrl+Alt+T"、"ctrl + shift + f10"），忽略大小写与多余空格。
    /// 若格式非法、缺失按键或存在多于一个按键，则返回 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut key: Option<String> = None;

        for part in trimmed.split('+') {
            let token = part.trim();
            if token.is_empty() {
                // 例如 "Ctrl++T" 或 "Ctrl+Alt+" 等非法空段
                return None;
            }

            let lower = token.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "menu" => alt = true,
                "shift" => shift = true,
                "win" | "windows" | "super" | "meta" => win = true,
                _ => {
                    if key.is_some() {
                        // 已存在非修饰键，不支持多个常规按键
                        return None;
                    }
                    let normalized = normalize_key(token)?;
                    key = Some(normalized);
                }
            }
        }

        let key = key?;
        Some(Self {
            ctrl,
            alt,
            shift,
            win,
            key,
        })
    }

    /// 输出规范的配置字符串，固定顺序：Ctrl -> Alt -> Shift -> Win -> Key。
    /// 例如 "Ctrl+Alt+T"、"Ctrl+Shift+F10"。
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// 友好显示字符串，带有空格分隔符。
    /// 例如 "Ctrl + Alt + T"、"Ctrl + Shift + F10"。
    pub fn display_string(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join(" + ")
    }
}

impl std::str::FromStr for HotkeyBinding {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

/// 键名规范化处理：统一大小写与常见按键命名。
fn normalize_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "space" => Some("Space".to_string()),
        "enter" | "return" => Some("Enter".to_string()),
        "tab" => Some("Tab".to_string()),
        "esc" | "escape" => Some("Esc".to_string()),
        "backspace" => Some("Backspace".to_string()),
        "del" | "delete" => Some("Delete".to_string()),
        "ins" | "insert" => Some("Insert".to_string()),
        "home" => Some("Home".to_string()),
        "end" => Some("End".to_string()),
        "pgup" | "pageup" => Some("PageUp".to_string()),
        "pgdn" | "pagedown" => Some("PageDown".to_string()),
        "up" => Some("Up".to_string()),
        "down" => Some("Down".to_string()),
        "left" => Some("Left".to_string()),
        "right" => Some("Right".to_string()),
        "capslock" => Some("CapsLock".to_string()),
        "numlock" => Some("NumLock".to_string()),
        "scrolllock" => Some("ScrollLock".to_string()),
        "printscreen" | "prtscn" => Some("PrintScreen".to_string()),
        "pause" => Some("Pause".to_string()),
        "plus" => Some("Plus".to_string()),
        "minus" => Some("Minus".to_string()),
        _ => {
            // F1 ~ F24 功能键
            if lower.starts_with('f') && lower.len() > 1 {
                if let Ok(num) = lower[1..].parse::<u8>() {
                    if (1..=24).contains(&num) {
                        return Some(format!("F{num}"));
                    }
                }
            }

            // 单个 ASCII 可视字符（字母大写、数字、常见符号）
            let mut chars = trimmed.chars();
            if let Some(c) = chars.next() {
                if chars.next().is_none() && c.is_ascii() && !c.is_ascii_control() {
                    return Some(c.to_ascii_uppercase().to_string());
                }
            }

            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_labels_and_descriptions() {
        for action in HotkeyAction::ALL {
            assert!(!action.label().is_empty());
            assert!(!action.description().is_empty());
        }
        assert_eq!(HotkeyAction::ConsoleToggle.label(), "控制中心");
        assert_eq!(HotkeyAction::DesktopToggle.label(), "切换桌面");
        assert_eq!(HotkeyAction::AutoOrganize.label(), "一键整理");
        assert_eq!(HotkeyAction::Quit.label(), "紧急退出");
    }

    #[test]
    fn test_parse_valid() {
        let b1 = HotkeyBinding::parse("Ctrl+Alt+T").unwrap();
        assert_eq!(
            b1,
            HotkeyBinding {
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "T".into()
            }
        );

        let b2 = HotkeyBinding::parse("ctrl + shift + f10").unwrap();
        assert_eq!(
            b2,
            HotkeyBinding {
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
                key: "F10".into()
            }
        );

        let b3 = HotkeyBinding::parse("win + alt + d").unwrap();
        assert_eq!(
            b3,
            HotkeyBinding {
                ctrl: false,
                alt: true,
                shift: false,
                win: true,
                key: "D".into()
            }
        );

        let b4 = HotkeyBinding::parse("Ctrl + Space").unwrap();
        assert_eq!(
            b4,
            HotkeyBinding {
                ctrl: true,
                alt: false,
                shift: false,
                win: false,
                key: "Space".into()
            }
        );

        let b5 = HotkeyBinding::parse("F1").unwrap();
        assert_eq!(
            b5,
            HotkeyBinding {
                ctrl: false,
                alt: false,
                shift: false,
                win: false,
                key: "F1".into()
            }
        );

        let b6 = HotkeyBinding::parse("cTrL + aLt + sHiFt + wIn + o").unwrap();
        assert_eq!(
            b6,
            HotkeyBinding {
                ctrl: true,
                alt: true,
                shift: true,
                win: true,
                key: "O".into()
            }
        );

        let b7 = HotkeyBinding::parse("control + menu + escape").unwrap();
        assert_eq!(
            b7,
            HotkeyBinding {
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "Esc".into()
            }
        );
    }

    #[test]
    fn test_parse_invalid() {
        assert_eq!(HotkeyBinding::parse(""), None);
        assert_eq!(HotkeyBinding::parse("   "), None);
        assert_eq!(HotkeyBinding::parse("Ctrl"), None);
        assert_eq!(HotkeyBinding::parse("Ctrl+Alt"), None);
        assert_eq!(HotkeyBinding::parse("Ctrl++T"), None);
        assert_eq!(HotkeyBinding::parse("Ctrl+Alt+"), None);
        assert_eq!(HotkeyBinding::parse("+Ctrl+T"), None);
        assert_eq!(HotkeyBinding::parse("Ctrl+Alt+T+D"), None);
        assert_eq!(HotkeyBinding::parse("Ctrl+Alt+InvalidKey123"), None);
    }

    #[test]
    fn test_to_string_fixed_order() {
        // 乱序输入修饰键，输出必须固定为 Ctrl -> Alt -> Shift -> Win -> Key
        let b = HotkeyBinding::parse("Win + Shift + Alt + Ctrl + D").unwrap();
        assert_eq!(b.to_string(), "Ctrl+Alt+Shift+Win+D");
        assert_eq!(b.display_string(), "Ctrl + Alt + Shift + Win + D");

        let b2 = HotkeyBinding::parse("alt + ctrl + t").unwrap();
        assert_eq!(b2.to_string(), "Ctrl+Alt+T");
        assert_eq!(b2.display_string(), "Ctrl + Alt + T");
    }

    #[test]
    fn test_display_string() {
        let b = HotkeyBinding::parse("ctrl + shift + f10").unwrap();
        assert_eq!(b.display_string(), "Ctrl + Shift + F10");

        let b_single = HotkeyBinding::parse("space").unwrap();
        assert_eq!(b_single.display_string(), "Space");
        assert_eq!(b_single.to_string(), "Space");
    }

    #[test]
    fn test_serde_roundtrip() {
        let b = HotkeyBinding::parse("Ctrl+Alt+T").unwrap();
        let json = serde_json::to_string(&b).unwrap();
        let de: HotkeyBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(b, de);

        let action = HotkeyAction::ConsoleToggle;
        let action_json = serde_json::to_string(&action).unwrap();
        let action_de: HotkeyAction = serde_json::from_str(&action_json).unwrap();
        assert_eq!(action, action_de);
    }

    #[test]
    fn test_to_string_and_from_str() {
        let b = HotkeyBinding::parse("Ctrl+Alt+T").unwrap();
        assert_eq!(b.to_string(), "Ctrl+Alt+T");

        let parsed: HotkeyBinding = "Ctrl+Alt+T".parse().unwrap();
        assert_eq!(b, parsed);

        assert!("invalid".parse::<HotkeyBinding>().is_err());
    }
}
