//! 虚拟壳项（无文件系统路径的桌面项，如回收站）的注册表裁决 + PIDL 复刻。
//!
//! **唯一允许碰注册表 / PIDL 的虚拟壳项代码**。分层理由：注册表读与 PIDL 构造都是
//! OS 行为，必须留在 `winbosk-shell`；`winbosk-core` 只持有纯口径（`shell_items`）。
//!
//! ## 成本纪律
//!
//! - 探测只做**注册表读**（本切片 1 条白名单 × 2 hive = 2 次读，微秒级）；
//! - **禁止**在这里调 `IShellFolder::EnumObjects` 做全量枚举——实测 11.9~14.9 ms，
//!   会把 4~7 ms 的库同步心跳预算撑爆；
//! - 调用时机仅限**启动一次**与控制中心「刷新」（本切片只有前者）。

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{
    IShellFolder, SHGetDesktopFolder, SHParseDisplayName, ShellExecuteExW, SEE_MASK_INVOKEIDLIST,
    SHELLEXECUTEINFOW, SHGDNF, SHGDN_FORPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use winbosk_core::shell_items::{PolicyValue, VIRTUAL_ID_PREFIX};

/// 本切片只镜像回收站。CLSID 大写。
///
/// 放白名单 **不等于**一定显示——还要过策略闸（`policy_hidden`）与注册探针
/// （PIDL 能否解析、显示名能否取到）。
pub const MIRRORABLE_VIRTUAL_ITEMS: &[&str] = &[
    "{645FF040-5081-101B-9F08-00AA002F954E}", // 回收站
];

/// 「隐藏桌面图标」策略所在的子键（`Explorer` 之下）。
///
/// 每个 CLSID 在这里有一个 DWORD：0 = 显式显示，1 = 显式隐藏，缺键 = 未表态。
const HIDE_DESKTOP_ICONS_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons";

/// 当前应镜像的一个虚拟壳项快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualItemSnapshot {
    /// `::{645FF040-…}`（`SHParseDisplayName` 可直接吃）。
    pub parsing_name: String,
    /// `{645FF040-…}` 大写，与白名单比对用。
    pub clsid: String,
    /// 本地化显示名（「回收站」/「Recycle Bin」…）。
    pub display_name: String,
}

/// 读某个 hive 下该 CLSID 的策略值（只读，`KEY_READ` 权限）。
///
/// 键不存在 / 类型不是 DWORD / 被锁 → `PolicyValue::Unset`（保守：视为未表态）。
pub fn policy_value(hive: HKEY, clsid: &str) -> PolicyValue {
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            hive,
            PCWSTR(wide(HIDE_DESKTOP_ICONS_SUBKEY).as_ptr()),
            PCWSTR(wide(clsid).as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    PolicyValue::from_dword(status.is_ok().then_some(data))
}

/// 当前应镜像的虚拟项快照。
///
/// 判定 = 白名单准入 && 未被 `policy_hidden` 显式隐藏（HKCU 优先，其次 HKLM）。
/// **只在启动与用户点「刷新」时调用。**
pub fn mirrorable_virtual_snapshot() -> Vec<VirtualItemSnapshot> {
    let mut out = Vec::new();
    for clsid in MIRRORABLE_VIRTUAL_ITEMS {
        let hkcu = policy_value(HKEY_CURRENT_USER, clsid);
        let hklm = policy_value(HKEY_LOCAL_MACHINE, clsid);
        if winbosk_core::shell_items::policy_hidden(clsid, hkcu, hklm) {
            continue;
        }
        if let Some(snap) = snapshot_for(clsid) {
            out.push(snap);
        }
    }
    out
}

/// 白名单内单条的快照：解析名 → PIDL → 本地化显示名。任一步失败返回 None。
fn snapshot_for(clsid: &str) -> Option<VirtualItemSnapshot> {
    let parsing_name = format!("::{clsid}");
    let handle = virtual_item_handle(&parsing_name)?;
    let display_name = display_name_of(handle.pidl)?;
    Some(VirtualItemSnapshot {
        parsing_name,
        clsid: (*clsid).to_string(),
        display_name,
    })
}

/// 虚拟壳项的 id（H5 口径）。
///
/// `shell:<显示名小写>-<clsid 第 1..9 位小写>`。CLSID 片段不可省：实测存在**两个**同名
/// 「控制面板」，只按显示名会撞 id。本切片只对无路径项调用。
pub fn virtual_item_id(display_name: &str, clsid: &str) -> String {
    let frag = clsid
        .get(1..9)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| clsid.to_ascii_lowercase());
    format!("{}{}-{}", VIRTUAL_ID_PREFIX, display_name, frag)
}

/// PIDL 句柄；Drop 时 `CoTaskMemFree`。
pub struct VirtualItemHandle {
    pub pidl: *mut ITEMIDLIST,
}

impl Drop for VirtualItemHandle {
    fn drop(&mut self) {
        if !self.pidl.is_null() {
            unsafe { CoTaskMemFree(Some(self.pidl as *const _)) };
        }
    }
}

/// 由快照构造虚拟壳项的 `DesktopItem`（PIDL 由解析名复刻；失败时 pidl 为空，
/// 调用方仍可登记元数据，右键/打开会各自重建 PIDL）。
///
/// PIDL 的所有权整体移交给返回的 `DesktopItem`（其 `Drop` 负责 `CoTaskMemFree`），
/// 因此这里用 `ManuallyDrop` 阻止 `VirtualItemHandle` 提前释放。
pub fn virtual_item(snap: &VirtualItemSnapshot) -> crate::items::DesktopItem {
    let pidl = virtual_item_handle(&snap.parsing_name)
        .map(|h| std::mem::ManuallyDrop::new(h).pidl)
        .unwrap_or(std::ptr::null_mut());
    crate::items::DesktopItem {
        id: virtual_item_id(&snap.display_name, &snap.clsid),
        display_name: snap.display_name.clone(),
        kind: winbosk_core::model::ItemKind::Unknown,
        path: None, // H4：显式 None
        pidl,
    }
}

/// `SHParseDisplayName(parsing_name)` → PIDL。失败返回 None（调用方记日志并跳过）。
pub fn virtual_item_handle(parsing_name: &str) -> Option<VirtualItemHandle> {
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    let name = wide(parsing_name);
    let hr = unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) };
    if hr.is_err() || pidl.is_null() {
        return None;
    }
    Some(VirtualItemHandle { pidl })
}

/// 从 PIDL 取解析名（`SHGDN_FORPARSING`）——虚拟项 Shell 菜单预热 / 打开的键源。
///
/// crate 内可见：对外一律走 [`crate::items::DesktopItem::parsing_name`]（安全封装）。
/// 做成 `pub fn` 会被 `clippy::not_unsafe_ptr_arg_deref` 拦下。
pub(crate) fn parsing_name_of(pidl: *const ITEMIDLIST) -> String {
    if pidl.is_null() {
        return String::new();
    }
    let desktop: IShellFolder = match unsafe { SHGetDesktopFolder() } {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    let mut strret = STRRET::default();
    unsafe {
        if desktop
            .GetDisplayNameOf(pidl, SHGDN_FORPARSING, &mut strret)
            .is_err()
        {
            return String::new();
        }
    }
    crate::items::strret_to_string(&strret)
}

/// 本地化显示名（桌面文件夹的 `SHGDNF_NORMAL`）。
fn display_name_of(pidl: *const ITEMIDLIST) -> Option<String> {
    let desktop: IShellFolder = unsafe { SHGetDesktopFolder().ok()? };
    let mut strret = STRRET::default();
    unsafe {
        desktop
            .GetDisplayNameOf(pidl, SHGDNF(0), &mut strret)
            .ok()?;
    }
    let s = crate::items::strret_to_string(&strret);
    (!s.is_empty()).then_some(s)
}

/// 真实「打开」：`ShellExecuteEx` + `SEE_MASK_INVOKEIDLIST` + `lpIDList`。
///
/// 成功判据 = `ShellExecuteExW` 返回 `Ok`（windows-rs 已把 BOOL 折进 `Result`；
/// 失败时错误码来自 `hInstApp` 口径，不是 `GetLastError`）。失败日志由调用方记。
pub fn open_shell_item(pidl: *const ITEMIDLIST) -> bool {
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        hwnd: HWND::default(),
        lpVerb: windows::core::w!("open"),
        lpFile: PCWSTR::null(),
        lpParameters: PCWSTR::null(),
        lpDirectory: PCWSTR::null(),
        nShow: SW_SHOWNORMAL.0,
        lpIDList: pidl as *mut c_void,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.is_ok()
}

/// UTF-16 编码（含结尾 NUL），供 Win32 宽字符串参数使用。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_item_id_includes_clsid_fragment() {
        let a = virtual_item_id("控制面板", "{5399E694-F7B0-4E1B-9B0C-1F3E2D4C5B6A}");
        let b = virtual_item_id("控制面板", "{21EC2020-3AEA-1069-A2DD-08002B30309D}");
        assert_eq!(a, "shell:控制面板-5399e694");
        // 同名不同 CLSID → id 必须不同（H5）
        assert_ne!(a, b);
        assert_eq!(
            virtual_item_id("回收站", "{645FF040-5081-101B-9F08-00AA002F954E}"),
            "shell:回收站-645ff040"
        );
    }

    #[test]
    fn virtual_item_id_falls_back_when_clsid_too_short() {
        // 畸形 CLSID（不是有效 UTF-8 边界内未到 9 字符）不得 panic，退回完整小写串
        assert_eq!(virtual_item_id("回收站", "{45Z"), "shell:回收站-{45z");
    }

    #[test]
    fn whitelist_only_contains_recycle_bin() {
        assert_eq!(MIRRORABLE_VIRTUAL_ITEMS.len(), 1);
        assert!(MIRRORABLE_VIRTUAL_ITEMS[0].starts_with("{645FF040"));
    }
}
