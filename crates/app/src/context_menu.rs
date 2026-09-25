//! 右键/托盘菜单：菜单构建、动作分发、剪贴板与文件选择。

use crate::*;
use windows::Win32::UI::Shell::SHCreateItemFromIDList;
pub(crate) const MENU_ICON_OPEN: usize = 1;
pub(crate) const MENU_ICON_REMOVE: usize = 2;
pub(crate) const MENU_PASTE: usize = 2500;
pub(crate) const MENU_ORGANIZE: usize = 3500;
pub(crate) const MENU_DELETE_FENCE: usize = 5000;
pub(crate) const MENU_RENAME_FENCE: usize = 6000;
pub(crate) const MENU_TOGGLE_COLLAPSE: usize = 7000;
pub(crate) fn handle_tray_menu(rt: &mut Runtime) {
    const MENU_TRAY_ORGANIZE: usize = 8199;
    const MENU_TRAY_CONSOLE: usize = 8200;
    const MENU_TRAY_QUIT: usize = 8201;
    let menu = popup_menu();
    if menu.is_invalid() {
        return;
    }
    unsafe {
        let s = wide("⚡ 一键整理桌面");
        let _ = AppendMenuW(menu, MF_STRING, MENU_TRAY_ORGANIZE, PCWSTR(s.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let s = wide("显示 WinBosk 控制中心");
        let _ = AppendMenuW(menu, MF_STRING, MENU_TRAY_CONSOLE, PCWSTR(s.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let s = wide("退出 WinBosk");
        let _ = AppendMenuW(menu, MF_STRING, MENU_TRAY_QUIT, PCWSTR(s.as_ptr()));
    }
    let (sx, sy) = cursor_screen();
    let cmd = track_popup_menu(rt, menu, sx, sy);
    unsafe {
        let _ = DestroyMenu(menu);
    }
    match cmd {
        MENU_TRAY_ORGANIZE => {
            execute_auto_organize(rt);
        }
        MENU_TRAY_CONSOLE => {
            set_console_open(rt, !rt.desk.console_open);
        }
        MENU_TRAY_QUIT => unsafe {
            let _ = PostMessageW(Some(rt.hwnd), WM_APP_QUIT, WPARAM(0), LPARAM(0));
        },
        _ => {}
    }
}

/// 图标右键菜单动作。
pub(crate) enum IconMenuAction {
    Open,
    Remove,
}

/// 栅栏右键菜单动作（精简版：收起/展开 / 粘贴 / 重命名 / 整理 / 删除）。
pub(crate) enum FenceMenuAction {
    ToggleCollapse,
    Paste,
    Rename,
    Organize,
    Delete,
}

/// 处理右键：弹出上下文菜单并执行选中动作（菜单为模态，阻塞到关闭）。
pub(crate) fn handle_context_menu(
    rt: &mut Runtime,
    fence: usize,
    icon: Option<usize>,
    _pos: (f32, f32),
) {
    let (sx, sy) = cursor_screen();
    if let Some(ii) = icon {
        // 该项文件路径（打开/删除判定也要用）
        let path = rt
            .desk
            .fences
            .get(fence)
            .and_then(|f| f.icon_ids.get(ii))
            .and_then(|id| rt.desk.icons.get(id))
            .and_then(|ic| ic.path.clone());
        // 虚拟壳项（回收站等无路径项）：真实 Shell 菜单走 PIDL 构造的 IShellItem，
        // 不走 `SHCreateItemFromParsingName`——它没有文件系统路径。
        let virtual_id = rt
            .desk
            .fences
            .get(fence)
            .and_then(|f| f.icon_ids.get(ii))
            .cloned()
            .filter(|id| winbosk_core::shell_items::is_virtual_id(id));
        // 该项是否由 WinBosk 管理（库内项 / 链接镜像项 / 虚拟项，added=true）：
        // 栅栏内容与文件夹同步，「移出栅栏」与「删除」等价（镜像项移出即删文件、
        // 库内项移出即删引用），菜单不再重复提供「移出栅栏」，只留「删除」。
        // 真实桌面图标（added=false）不同：移出=回未分组区，删除=回收站，保留「移出」。
        let managed = rt
            .desk
            .fences
            .get(fence)
            .and_then(|f| f.icon_ids.get(ii))
            .and_then(|id| rt.desk.icons.get(id))
            .map(|ic| ic.added)
            .unwrap_or(false);
        // 多选集合：右键集合中的任意一项 → 集合操作（打开全部 / 复制 / 移出 / 删除）。
        // 右键未选中的项 → 先单选该项，再走单项逻辑（资源管理器行为）。
        let key = (fence, ii);
        let multi = rt.selected.len() > 1 && rt.selected.contains(&key);
        if multi {
            // 先取值再分发：菜单函数只借 `&Runtime`，若写在 match 表达式里，
            // 借用会持续到整个 match，arm 内再拿 `&mut` 就会冲突。
            let action = multi_icon_context_menu(rt, sx, sy, managed);
            match action {
                Some(MultiMenuAction::Open) => open_selected(rt),
                Some(MultiMenuAction::Copy) => copy_selected(rt),
                Some(MultiMenuAction::Remove) => remove_selected(rt),
                Some(MultiMenuAction::Delete) => delete_selected(rt),
                None => {}
            }
            return;
        }
        if !rt.selected.contains(&key) {
            rt.selected = vec![key];
        }
        // 虚拟壳项：用 PIDL 构造 IShellItem 弹真实 Shell 菜单（等同桌面右键回收站）。
        // 菜单注入侧同样标记 `managed`（虚拟项 added=true），不提供「移出栅栏」。
        if let (Some(vid), Some(&item_idx)) = (
            virtual_id.as_deref(),
            rt.desk
                .fences
                .get(fence)
                .and_then(|f| f.icon_ids.get(ii))
                .and_then(|id| rt.item_index.get(id)),
        ) {
            let (pidl, name, parsing) = rt
                .items
                .get(item_idx)
                .map(|it| (it.pidl, it.display_name.clone(), it.parsing_name()))
                .unwrap_or((std::ptr::null_mut(), vid.to_string(), String::new()));
            if pidl.is_null() {
                tracing::warn!(id = %vid, "虚拟壳项 PIDL 缺失，退回简版菜单");
                if let Some(IconMenuAction::Open) = icon_context_menu(rt, sx, sy, managed) {
                    launch_fence_icon(rt, fence, ii);
                }
                return;
            }
            let item: windows::Win32::UI::Shell::IShellItem = match unsafe {
                SHCreateItemFromIDList::<windows::Win32::UI::Shell::IShellItem>(pidl)
            } {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!(id = %vid, "虚拟壳项 IShellItem 构造失败: {e}");
                    if let Some(IconMenuAction::Open) = icon_context_menu(rt, sx, sy, managed) {
                        launch_fence_icon(rt, fence, ii);
                    }
                    return;
                }
            };
            let result = shell_menu::show_item(rt, &item, &parsing, &name, sx, sy, managed);
            // 虚拟壳项的原生动词（清空回收站 / 属性 / 固定到快速访问…）都在 Shell 内
            // 完成，本进程没有任何状态要跟：栅栏成员不变、没有死图标要回收、
            // 「移出栅栏」与「重命名」本就不注入（managed=true）。故只留一行日志。
            tracing::debug!(id = %vid, ?result, "虚拟壳项 Shell 菜单结束");
            return;
        }
        // 有文件路径的项走真实 Shell 右键菜单（等同桌面右键）；虚拟项（无路径）
        // 退回简版「打开」菜单。Shell 菜单在主线程模态弹出，与 Windows 右键
        // 行为一致；慢扩展已由后台预热（见 shell_menu::show），首次右键不卡死。
        match path {
            // 先取值再分发：菜单函数只借 `&Runtime`，写在 match 表达式里会让借用
            // 持续到整个 match，arm 内再拿 `&mut` 就会冲突。
            Some(p) => {
                let result = shell_menu::show(rt, &p, sx, sy, managed);
                match result {
                    shell_menu::ShellMenuResult::Remove => remove_fence_icon(rt, fence, ii),
                    shell_menu::ShellMenuResult::Rename => {
                        start_inplace_rename(rt, EditTarget::Item { fence, icon: ii })
                    }
                    shell_menu::ShellMenuResult::Invoked => {
                        // 原生动词执行后（如「删除」）：文件若已被移走/删除，立即清掉
                        // 栅栏里的死图标，等价于资源管理器删除后刷新视图。
                        if !std::path::Path::new(&p).exists() {
                            if let Some(id) = rt
                                .desk
                                .fences
                                .get(fence)
                                .and_then(|f| f.icon_ids.get(ii))
                                .cloned()
                            {
                                remove_icon_entirely(rt, &id);
                            }
                            let _ = rt.store.save(&rt.desk);
                        }
                    }
                    // 真实 Shell 菜单没弹出来（路径无效 / COM 异常 / 工作线程失败等）：
                    // 退回简版菜单，保证右键必有反馈，不让「Windows 右击列表」静默消失。
                    shell_menu::ShellMenuResult::Failed => {
                        tracing::warn!(path = %p, "Shell 右键菜单创建失败，退回简版菜单");
                        let action = icon_context_menu(rt, sx, sy, managed);
                        match action {
                            Some(IconMenuAction::Open) => launch_fence_icon(rt, fence, ii),
                            Some(IconMenuAction::Remove) => remove_fence_icon(rt, fence, ii),
                            None => {}
                        }
                    }
                    // 用户取消菜单（Esc / 点击别处）：什么都不做。
                    // 旧实现把「取消」误当成「创建失败」，取消了还会再弹一次简版菜单。
                    shell_menu::ShellMenuResult::Canceled => {}
                }
            }
            None => {
                let action = icon_context_menu(rt, sx, sy, managed);
                match action {
                    Some(IconMenuAction::Open) => launch_fence_icon(rt, fence, ii),
                    Some(IconMenuAction::Remove) => remove_fence_icon(rt, fence, ii),
                    None => {}
                }
            }
        }
    } else if let Some(action) = fence_context_menu(rt, fence, sx, sy) {
        match action {
            FenceMenuAction::ToggleCollapse => {
                if let Some(f) = rt.desk.fences.get_mut(fence) {
                    if f.appearance.layout != FenceLayout::Sidebar {
                        f.collapsed = !f.collapsed;
                        let _ = rt.store.save(&rt.desk);
                    }
                }
            }
            FenceMenuAction::Paste => {
                let paths = clipboard_file_paths();
                if !paths.is_empty() {
                    add_paths_to_fence(rt, fence, &paths);
                } else {
                    tracing::info!("剪贴板中没有文件，跳过粘贴");
                }
            }
            FenceMenuAction::Rename => {
                start_inplace_rename(rt, EditTarget::FenceTitle { fence });
            }
            FenceMenuAction::Organize => {
                execute_auto_organize(rt);
            }
            FenceMenuAction::Delete => {
                delete_fence_and_reclaim_icons(rt, fence);
            }
        }
    }
}

/// 图标右键菜单（简版回退）：打开 / 移出栅栏（WinBosk 管理项不提供移出，见 `handle_context_menu`）。
pub(crate) fn icon_context_menu(
    rt: &Runtime,
    sx: i32,
    sy: i32,
    managed: bool,
) -> Option<IconMenuAction> {
    let menu = popup_menu();
    if menu.is_invalid() {
        return None;
    }
    unsafe {
        let s = wide("打开");
        let _ = AppendMenuW(menu, MF_STRING, MENU_ICON_OPEN, PCWSTR(s.as_ptr()));
        if !managed {
            let s2 = wide("移出栅栏");
            let _ = AppendMenuW(menu, MF_STRING, MENU_ICON_REMOVE, PCWSTR(s2.as_ptr()));
        }
    }
    let cmd = track_popup_menu(rt, menu, sx, sy);
    unsafe {
        let _ = DestroyMenu(menu);
    }
    match cmd {
        MENU_ICON_OPEN => Some(IconMenuAction::Open),
        MENU_ICON_REMOVE => Some(IconMenuAction::Remove),
        _ => None,
    }
}

/// 多选集合的右键菜单动作。
pub(crate) enum MultiMenuAction {
    Open,
    Copy,
    Remove,
    Delete,
}

/// 多选右键菜单：打开全部 / 复制 / 移出栅栏 / 删除。返回选中的动作。
/// `managed`（右键项为 WinBosk 管理项，见 `handle_context_menu`）：移出与「删除」等价，
/// 跳过「移出栅栏」，只留 打开/复制/删除。
pub(crate) fn multi_icon_context_menu(
    rt: &Runtime,
    sx: i32,
    sy: i32,
    managed: bool,
) -> Option<MultiMenuAction> {
    const M_OPEN: usize = 1;
    const M_COPY: usize = 2;
    const M_REMOVE: usize = 3;
    const M_DELETE: usize = 4;
    let menu = popup_menu();
    if menu.is_invalid() {
        return None;
    }
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING, M_OPEN, PCWSTR(wide("打开").as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, M_COPY, PCWSTR(wide("复制").as_ptr()));
        if !managed {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, M_REMOVE, PCWSTR(wide("移出栅栏").as_ptr()));
        }
        let _ = AppendMenuW(menu, MF_STRING, M_DELETE, PCWSTR(wide("删除").as_ptr()));
    }
    let cmd = track_popup_menu(rt, menu, sx, sy);
    unsafe {
        let _ = DestroyMenu(menu);
    }
    match cmd {
        M_OPEN => Some(MultiMenuAction::Open),
        M_COPY => Some(MultiMenuAction::Copy),
        M_REMOVE => Some(MultiMenuAction::Remove),
        M_DELETE => Some(MultiMenuAction::Delete),
        _ => None,
    }
}

/// 把一组路径写入剪贴板（CF_HDROP，与资源管理器「复制」同一格式）。
pub(crate) fn set_clipboard_paths(paths: &[String]) {
    // DROPFILES 头（20 字节）：pFiles 偏移 + 坐标 + 标志；随后每个路径 UTF-16LE + \0，
    // 列表以额外 \0 结束（末路径的 \0 + 结尾 \0 = 双 \0 终止）。
    let header = 20usize;
    let bytes_len: usize = header
        + paths
            .iter()
            .map(|p| p.encode_utf16().count() * 2 + 2)
            .sum::<usize>()
        + 2;
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, bytes_len) {
            let ptr = GlobalLock(hglobal);
            if !ptr.is_null() {
                let buf = std::slice::from_raw_parts_mut(ptr as *mut u8, bytes_len);
                buf.fill(0);
                // DROPFILES 头（20 字节）：pFiles=偏移20、pt 坐标、fNC、fWide
                // 偏移：pFiles(0..4) | pt(4..12) | fNC(12..16) | fWide(16..20)
                buf[0..4].copy_from_slice(&(header as u32).to_le_bytes());
                buf[16..20].copy_from_slice(&1u32.to_le_bytes()); // fWide = TRUE（UTF-16）
                let mut off = header;
                for p in paths {
                    for u in p.encode_utf16() {
                        let bytes = u.to_le_bytes();
                        buf[off] = bytes[0];
                        buf[off + 1] = bytes[1];
                        off += 2;
                    }
                    off += 2; // 路径结尾 \0
                }
                // 列表结束双 \0 的第二个由上面的 buf.fill(0) 保证（bytes_len 已计入 +2）
                let _ = GlobalUnlock(hglobal);
                // fWide 位于偏移 16（BOOL），fNC 位于偏移 12
                let _ = SetClipboardData(CF_HDROP, Some(HANDLE(hglobal.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

/// 选择单个文件夹（控制中心的「更改文件位置…」用）：弹出系统文件夹选择对话框，返回选中路径。
///
/// 单选 + 标题明确，**不要**退化成多选：改存储位置只会指向**一个**目录，而多选版
/// （曾经的 `pick_paths`，标题「添加到栅栏」）会让用户以为自己在往栅栏里加文件——
/// 那正是本次要修掉的语义错位之一。用户取消返回 `None`。
///
/// COM 已在启动早期以 STA 初始化。对话框运行在自己模态消息循环里，期间到达的
/// overlay 事件会被重入守卫丢弃——与 `TrackPopupMenu` 同一套机制，不会破坏状态。
pub(crate) fn pick_folder(owner: HWND) -> Option<String> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let title = PCWSTR(wide("选择栅栏的存储文件夹").as_ptr());
        dialog.SetTitle(title).ok()?;
        let opts = FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM;
        dialog.SetOptions(opts).ok()?;
        if dialog.Show(Some(owner)).is_err() {
            return None;
        }
        let item: IShellItem = dialog.GetResult().ok()?;
        let name: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        name.to_string().ok()
    }
}

/// 栅栏右键菜单：粘贴 / 重命名 / 删除（精简版）。
pub(crate) fn fence_context_menu(
    rt: &mut Runtime,
    fence: usize,
    sx: i32,
    sy: i32,
) -> Option<FenceMenuAction> {
    let main = popup_menu();
    if main.is_invalid() {
        return None;
    }
    let is_sidebar = rt
        .desk
        .fences
        .get(fence)
        .map(|f| f.appearance.layout == FenceLayout::Sidebar)
        .unwrap_or(false);
    let is_collapsed = rt
        .desk
        .fences
        .get(fence)
        .map(|f| f.collapsed)
        .unwrap_or(false);
    let collapse_text = if is_collapsed {
        "展开栅栏"
    } else {
        "收起栅栏"
    };
    unsafe {
        if !is_sidebar {
            let s = wide(collapse_text);
            let _ = AppendMenuW(main, MF_STRING, MENU_TOGGLE_COLLAPSE, PCWSTR(s.as_ptr()));
        }
        let s = wide("粘贴文件");
        let _ = AppendMenuW(main, MF_STRING, MENU_PASTE, PCWSTR(s.as_ptr()));
        let s = wide("重命名栅栏");
        let _ = AppendMenuW(main, MF_STRING, MENU_RENAME_FENCE, PCWSTR(s.as_ptr()));
        let s = wide("⚡ 一键整理桌面");
        let _ = AppendMenuW(main, MF_STRING, MENU_ORGANIZE, PCWSTR(s.as_ptr()));
        let _ = AppendMenuW(main, MF_SEPARATOR, 0, PCWSTR::null());
        let s = wide("删除栅栏");
        let _ = AppendMenuW(main, MF_STRING, MENU_DELETE_FENCE, PCWSTR(s.as_ptr()));
    }
    let cmd = track_popup_menu(rt, main, sx, sy);
    unsafe {
        let _ = DestroyMenu(main);
    }
    match cmd {
        MENU_TOGGLE_COLLAPSE => Some(FenceMenuAction::ToggleCollapse),
        MENU_PASTE => Some(FenceMenuAction::Paste),
        MENU_RENAME_FENCE => Some(FenceMenuAction::Rename),
        MENU_ORGANIZE => Some(FenceMenuAction::Organize),
        MENU_DELETE_FENCE => Some(FenceMenuAction::Delete),
        _ => None,
    }
}

/// 创建一个弹出菜单句柄（失败返回无效句柄，后续用 `is_invalid` 判空）。
pub(crate) fn popup_menu() -> HMENU {
    unsafe { CreatePopupMenu().unwrap_or_default() }
}

/// 模态弹出菜单，返回选中的命令 id（0 = 用户取消：Esc 或点击菜单外）。
///
/// `TrackPopupMenu` 要求 owner 窗口在弹出前**已经**是前台窗口，否则点击菜单外区域
/// 菜单不会关闭——一直悬在屏幕上，只剩 Esc 和「随便选一项」两条退路（托盘右键菜单
/// 就是这个症状）。overlay 本体带 `WS_EX_NOACTIVATE`（激活它会把桌面壳层提到应用
/// 之上，系统也不接受激活），所以 owner 与前台都交给可激活的焦点代理窗口，见
/// `OverlayWindow::menu_owner`。
///
/// 菜单收起后，只有前台仍停在代理窗口（用户选了某项或按 Esc）才把焦点还给菜单弹出
/// 前的窗口，避免用户当前窗口一直灰着；用户点到别的窗口时不抢焦点回来。
pub(crate) fn track_popup_menu(rt: &Runtime, menu: HMENU, sx: i32, sy: i32) -> usize {
    unsafe {
        let prev = GetForegroundWindow();
        let owner = (*rt.overlay_ptr).menu_owner();
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            sx,
            sy,
            Some(0),
            owner,
            None,
        )
        .0 as usize;
        // 收尾空消息：确保菜单的模态状态彻底退出（owner 收到 WM_NULL 无副作用）。
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        if !prev.is_invalid() && GetForegroundWindow() == owner {
            let _ = SetForegroundWindow(prev);
        }
        cmd
    }
}

/// 删除栅栏前的二次确认。返回 `true` 表示用户确认删除。
///
/// 删除栅栏不可撤销：`fences.remove` 会连同标题、外观、布局、分类规则一起丢弃并立即落盘。
/// 而入口之一是控制中心里**单击即触发**的「移出栅栏」，太容易误触，故统一收口做确认。
/// 图标本身不会丢失（删除时会被无损归流到「桌面」栅栏），文案里说明这一点以免用户恐慌。
///
/// `MessageBoxW` 是模态的，会在本线程派发嵌套消息；调用方 `handle_event` 外层已由
/// `ReentryGuard` 保护，再入事件会被直接丢弃，不会造成 `Runtime` 借用冲突。
pub(crate) fn confirm_delete_fence(rt: &Runtime, title: &str, icon_count: usize) -> bool {
    let text = format!(
        "确定要删除栅栏「{title}」吗？\n\n其中的 {icon_count} 个图标会移回「桌面」栅栏，不会丢失。"
    );
    let flags = MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2; // 默认焦点落在「否」
    modal_box(rt, "删除栅栏", &text, flags) == IDYES
}

/// 「更改文件位置…」前的二次确认。返回 `true` 表示用户确认搬移。
///
/// 该动作是**真搬文件**（复制到新目录 + 删除原位置的副本，见 `file_ops::change_fence_storage`），
/// 不可撤销，故在确有文件要被搬动时收口做确认。
///
/// **只在 `count > 0` 时调用**：没有文件要搬就保持无模态，不给基础操作加摩擦。
pub(crate) fn confirm_move_storage(rt: &Runtime, count: usize, dir: &str) -> bool {
    let text = format!(
        "确定更改该栅栏的文件位置吗？\n\n将把其中的 {count} 个文件移动到：\n{dir}\n\n\
         原位置的副本会被删除，此操作不可撤销。"
    );
    let flags = MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2; // 默认焦点落在「否」
    modal_box(rt, "更改文件位置", &text, flags) == IDYES
}

/// 目标目录不可用时的告警框（只有一个「确定」）。
///
/// 此前这类拒绝只写一行日志：用户选完文件夹、界面毫无反应，无法分辨"没生效"还是"没点中"。
pub(crate) fn warn_storage_reject(rt: &Runtime, reason: &str) {
    let text = format!("无法把文件位置设置为该文件夹。\n\n原因：{reason}");
    modal_box(rt, "更改文件位置", &text, MB_OK | MB_ICONWARNING);
}

/// 模态弹框的统一外壳：借焦点代理把本线程提到前台 → 弹框 → 把前台还给弹出前的窗口。
///
/// 常驻后台的进程直接弹框会被系统拒绝前台化（对话框不获焦、可能被前台窗口盖住），
/// 故所有模态框都必须走这里——与 `track_popup_menu` 同一套手法。
/// owner 用 overlay 本体而**不是**代理：代理是离屏 1×1，拿它当 owner 会把对话框
/// 居中到 (-32000,-32000) 屏幕外。
pub(crate) fn modal_box(
    rt: &Runtime,
    caption: &str,
    text: &str,
    flags: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    unsafe {
        // 顺序不能换：**先**记下弹出前的前台窗口，**再**提权。反过来的话提权已经把前台
        // 交给了我们的隐藏代理，`prev` 会被记成代理，弹完就还原不回去了。
        let prev = GetForegroundWindow();
        let proxy = (*rt.overlay_ptr).menu_owner();
        // 提权就发生在这里，**不要再挪回调用方**：本函数存在的唯一理由就是替所有模态框
        // 做掉这一步，漏掉任何一处调用方就会重新出现「确认框不获焦、被盖住」的老问题。
        (*rt.overlay_ptr).raise_to_foreground();
        let text = wide(text);
        let caption = wide(caption);
        let result = MessageBoxW(
            Some(rt.hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            flags,
        );
        // 与 `track_popup_menu` 一致：前台若仍停在我们自己的窗口上就还给弹出前的窗口，
        // 免得用户原先的窗口一直灰着；用户已经切到别处则不去抢。
        let fg = GetForegroundWindow();
        if !prev.is_invalid() && (fg == rt.hwnd || fg == proxy) {
            let _ = SetForegroundWindow(prev);
        }
        result
    }
}

/// 把字符串转成 UTF-16（含结尾 NUL），供 Win32 宽字符 API 使用。
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
