//! 壳层接管：探测桌面窗口层级，隐藏真实图标视图，并提供恢复。
//!
//! 反冲突原则（设计文档 §6.2）：
//! 1. **动态探测**层级，不假设经典 Progman→WorkerW→DefView 结构；
//! 2. **绝不重挂/销毁他人窗口**——只隐藏 `SysListView32`（保留句柄）+ 插入自己的
//!    overlay 窗口作为 sibling；Wallpaper Engine 的窗口树不被动过；
//! 3. 可恢复：卸载时把隐藏的 ListView 原样恢复。
//!
//! 注意：图标枚举走 `IShellFolder`（见 `items.rs`），不依赖 DefView 是否存在，
//! 因此用户开启「隐藏图标」时本模块的接管流程依然成立。

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, FindWindowExW, FindWindowW, GetClassNameW, GetParent, SendMessageW,
    ShowWindow, ShowWindowAsync, SW_HIDE, SW_SHOW,
};

/// 触发桌面 WorkerW 生成的私有消息（广泛使用的 Progman 技巧）。
const WM_SPAWN_WORKERW: u32 = 0x052C;

const CLASS_PROGMAN: &str = "Progman";
const CLASS_DEFVIEW: &str = "SHELLDLL_DefView";
const CLASS_LISTVIEW: &str = "SysListView32";

/// 桌面层级探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopHierarchy {
    /// Progman 根窗口。
    pub progman: HWND,
    /// 持有 SHELLDLL_DefView 的窗口（通常是 WorkerW；DefView 不存在时为 None）。
    pub worker: Option<HWND>,
    /// 真实桌面图标视图 `SHELLDLL_DefView`。
    pub def_view: Option<HWND>,
    /// 真实图标列表 `SysListView32`（DefView 的子窗口）。
    pub list_view: Option<HWND>,
}

impl DesktopHierarchy {
    /// overlay 窗口应挂靠的父窗口：优先 DefView 所在层级，
    /// 无 DefView（用户隐藏了图标）时退回 Progman。
    pub fn overlay_parent(&self) -> HWND {
        self.worker.or(self.def_view).unwrap_or(self.progman)
    }

    /// 隐藏真实图标列表。返回 `false` 表示本就没有可隐藏的图标视图。
    pub fn hide_icons(&self) -> bool {
        match self.list_view {
            Some(lv) if !lv.is_invalid() => {
                let _hid = unsafe { ShowWindow(lv, SW_HIDE) };
                true
            }
            _ => false,
        }
    }

    /// 恢复真实图标列表（同步）：启动期 `desktop_mode` 分支与运行期切换需要立即生效。
    pub fn restore_icons(&self) {
        if let Some(lv) = self.list_view {
            if !lv.is_invalid() {
                let _shw = unsafe { ShowWindow(lv, SW_SHOW) };
            }
        }
    }

    /// 恢复真实图标列表（**异步**投递版）：`ShowWindowAsync` 把显示请求投递给目标
    /// 线程队列后立即返回——退出路径绝不等待 explorer 处理（同步版在 explorer 繁忙
    /// 时会把本进程的退出钉死）。语义差异：拿不到「先前是否可见」的返回值（本工程
    /// 不使用该返回值）；即使本进程投递后立即结束，explorer 仍会在自己的队列里处理。
    pub fn restore_icons_async(&self) {
        if let Some(lv) = self.list_view {
            if !lv.is_invalid() {
                let _shw = unsafe { ShowWindowAsync(lv, SW_SHOW) };
            }
        }
    }
}

/// overlay 窗口类名。必须与 render 层 `overlay::CLASS_NAME` 一致；依赖方向禁止
/// shell 依赖 render，故此处硬编码（与 `CLASS_PROGMAN` 等同款约定）。
const CLASS_OVERLAY: &str = "WinBoskOverlay";

/// overlay 窗口是否仍存在。**只做顶层窗口检索、不发任何消息**（best-effort 提示，
/// 不作判据）。
///
/// overlay 是 `WS_POPUP` **顶层**窗口——`OverlayWindow::create` 虽接收 WorkerW/
/// Progman 作参数，但那是它的 **owner**（`GetParent` 返回 owner），不是 `WS_CHILD`
/// 的父窗口。所以顶层 `FindWindowW` 直接可查；而子窗口方向（`FindWindowExW`/
/// `EnumChildWindows` 沿 Progman 下钻）**永远找不到它**（2026-09-28 实测纠正：
/// 曾按「子窗口」实现，第二实例日志实测 `overlay_alive=false`）。
///
/// 禁止复用 [`probe`]：它会给 Progman 发 `WM_SPAWN_WORKERW`，在被拒的第二实例里
/// 既多余、又可能在 explorer 繁忙时把本进程拖住。
///
/// 结果仅作提示：上次退出若卡在 `DestroyWindow` 之前，overlay 仍在，会误报 alive；
/// 定案证据以收尾阶段埋点为准。
pub fn overlay_window_alive() -> bool {
    overlay_window().is_some()
}

/// overlay 窗口句柄（按类名只读查找）。存活判定与第二实例唤醒共用。
///
/// 查找方式与 [`overlay_window_alive`] 同款：顶层 `FindWindowW`（overlay 是
/// WS_POPUP 顶层窗口，WorkerW 只是 owner），禁止复用 [`probe`]（会发
/// `WM_SPAWN_WORKERW`，在被拒的第二实例里多余且可能拖住本进程）。
pub fn overlay_window() -> Option<HWND> {
    find_class_window(CLASS_OVERLAY)
}

/// 探测桌面窗口层级。
///
/// 流程：
/// 1. 定位 Progman；
/// 2. 发送 spawn-worker 消息，确保图标层存在；
/// 3. 搜索持有 `SHELLDLL_DefView` 的窗口（顶层或 Progman 子树）；
/// 4. 从 DefView 定位 `SysListView32` 与父 WorkerW。
pub fn probe() -> Option<DesktopHierarchy> {
    let progman = find_class_window(CLASS_PROGMAN)?;
    // 触发 WorkerW / DefView 生成（幂等）
    unsafe {
        SendMessageW(
            progman,
            WM_SPAWN_WORKERW,
            Default::default(),
            Default::default(),
        )
    };

    let def_view = find_def_view();
    let worker = def_view
        .and_then(|dv| unsafe { GetParent(dv) }.ok())
        .filter(|p| !p.is_invalid());
    let list_view = def_view.and_then(|dv| find_class_child(dv, CLASS_LISTVIEW));

    Some(DesktopHierarchy {
        progman,
        worker,
        def_view,
        list_view,
    })
}

/// 查找指定类名的顶层窗口。
fn find_class_window(class_name: &str) -> Option<HWND> {
    let class = wide(class_name);
    let hwnd = unsafe { FindWindowW(PCWSTR(class.as_ptr()), None) }.ok()?;
    (!hwnd.is_invalid()).then_some(hwnd)
}

/// 查找 `SHELLDLL_DefView`：
/// 先试顶层（部分系统上 DefView 是顶层窗口），再递归 Progman 子树
/// （Wallpaper Engine 会把 DefView 重挂到它新建的 WorkerW 下）。
fn find_def_view() -> Option<HWND> {
    if let Some(hwnd) = find_class_window(CLASS_DEFVIEW) {
        return Some(hwnd);
    }
    let progman = find_class_window(CLASS_PROGMAN)?;
    find_class_descendant(progman, CLASS_DEFVIEW)
}

/// 在 `hwnd` 的直接子窗口中查找指定类名的窗口。
fn find_class_child(hwnd: HWND, class_name: &str) -> Option<HWND> {
    let class = wide(class_name);
    unsafe { FindWindowExW(Some(hwnd), None, PCWSTR(class.as_ptr()), None) }
        .ok()
        .filter(|h| !h.is_invalid())
}

/// 递归枚举回调的上下文。
struct FindCtx {
    class_name: String,
    found: Option<HWND>,
}

/// 在 `hwnd` 子树中递归查找指定类名的窗口（返回第一个匹配）。
fn find_class_descendant(hwnd: HWND, class_name: &str) -> Option<HWND> {
    let mut ctx = FindCtx {
        class_name: class_name.to_string(),
        found: None,
    };
    // 无尾分号：让 BOOL 作为块值，避免 unused_must_use
    let _enum = unsafe {
        EnumChildWindows(
            Some(hwnd),
            Some(enum_child_cb),
            LPARAM(&mut ctx as *mut _ as isize),
        )
    };
    ctx.found
}

unsafe extern "system" fn enum_child_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut FindCtx);
    if ctx.found.is_some() {
        return BOOL(0); // 已找到，停止
    }
    if class_of(hwnd).as_deref() == Some(ctx.class_name.as_str()) {
        ctx.found = Some(hwnd);
        BOOL(0)
    } else {
        BOOL(1) // 继续枚举
    }
}

/// 读取窗口类名。
fn class_of(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// 转换 `&str` 为以 NUL 结尾的 UTF-16 宽字符串。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_conversion_roundtrip() {
        assert_eq!(wide_to_string_test(&wide("Progman")), "Progman");
        assert_eq!(
            wide_to_string_test(&wide("SHELLDLL_DefView")),
            "SHELLDLL_DefView"
        );
    }

    fn wide_to_string_test(w: &[u16]) -> String {
        let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..end])
    }

    /// 只读检索路径可执行、不 panic、不发消息（真值取决于本机是否运行 WinBosk，
    /// 不作断言：CI 上必然 false，开发机上可能 true）。
    #[test]
    fn overlay_window_alive_is_read_only_probe() {
        let _ = overlay_window_alive();
    }

    #[test]
    fn probe_compiles_and_is_consistent() {
        // 真实桌面环境下运行；只要返回了 Progman，层级字段必须自洽。
        if let Some(h) = probe() {
            assert!(!h.progman.is_invalid());
            if let Some(dv) = h.def_view {
                assert_eq!(class_of(dv).as_deref(), Some(CLASS_DEFVIEW));
            }
            if let Some(w) = h.worker {
                // 父窗口要么是 WorkerW，要么是 Progman 本身
                let cls = class_of(w);
                assert!(cls.is_some());
            }
            if let Some(lv) = h.list_view {
                assert_eq!(class_of(lv).as_deref(), Some(CLASS_LISTVIEW));
            }
        }
    }
}
