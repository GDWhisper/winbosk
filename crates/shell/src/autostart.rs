//! 开机自启管理（基于 Windows 注册表 Run 键）。
//!
//! 项位置：`HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
//! 键名默认为 `"WinBosk"`，值为当前可执行文件全路径。

use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 设置或清除当前用户的开机自启项。
///
/// 当 `enable == true` 时，将 `"<exe_path>"` 写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\<app_name>`。
/// 当 `enable == false` 时，从注册表中删除该项（若不存在则视作成功）。
pub fn set_autostart(app_name: &str, exe_path: &Path, enable: bool) -> Result<(), String> {
    let subkey_w = wide(RUN_SUBKEY);
    let mut hkey = HKEY::default();
    let res = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if res != ERROR_SUCCESS {
        return Err(format!("无法打开注册表 Run 键: 错误代码 {}", res.0));
    }

    let val_name_w = wide(app_name);
    let result = if enable {
        let quoted = format!("\"{}\"", exe_path.display());
        let val_data_w = wide(&quoted);
        let set_res = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(val_name_w.as_ptr()),
                None,
                REG_SZ,
                Some(core::slice::from_raw_parts(
                    val_data_w.as_ptr() as *const u8,
                    val_data_w.len() * 2,
                )),
            )
        };
        if set_res == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("设置开机自启注册表失败: 错误代码 {}", set_res.0))
        }
    } else {
        let del_res = unsafe { RegDeleteValueW(hkey, PCWSTR(val_name_w.as_ptr())) };
        if del_res == ERROR_SUCCESS || del_res == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("删除开机自启注册表失败: 错误代码 {}", del_res.0))
        }
    };

    unsafe {
        let _ = RegCloseKey(hkey);
    }
    result
}

/// 查询注册表中是否存在有效的开机自启项。
///
/// 若 `exe_path` 为 `Some`，则进一步校验路径是否一致；为 `None` 时仅检查键值是否存在且非空。
pub fn is_autostart_enabled(app_name: &str, exe_path: Option<&Path>) -> bool {
    let subkey_w = wide(RUN_SUBKEY);
    let val_name_w = wide(app_name);
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;

    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            PCWSTR(val_name_w.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };

    if status.is_err() {
        return false;
    }

    let len = (size as usize / 2).saturating_sub(1);
    let raw_val = String::from_utf16_lossy(&buf[..len]);
    let clean = raw_val.trim_matches(|c: char| c == '\0' || c == '"' || c.is_whitespace());

    if let Some(target) = exe_path {
        let clean_path = Path::new(clean);
        clean_path == target || clean.eq_ignore_ascii_case(&target.to_string_lossy())
    } else {
        !clean.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_roundtrip() {
        let app_name = "WinBosk_UnitTest_Autostart";
        let fake_exe = Path::new(r"C:\Program Files\WinBosk\winbosk.exe");

        // 确保清理初始状态
        let _ = set_autostart(app_name, fake_exe, false);
        assert!(!is_autostart_enabled(app_name, Some(fake_exe)));

        // 启用自启
        let set_res = set_autostart(app_name, fake_exe, true);
        assert!(set_res.is_ok(), "设置自启应成功: {set_res:?}");
        assert!(is_autostart_enabled(app_name, Some(fake_exe)));
        assert!(is_autostart_enabled(app_name, None));

        // 禁用自启
        let del_res = set_autostart(app_name, fake_exe, false);
        assert!(del_res.is_ok(), "禁用自启应成功: {del_res:?}");
        assert!(!is_autostart_enabled(app_name, Some(fake_exe)));
    }
}
