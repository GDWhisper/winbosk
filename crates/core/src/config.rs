//! 应用级设置与持久化。
//!
//! 数据目录由上层注入（Windows 上为 `%APPDATA%\WinBosk`），
//! 核心层不感知平台路径，保持可测。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::hotkey::{HotkeyAction, HotkeyBinding};
use crate::model::Desk;

/// 全局设置，随 `Desk` 一起持久化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// 是否显示未分组图标区。
    pub show_free_area: bool,
    /// 未分组图标区高度（逻辑 px）。
    pub free_area_height: f32,
    pub hotkeys: HotkeyConfig,
    /// 是否开机自启。
    #[serde(default)]
    pub autostart: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_free_area: true,
            free_area_height: 90.0,
            hotkeys: HotkeyConfig::default(),
            autostart: false,
        }
    }
}

/// 全局热键配置。值为可解析的键位描述（如 `"Ctrl+Alt+T"`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// 打开/关闭控制中心（默认 "Ctrl+Alt+T"）
    pub console_toggle: Option<String>,
    /// 切换原生桌面 / 栅栏（默认 "Ctrl+Alt+D"）
    pub desktop_toggle: Option<String>,
    /// 一键整理桌面（默认 None）
    pub auto_organize: Option<String>,
    /// 全局紧急安全退出（默认 "Ctrl+Shift+F10"）
    pub quit: Option<String>,

    // 兼容旧配置中的字段，反序列化时忽略不报错
    #[serde(default, skip_serializing)]
    pub collapse_all: Option<String>,
    #[serde(default, skip_serializing)]
    pub expand_all: Option<String>,
    #[serde(default, skip_serializing)]
    pub search: Option<String>,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            console_toggle: Some("Ctrl+Alt+T".to_string()),
            desktop_toggle: Some("Ctrl+Alt+D".to_string()),
            auto_organize: None,
            quit: Some("Ctrl+Shift+F10".to_string()),
            collapse_all: None,
            expand_all: None,
            search: None,
        }
    }
}

impl HotkeyConfig {
    /// 获取指定热键动作对应的键位字符串引用。
    pub fn get_action(&self, action: HotkeyAction) -> Option<&str> {
        match action {
            HotkeyAction::ConsoleToggle => self.console_toggle.as_deref(),
            HotkeyAction::DesktopToggle => self.desktop_toggle.as_deref(),
            HotkeyAction::AutoOrganize => self.auto_organize.as_deref(),
            HotkeyAction::Quit => self.quit.as_deref(),
        }
    }

    /// 设置指定热键动作的键位字符串。
    pub fn set_action(&mut self, action: HotkeyAction, val: Option<String>) {
        match action {
            HotkeyAction::ConsoleToggle => self.console_toggle = val,
            HotkeyAction::DesktopToggle => self.desktop_toggle = val,
            HotkeyAction::AutoOrganize => self.auto_organize = val,
            HotkeyAction::Quit => self.quit = val,
        }
    }

    /// 获取指定动作已解析的热键绑定模型。
    pub fn get_binding(&self, action: HotkeyAction) -> Option<HotkeyBinding> {
        self.get_action(action).and_then(HotkeyBinding::parse)
    }
}

/// 配置读写器：负责 `Desk` 的加载、保存与版本迁移。
pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn desk_path(&self) -> PathBuf {
        self.dir.join("desk.json")
    }

    /// 加载桌面状态；文件不存在时返回默认值（不报错）。
    pub fn load(&self) -> Result<Desk, crate::error::CoreError> {
        let path = self.desk_path();
        if !path.exists() {
            return Ok(Desk::new(AppSettings::default()));
        }
        let raw = std::fs::read_to_string(&path)?;
        let mut desk: Desk = serde_json::from_str(&raw)?;
        migrate(&mut desk);
        desk.validate();
        Ok(desk)
    }

    /// 原子保存：先写临时文件再改名，避免崩溃造成配置损坏。
    pub fn save(&self, desk: &Desk) -> Result<(), crate::error::CoreError> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.desk_path();
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(desk)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// 明确删除配置文件（用于「重置所有设置」）。
    pub fn delete(&self) -> Result<(), crate::error::CoreError> {
        let path = self.desk_path();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// 版本迁移：结构变化时在此按版本推进。
///
/// 规则：永远向前迁移；每次发布新结构时新增一段 `if desk.version < N`。
fn migrate(desk: &mut Desk) {
    // v1：初始版本，暂无迁移逻辑。
    let _ = desk;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Fence;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("winbosk-core-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_returns_default_when_missing() {
        let store = ConfigStore::new(tmp_dir("missing"));
        let desk = store.load().unwrap();
        assert!(desk.fences.is_empty());
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let store = ConfigStore::new(dir.clone());
        let mut desk = Desk::new(AppSettings::default());
        desk.fences.push(Fence {
            id: 1,
            title: Some("工作".into()),
            monitor_id: 0,
            bounds: crate::model::Rect::new(0.0, 0.0, 300.0, 200.0),
            state: crate::model::FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: crate::model::FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            rule: None,
            collapsed: false,
        });
        store.save(&desk).unwrap();

        let loaded = ConfigStore::new(dir).load().unwrap();
        assert_eq!(loaded, desk);
        assert_eq!(loaded.fences[0].title.as_deref(), Some("工作"));
    }

    #[test]
    fn app_settings_missing_autostart_defaults_to_false() {
        let json = r#"{"show_free_area":true,"free_area_height":90.0,"hotkeys":{}}"#;
        let settings: AppSettings = serde_json::from_str(json).expect("旧配置应能正常反序列化");
        assert!(!settings.autostart, "缺失 autostart 字段时应默认为 false");
    }

    #[test]
    fn hotkey_config_defaults_and_accessors() {
        let mut cfg = HotkeyConfig::default();
        assert_eq!(
            cfg.get_action(HotkeyAction::ConsoleToggle),
            Some("Ctrl+Alt+T")
        );
        assert_eq!(
            cfg.get_action(HotkeyAction::DesktopToggle),
            Some("Ctrl+Alt+D")
        );
        assert_eq!(cfg.get_action(HotkeyAction::AutoOrganize), None);
        assert_eq!(cfg.get_action(HotkeyAction::Quit), Some("Ctrl+Shift+F10"));

        let binding = cfg.get_binding(HotkeyAction::ConsoleToggle).unwrap();
        assert!(binding.ctrl && binding.alt && binding.key == "T");

        cfg.set_action(HotkeyAction::AutoOrganize, Some("Ctrl+Shift+O".into()));
        assert_eq!(
            cfg.get_action(HotkeyAction::AutoOrganize),
            Some("Ctrl+Shift+O")
        );

        cfg.set_action(HotkeyAction::ConsoleToggle, None);
        assert_eq!(cfg.get_action(HotkeyAction::ConsoleToggle), None);
        assert_eq!(cfg.get_binding(HotkeyAction::ConsoleToggle), None);
    }

    #[test]
    fn hotkey_config_backward_compatibility() {
        // 1. 空 JSON 对象应反序列化为默认值
        let empty_json = "{}";
        let cfg: HotkeyConfig = serde_json::from_str(empty_json).expect("空配置应能正常反序列化");
        assert_eq!(cfg, HotkeyConfig::default());

        // 2. 含有旧字段的 JSON 应能成功反序列化并自动填补默认值
        let old_json = r#"{"collapse_all":"Ctrl+1","expand_all":"Ctrl+2","search":"Ctrl+F"}"#;
        let old_cfg: HotkeyConfig =
            serde_json::from_str(old_json).expect("含旧字段配置应能正常反序列化");
        assert_eq!(old_cfg.console_toggle, Some("Ctrl+Alt+T".to_string()));
        assert_eq!(old_cfg.desktop_toggle, Some("Ctrl+Alt+D".to_string()));
        assert_eq!(old_cfg.auto_organize, None);
        assert_eq!(old_cfg.quit, Some("Ctrl+Shift+F10".to_string()));
        assert_eq!(old_cfg.collapse_all, Some("Ctrl+1".to_string()));

        // 3. 序列化时旧字段被忽略，只序列化新字段
        let serialized = serde_json::to_string(&old_cfg).expect("序列化应成功");
        assert!(!serialized.contains("collapse_all"));
        assert!(!serialized.contains("expand_all"));
        assert!(!serialized.contains("search"));
        assert!(serialized.contains("console_toggle"));

        // 4. 用户显式覆写字段
        let custom_json = r#"{"console_toggle":null,"auto_organize":"Ctrl+Alt+O"}"#;
        let custom_cfg: HotkeyConfig =
            serde_json::from_str(custom_json).expect("自定义配置反序列化");
        assert_eq!(custom_cfg.console_toggle, None);
        assert_eq!(custom_cfg.auto_organize, Some("Ctrl+Alt+O".to_string()));
        assert_eq!(custom_cfg.desktop_toggle, Some("Ctrl+Alt+D".to_string()));
        assert_eq!(custom_cfg.quit, Some("Ctrl+Shift+F10".to_string()));
    }
}
