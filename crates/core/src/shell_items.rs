//! 虚拟壳项（无文件系统路径的桌面项，如回收站）的 id 口径与隐藏判定——纯逻辑，零 OS 依赖。
//!
//! 为什么单独一个模块：虚拟壳项**没有文件系统路径**（`Icon.path == None`），却仍然要
//! 参与三个互相冲突的口径：
//!
//! 1. **入口闸**：注册表「隐藏桌面图标」策略是否显式关掉了它（`policy_hidden`）；
//! 2. **身份**：必须有一个不与文件项、也不与其它虚拟项碰撞的可持久化 id
//!    （`VIRTUAL_ID_PREFIX` + 显示名 + CLSID 片段）；
//! 3. **排除**：智能整理 / 自动归类不得把这类项搬进分类栅栏（`is_virtual_id`）。
//!
//! 这里只放**纯逻辑**，注册表读与 PIDL 操作在 `winbosk-shell` 的 `virtual_items`。

/// 虚拟壳项 id 前缀。生成在 shell（`items::item_id`），消费在 core（整理候选集）
/// 与 app（移出栅栏拒绝）。
pub const VIRTUAL_ID_PREFIX: &str = "shell:";

/// 是否虚拟壳项 id。
///
/// **不要**用 `Icon.path.is_none()` 代替——那会把将来「无路径的拖入项」误判成虚拟壳项，
/// 从而错误地拒绝它移出栅栏 / 错误地把它排除出归类候选。
pub fn is_virtual_id(id: &str) -> bool {
    id.starts_with(VIRTUAL_ID_PREFIX)
}

/// 注册表三态。
///
/// None（键不存在）与 Some(false)（显式值）语义不同：前者意味着策略未表态、要去看另一
/// 个 hive，后者是用户/机器的明确裁决，禁止折叠成 `Option<bool>`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyValue {
    /// 键不存在（或读不到）。
    Unset,
    /// 显式 0 —— 显式显示。
    Enabled,
    /// 显式 1 —— 显式隐藏。
    Disabled,
}

impl PolicyValue {
    /// DWORD → 三态。非 0/1 的脏值按 `Enabled`（不隐藏）处理，不 panic。
    ///
    /// 与 Windows 的 `HideDesktopIcons` 语义一致：只认 0 与 1，其它值视为未隐藏。
    pub fn from_dword(v: Option<u32>) -> Self {
        match v {
            None => Self::Unset,
            Some(0) => Self::Enabled,
            Some(1) => Self::Disabled,
            Some(_) => Self::Enabled,
        }
    }
}

/// 是否被「显式隐藏」。
///
/// **只回答"准不准"，不回答"能不能"**——缺值返回 `false` 只代表"没有显式关掉它"，
/// 不代表它就该被显示；准入还要过白名单与注册探针（见 `virtual_items`）。
///
/// 四态口径：
///
/// | HKCU | HKLM | `policy_hidden` |
/// | :--- | :--- | :--- |
/// | `Disabled` | 任意 | `true`（用户隐藏优先于一切） |
/// | `Enabled` | `Disabled` | `false`（用户显式显示，压过机器策略） |
/// | `Unset` | `Disabled` | `true`（机器策略隐藏） |
/// | `Unset` | `Unset` | `false` |
///
/// `clsid` 参数当前未使用（白名单只有 1 条，直接在调用处比对）；保留是为将来按 CLSID
/// 批量查询时不改签名。
pub fn policy_hidden(clsid: &str, hkcu: PolicyValue, hklm: PolicyValue) -> bool {
    let _ = clsid;
    match hkcu {
        PolicyValue::Disabled => true,
        PolicyValue::Enabled => false,
        PolicyValue::Unset => hklm == PolicyValue::Disabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_hidden_matrix() {
        let c = "{645FF040-5081-101B-9F08-00AA002F954E}";
        // 用户显式隐藏 → 优先于一切（含用户另一侧显式显示、机器显示）
        assert!(policy_hidden(
            c,
            PolicyValue::Disabled,
            PolicyValue::Disabled
        ));
        assert!(policy_hidden(
            c,
            PolicyValue::Disabled,
            PolicyValue::Enabled
        ));
        assert!(policy_hidden(c, PolicyValue::Disabled, PolicyValue::Unset));
        // 用户显式显示 → 压过机器策略
        assert!(!policy_hidden(
            c,
            PolicyValue::Enabled,
            PolicyValue::Disabled
        ));
        assert!(!policy_hidden(c, PolicyValue::Enabled, PolicyValue::Unset));
        // 用户未表态：跟随机器策略
        assert!(policy_hidden(c, PolicyValue::Unset, PolicyValue::Disabled));
        assert!(!policy_hidden(c, PolicyValue::Unset, PolicyValue::Unset));
        assert!(!policy_hidden(c, PolicyValue::Unset, PolicyValue::Enabled));
    }

    #[test]
    fn policy_value_from_dword_three_states() {
        assert_eq!(PolicyValue::from_dword(None), PolicyValue::Unset);
        assert_eq!(PolicyValue::from_dword(Some(0)), PolicyValue::Enabled);
        assert_eq!(PolicyValue::from_dword(Some(1)), PolicyValue::Disabled);
        // 脏值不 panic：按不隐藏处理
        assert_eq!(PolicyValue::from_dword(Some(7)), PolicyValue::Enabled);
        assert_eq!(
            PolicyValue::from_dword(Some(u32::MAX)),
            PolicyValue::Enabled
        );
    }

    #[test]
    fn is_virtual_id_recognizes_prefix_only() {
        assert!(is_virtual_id("shell:回收站-645ff040"));
        assert!(is_virtual_id("shell:This PC-59031a47"));
        // 有路径的普通文件项
        assert!(!is_virtual_id(r"c:\users\me\desktop\a.txt"));
        // 无路径但非本前缀 → 不是虚拟壳项（不得被移出拒绝 / 归类排除误伤）
        assert!(!is_virtual_id(""));
        assert!(!is_virtual_id("no-prefix-no-path"));
    }
}
