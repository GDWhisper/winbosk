//! overlay 窗口：挂在桌面壳层下的透明子窗口，承载 WinRT 合成视觉树。
//!
//! - 覆盖整个虚拟屏幕（多显示器），位置随父窗口屏幕坐标实时计算；
//! - `WS_EX_NOACTIVATE` 不抢焦点；窗口内容完全由 WinRT `Windows.UI.Composition`
//!   视觉树提供（`WS_EX_NOREDIRECTIONBITMAP` 无重定向位图——合成器直连窗口，
//!   `CreateDesktopWindowTarget` 才可用，且 BackdropBrush 才能采样到窗口背后
//!   真实的桌面，这是真·实时模糊的前置要求）；
//! - **点击穿透**：窗口区域被 `SetWindowRgn` 裁剪为全部栅栏矩形的并集
//!   （`CombineRgn(RGN_OR)`）。区域外的鼠标命中直接落到下方窗口（桌面/其他应用）。
//!   不用 `WM_NCHITTEST` 返回 `HTTRANSPARENT`——它只能把点击转发给**同一进程**
//!   的窗口，而 Explorer 桌面是不同进程，全屏 overlay 会变成点击死区；
//! - 交互：标题栏拖动栅栏、右下角手柄缩放、双击图标打开（窗口类需 `CS_DBLCLKS`）。
//!   事件以 `OverlayEvent` 交给 App 层回调处理；回调返回新的命中模型，窗口据此
//!   重建区域并更新命中数据（拖动过程每次移动都重建区域，栅栏随之移动/缩放）；
//! - 窗口状态（命中模型 / 拖拽状态 / 事件回调）保存在 `GWLP_USERDATA`，
//!   由 `OverlayWindow` 独占生命周期，同一线程读写，无需跨线程同步。

use std::sync::OnceLock;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, TRUE, WAIT_FAILED,
    WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, EqualRgn, SetBkColor, SetTextColor,
    SetWindowRgn, HBRUSH, HDC, HRGN, RGN_COPY, RGN_ERROR, RGN_OR,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, CancelWaitableTimer, CreateWaitableTimerExW, GetCurrentThreadId,
    SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
    TIMER_MODIFY_STATE,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, GCS_COMPSTR, GCS_RESULTSTR,
    IME_COMPOSITION_STRING,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture, SetFocus,
    TrackMouseEvent, UnregisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, Shell_NotifyIconW, HDROP,
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetAncestor, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindow, GetWindowLongPtrW,
    GetWindowThreadProcessId, KillTimer, LoadCursorW, LoadIconW, PeekMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, SendMessageW, SetCursor, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, CS_DBLCLKS, GA_ROOT,
    GWLP_USERDATA, GW_HWNDPREV, HCURSOR, HICON, HTCLIENT, HTTRANSPARENT, HWND_TOP, ICON_BIG,
    ICON_SMALL, IDC_ARROW, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, MSG,
    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, PM_REMOVE, QS_ALLINPUT, SET_WINDOW_POS_FLAGS,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOREDRAW, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNA,
    SW_SHOWNOACTIVATE, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_CTLCOLOREDIT, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_DROPFILES, WM_ERASEBKGND, WM_HOTKEY, WM_IME_COMPOSITION,
    WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KILLFOCUS,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST,
    WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETICON, WM_SYSKEYDOWN, WM_TIMER,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_POPUP,
};

use winbosk_core::hotkey::HotkeyAction;
use winbosk_core::model::{CategoryPreset, FenceLayout, FenceStyle, SidebarPosition};

/// 托盘左键单击去重用：上次派发 `TrayToggle` 的时刻（Unix 纪元毫秒）。
///
/// 双击托盘会派发两次 `WM_LBUTTONUP`（序列：UP → DBLCLK → UP），不去重就变成
/// 「打开又立刻关上」白闪一次。以系统双击时限做间隔闸门，双击只算一次点击。
static LAST_TRAY_CLICK_MS: AtomicU64 = AtomicU64::new(0);

/// 当前时刻（Unix 纪元毫秒）；时钟异常（早于纪元）时退回 0。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 窗口类名（全局唯一，单实例）。
const CLASS_NAME: &str = "WinBoskOverlay";
/// 隐藏焦点代理窗口类名（独立顶层，离屏 1×1，DefWindowProc 处理即可）。
const PROXY_CLASS: &str = "WinBoskFocusProxy";

/// 外部通知主循环退出的消息（WM_APP + 1）。
/// 由 `run_message_loop` 的调用方决定在退出前恢复现场（如恢复真实桌面图标）。
pub const WM_APP_QUIT: u32 = 0x8000 + 1;

/// App 层注入一个 `OverlayEvent`（WM_APP + 2）。`lParam` 指向一个
/// `Box<OverlayEvent>`（发送方 `Box::into_raw`，接收方 `Box::from_raw` 释放）。
/// 用途：就地重命名提交后，绕过鼠标消息直接触发一次完整重绘 + 命中模型重建。
pub const WM_WINBOSK_INJECT: u32 = 0x8000 + 2;

/// 托盘图标回调消息（WM_APP + 3）：`lParam` 为托盘鼠标消息（WM_RBUTTONUP 等）。
pub const WM_TRAY: u32 = 0x8000 + 3;

/// 一帧动画节拍到点的自投递消息（WM_APP + 4）。
///
/// 不走 `WM_TIMER`：`SetTimer` 的到期时刻会被吸附到系统 ~15.6ms 的定时器栅格，
/// 请求 16ms 实测得到的是 15/30ms 交替的节拍（240ms 的补间只有 10 帧，肉眼可见
/// 一跳一跳）。改由 `run_message_loop` 等待**高分辨率可等待定时器**，到点后投递
/// 本消息给窗口过程——实测节拍稳定 16ms、零抖动，且不依赖 `timeBeginPeriod`
/// （Win11 对「被遮挡进程」会无视它，而本窗口常驻桌面层、天然被上层窗口盖住）。
/// 投递而非在循环里直接回调，是为了复用 `WM_TIMER` 那条路径上的再入保护与
/// 模态丢弃语义（见 `set_event_handler` 注释）。
pub const WM_APP_ANIM_TICK: u32 = 0x8000 + 4;

/// 托盘图标 ID（进程内唯一）。
const TRAY_ID: u32 = 1;

/// 系统热键 ID 常量。
pub const HOTKEY_QUIT: i32 = 1;
pub const HOTKEY_CONSOLE: i32 = 2;
pub const HOTKEY_DESKTOP: i32 = 3;
pub const HOTKEY_AUTO_ORGANIZE: i32 = 4;

/// 库同步定时器 ID：周期触发 `SyncLibrary`，App 检查库文件夹中已被外部删除的
/// 文件，同步移除栅栏对应项（「库内删除 → 栅栏项消失」）。
const SYNC_LIBRARY_TIMER: usize = 0x5311;
/// 库同步间隔（毫秒）。
const SYNC_LIBRARY_MS: u32 = 4000;

/// 动画帧间隔（毫秒，≈60fps）。
const ANIM_MS: u32 = 16;
/// 兜底时钟（`SetTimer`/`WM_TIMER`）的定时器 ID，仅在拿不到可等待定时器时使用。
const ANIM_TIMER_FALLBACK: usize = 0x5313;

/// 右下角缩放手柄尺寸（物理像素）。App 层用同样数值生成手柄命中区域。
pub const GRIP_SIZE: f32 = 26.0;
/// 左/右/下边缘的缩放松动距离（物理像素），拖边缘即可改宽/改高。
pub const EDGE_RESIZE: f32 = 9.0;
/// 非手柄角的缩放松动距离（物理像素，左下/右上角）。
pub const CORNER_RESIZE: f32 = 12.0;
/// 按下与松开间鼠标位移小于该值即视为「单击」（选中图标，如资源管理器）；
/// 超过则视为拖动（移动栅栏）。
pub const CLICK_DRAG_THRESHOLD: f32 = 5.0;
/// 侧边栏图标拖动排序的启动阈值（像素）：按下后移动超过此距离才进入 reorder 模式。
const REORDER_THRESHOLD: f32 = 8.0;

/// 类只注册一次（同一 HINSTANCE）。
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();
static PROXY_CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// 动画节拍定时器句柄（`CreateWaitableTimerExW` 高分辨率定时器）。
///
/// 进程内只有一个 overlay（单实例互斥），故用静态保存，供 `run_message_loop`
/// 在等待消息时一并等待。`0` = 尚未创建；创建后长期复用，只做 arm/cancel。
static ANIM_TIMER_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// 节拍定时器是否已武装（arm）。为 false 时消息循环回到纯阻塞 `GetMessageW`。
static ANIM_TIMER_ARMED: AtomicBool = AtomicBool::new(false);
/// 节拍时钟是否处于 `SetTimer`/`WM_TIMER` 兜底态（拆除时要 `KillTimer` 而非 cancel）。
static ANIM_CLOCK_FALLBACK: AtomicBool = AtomicBool::new(false);
/// overlay 主窗口句柄：`run_message_loop` 投递 `WM_APP_ANIM_TICK` 用。
static OVERLAY_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// 矩形（虚拟屏幕坐标，物理像素）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    /// 点是否在矩形内（含边界）。
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// 一个栅栏的命中数据：整体矩形（区域 + 兜底命中）、标题栏（移动把手）、
/// 右下角手柄（缩放把手）。`id` 是 App 层 `desk.fences` 的下标。
#[derive(Debug, Clone, Copy)]
pub struct FenceHit {
    pub body: RectF,
    pub title: RectF,
    pub grip: RectF,
    pub id: usize,
    /// 侧边栏悬停工具提示矩形（可延伸到栅栏之外）；None = 无。用于把工具提示
    /// 区域并入窗口区域，否则区域外的绘制不可见。
    pub tooltip: Option<RectF>,
    /// 是否为侧边栏布局（用于拖动排序判定）。
    pub is_sidebar: bool,
    /// 标题栏折叠/展开切换按钮矩形（物理像素）。
    pub collapse_btn: Option<RectF>,
    /// 是否处于折叠状态（折叠时禁止缩放）。
    pub collapsed: bool,
}

/// 一个图标的命中数据。`fence` / `icon` 分别是 App 层 `desk.fences` 下标
/// 与该栅栏 `icon_ids` 下标（与场景中图标的排列一一对应）。
#[derive(Debug, Clone, Copy)]
pub struct IconHit {
    pub rect: RectF,
    pub fence: usize,
    pub icon: usize,
}

/// 控制台面板内可点击的控件。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsoleZone {
    /// 关闭控制台面板（折叠为胶囊，不销毁）。
    Close,
    /// 点击折叠胶囊 / 空白：展开面板。
    Expand,
    /// 切换到第 `index` 个标签页。
    Tab(usize),
    /// 标题栏「切换桌面」按钮：栅栏 ⇄ 原始桌面。
    DesktopToggle,
    /// 底部「开机自启」切换按钮。
    AutostartToggle,
    /// 栅栏管理页：选中第 `index` 个栅栏显示详情控制。
    FenceSelect(usize),
    FenceLayout(FenceLayout),
    FenceIconSize(f32),
    FenceStyle(FenceStyle),
    /// 栅栏管理页：设置侧边栏停靠位置（仅 Sidebar 布局有效）。
    FenceSidebarPos(SidebarPosition),
    FenceTint(Option<[f32; 3]>),
    /// 栅栏管理页：「添加栅栏」按钮（新建一个空白栅栏并选中）。
    AddFence,
    /// 栅栏管理页：移出当前选中的栅栏。
    RemoveFence,
    /// 栅栏管理页：更改选中栅栏的存储位置（打开文件夹选择器）。
    ChangeStoragePath,
    /// 栅栏管理页：在资源管理器中打开选中栅栏的真实落地目录。
    OpenStoragePath,
    /// 栅栏管理页：把选中栅栏的存储位置恢复为应用内部库（解除外部文件夹链接）。
    ResetStoragePath,
    /// 栅栏管理页：设置选中栅栏的分类规则（预设模板；None = 无规则）。
    FenceRulePreset(Option<CategoryPreset>),
    /// 栅栏管理页：「一键整理」按钮（按各栅栏规则自动收纳桌面图标）。
    AutoOrganize,
    /// 控制中心：切换简化/高级模式。
    ToggleAdvancedMode,
    /// 规则总开关：开启/禁用当前栅栏规则。
    RuleToggleEnabled,
    /// 规则编辑：添加后缀白名单（弹出内联编辑）。
    RuleAddExtension,
    /// 规则编辑：删除第 idx 个后缀白名单。
    RuleDeleteExtension(usize),
    /// 规则编辑：添加排除后缀黑名单。
    RuleAddExcludeExtension,
    /// 规则编辑：删除第 idx 个排除后缀黑名单。
    RuleDeleteExcludeExtension(usize),
    /// 规则编辑：添加文件名通配符模式。
    RuleAddPattern,
    /// 规则编辑：删除第 idx 个文件名通配符模式。
    RuleDeletePattern(usize),
    /// 规则编辑：切换自动捕获新文件开关。
    RuleToggleAutoCapture,
    /// 规则编辑：针对当前栅栏立即应用规则重新整理。
    RuleApplyFence,
    /// 控制中心：切换设置页面与栅栏管理页面。
    ToggleSettingsPage,
    /// 控制中心设置页：开始录制指定动作的快捷键。
    HotkeyRecord(HotkeyAction),
    /// 控制中心设置页：清除指定动作的快捷键。
    HotkeyClear(HotkeyAction),
    /// 控制中心设置页：恢复默认快捷键。
    HotkeyResetDefault,
}

/// 控制台（插件面板）的命中数据：整体矩形（窗口区域 + 命中判定范围）、
/// 标题栏（拖动把手）、各控件矩形。
#[derive(Debug, Clone)]
pub struct ConsoleHit {
    /// 整个面板矩形（区域并集 + 命中范围；点在此矩形外不视为控制台点击）。
    pub rect: RectF,
    /// 标题栏（拖动控制台移动）。
    pub title: RectF,
    /// 控件矩形列表（与命中测试顺序一致，先命中的生效）。
    pub zones: Vec<(ConsoleZone, RectF)>,
}

/// 命中模型：窗口区域 + 交互命中测试的数据源。
#[derive(Debug, Clone, Default)]
pub struct HitModel {
    pub fences: Vec<FenceHit>,
    pub icons: Vec<IconHit>,
    /// 控制台面板；None = 未打开。
    pub console: Option<ConsoleHit>,
    /// 当前内联编辑框矩形（浮于栅栏之上）：点击内部用于定位光标，不再落到
    /// 下面的栅栏/图标（否则会误触发「点击别处提交编辑」）。
    pub edit_rect: Option<RectF>,
    /// 收起栅栏的「原大小占位框」矩形：**只并入窗口区域**（`SetWindowRgn`），
    /// 绝不参与命中判定——区域外既不渲染也不接收事件，不并入就画不出来；
    /// 而并入又意味着该区域短暂不再穿透，故仅拖动期间存在（见 App 层 `drag_hint`）。
    pub reserved: Vec<RectF>,
}

/// 缩放拖拽所作用的栅栏区域（决定改宽 / 改高 / 是否随动左上角）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeZone {
    /// 右边缘：只改宽度。
    Right,
    /// 下边缘：只改高度。
    Bottom,
    /// 右下角：宽高一起改。
    BottomRight,
    /// 左边缘：改宽度并随动 x。
    Left,
    /// 左下角：宽高一起改并随动 x。
    BottomLeft,
    /// 右上角：宽高一起改并随动 y。
    TopRight,
}

/// 用户交互事件（坐标全部为虚拟屏幕物理像素）。
#[derive(Debug, Clone)]
pub enum OverlayEvent {
    /// 拖动标题栏移动栅栏：目标位置（左上角）。
    FenceMove { fence: usize, pos: (f32, f32) },
    /// 拖边缘/角标缩放：目标矩形（左上角 x, y 与新宽高 w, h）与作用的边。
    FenceResize {
        fence: usize,
        zone: ResizeZone,
        rect: (f32, f32, f32, f32),
    },
    /// 一次拖动结束（App 在此持久化布局）。
    FenceDragEnd { fence: usize },
    /// 双击栅栏内的图标。
    IconDoubleClicked { fence: usize, icon: usize },
    /// 单击栅栏内的图标（按下+松开且位移很小）：用于选中高亮，如资源管理器。
    /// `ctrl` = 是否按住 Ctrl（按住时切换该图标的选择状态，实现不连续多选）。
    IconClicked {
        fence: usize,
        icon: usize,
        ctrl: bool,
    },
    /// 框选拖拽（橡皮筋）：`rect` 是当前框选矩形，`selected` 是框中的图标（拖动中逐帧上报）。
    SelectDrag {
        fence: usize,
        rect: (f32, f32, f32, f32),
        selected: Vec<(usize, usize)>,
    },
    /// 框选拖拽结束：清除橡皮筋显示（选择结果已在最后一次 `SelectDrag` 中生效）。
    SelectEnd,
    /// 就地重命名提交后由 App 注入：仅用于触发一次完整重绘 + 命中模型重建
    /// （场景数据已在注入前改好）。
    EditCommitted,
    /// 鼠标点在编辑框内：`x` 为虚拟屏幕物理坐标，App 据此把光标定位到对应字符。
    /// 点击编辑框不再穿透到下面的栅栏/图标（避免误触发「点击别处提交」）。
    EditCaret { x: f32 },
    /// 右键按下：`icon` 为 Some 表示点在图标的图标上，None 表示点在栅栏空白/标题上。
    /// `pos` 为虚拟屏幕坐标（App 层据此弹上下文菜单）。
    ContextMenu {
        fence: usize,
        icon: Option<usize>,
        pos: (f32, f32),
    },
    /// 鼠标悬停到某个图标（用于高亮反馈）。
    HoverEnter { fence: usize, icon: usize },
    /// 鼠标移出所有图标（清除高亮）。
    HoverLeave,
    /// 光标位置变化（虚拟屏幕物理坐标）：App 层据此做连续 Dock 放大，
    /// 位置未变时不上报（避免无谓重绘）。
    CursorMove { x: f32, y: f32 },
    /// 光标离开窗口（`WM_MOUSELEAVE`）：App 层清除 Dock 放大（恢复 1.0）。
    CursorLeave,
    /// 文件被拖入某个栅栏（`WM_DROPFILES`）：App 把这些路径加进该栅栏。
    FilesDropped { fence: usize, paths: Vec<String> },
    /// 鼠标滚轮滚动某栅栏：`delta` 是滚轮原始刻度（正=向上/远离，负=向下）。
    FenceScroll { fence: usize, delta: i32 },
    /// 点击栅栏折叠切换按钮。
    FenceCollapseToggle { fence: usize },
    /// 双击栅栏标题栏空白区。
    FenceTitleDoubleClicked { fence: usize },
    /// 托盘图标：右键（显示控制中心/退出菜单）。
    TrayMenu,
    /// 托盘图标：左键单击（切换控制中心开合）。
    TrayToggle,
    /// 控制台控件悬停变化（None = 移出所有控件；仅展开面板内上报）。
    ConsoleHover { zone: Option<ConsoleZone> },
    /// 控制台（插件面板）内点击某个控件。
    ConsoleClick { zone: ConsoleZone },
    /// 在控制台面板上滚动滚轮（待办列表滚动）：`delta` 是滚轮原始刻度。
    ConsoleScroll { delta: i32 },
    /// 拖动控制台标题栏：目标左上角（虚拟屏幕坐标）。
    ConsoleMove { pos: (f32, f32) },
    /// 控制台拖动结束（App 在此持久化位置）。
    ConsoleDragEnd,
    /// 拖控制台右/下边缘或右下角：目标矩形（宽高随动，左上角锚定不变）。
    ConsoleResize { rect: (f32, f32, f32, f32) },
    /// 控制台缩放拖拽结束（App 在此持久化尺寸）。
    ConsoleResizeEnd,
    /// 全局热键：切换控制台开关。
    ConsoleToggle,
    /// 全局热键：在原生桌面与栅栏之间切换。
    DesktopToggle,
    /// 全局热键：一键整理桌面图标。
    AutoOrganize,
    /// 键盘按下（overlay 获得焦点时）：`vk` 虚拟键码，各修饰键按下状态。
    /// 文本字符走 `Char`（经 TranslateMessage 转换）。
    KeyDown {
        vk: u32,
        ctrl: bool,
        shift: bool,
        alt: bool,
        win: bool,
    },
    /// 文本字符（非 IME 合成路径的普通输入）。
    Char { ch: u16 },
    /// IME 开始合成。
    ImeStart,
    /// IME 合成更新：`text` 为当前合成串，`caret` 为合成串内光标。
    ImeCompose { text: String, caret: usize },
    /// IME 合成结果上屏（最终提交的文本）。
    ImeResult { text: String },
    /// IME 合成结束（上屏）。
    ImeEnd,
    /// overlay 失去键盘焦点（内联编辑应取消聚焦但不丢文本）。
    OverlayFocusLost,
    /// 定时器周期触发：App 检查内部库文件夹，删除的库文件对应栅栏项同步移除。
    SyncLibrary,
    /// 动画定时器触发（ANIM_TIMER，16ms）：App 推进控制台面板/待办行动画。
    /// 无动画进行时 App 调用 `OverlayWindow::set_anim_active(false)` 停止本定时器。
    AnimTick,
    /// 所在显示器 DPI 变化（`WM_DPICHANGED`）：`dpi` 为新 x-DPI。
    /// App 据此重算主题缩放并重新夹屏 + 重绘（Per-Monitor v2 下的实时跟随）。
    DpiChanged { dpi: u32 },
    /// 显示拓扑/分辨率变化（`WM_DISPLAYCHANGE`）：虚拟屏宽高可能已变。
    /// App 据此重算虚拟屏尺寸并重新夹屏 + 重绘（找回超屏栅栏）。
    DisplayChange,
    /// 侧边栏图标拖动中（重排序）：`icon` 是被拖动的图标在 `icon_ids` 中的下标，
    /// `(mx, my)` 为当前光标位置（虚拟屏幕物理像素）。
    SidebarReorderDrag {
        fence: usize,
        icon: usize,
        mx: f32,
        my: f32,
    },
    /// 侧边栏图标拖动结束：`from` 是原始下标，`to` 是目标插入位置。
    SidebarReorderEnd {
        fence: usize,
        from: usize,
        to: usize,
    },
}

/// 拖拽会话（按下到松开之间持续有效）。
#[derive(Debug, Clone, Copy)]
struct DragState {
    kind: DragKind,
    fence: usize,
    /// 按下时的鼠标位置（虚拟屏幕坐标）；原始目标 = `orig + (鼠标 - start)`。
    start: (f32, f32),
    /// 按下时栅栏的整体矩形（吸附/碰撞的基准原点）。
    orig: RectF,
    /// 按下点是否落在某图标上（该栅栏内图标下标）；用于单击选中判定。
    pressed_icon: Option<usize>,
    /// 按下时是否按住 Ctrl（单击时决定「切换选择」还是「单选」）。
    ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragKind {
    /// 标题栏/图标拖动 → 移动栅栏。
    Move,
    /// 空白处拖动 → 框选（橡皮筋多选，资源管理器行为）。
    Select,
    /// 边缘/角标拖动 → 缩放（记录作用于哪个区域）。
    Resize(ResizeZone),
    /// 控制台标题栏拖动 → 移动控制台面板。
    ConsoleMove,
    /// 控制台右/下边缘或右下角拖动 → 缩放面板（锚定左上角）。
    ConsoleResize(ResizeZone),
    /// 侧边栏图标拖动 → 重新排序。
    SidebarReorder,
}

struct WindowState {
    model: HitModel,
    drag: Option<DragState>,
    /// 当前悬停目标（栅栏下标, 图标下标）；仅在变化时上报 Hover 事件。
    hovered: Option<(usize, Option<usize>)>,
    /// 当前悬停的控制台控件（仅在变化时上报 ConsoleHover 事件）。
    console_hovered: Option<ConsoleZone>,
    /// 上一次「按下」是否被控制台面板整体消费（命中控件 / 标题栏 / 面板空白均算）。
    ///
    /// 双击处理以它为准，而不是「当前坐标是否还在面板矩形内」：点「关闭」会把面板折成
    /// 胶囊乃至完全不渲染，此时按坐标判定会失效并放行穿透；拖动面板也会让它从光标下移开。
    last_press_in_console: bool,
    /// 上一次「按下」在控制台面板上命中的控件（未命中控件则为 None）。
    ///
    /// 快速连击时 Windows 会把第二次按下变成 `WM_LBUTTONDBLCLK`，需要判断它是否仍落在
    /// **同一个**控件上才允许重复触发：控制台的行数会随选中栅栏/布局动态增减，一旦发生
    /// 回流，同一坐标可能已经换成别的控件（例如点「列表」后下方整排上移），此时必须拒绝，
    /// 否则会误触到相邻控件。
    last_console_zone: Option<ConsoleZone>,
    /// 上次上报的光标位置（仅位置变化才发 CursorMove，避免无谓重绘）。
    last_cursor: Option<(f32, f32)>,
    /// 上次 SetWindowRgn 的区域句柄：区域几何未变时跳过 SetWindowRgn。
    ///
    /// 即便 `bRedraw=false`，每次调用仍有区域对象创建/销毁与 DWM 形状通知开销；
    /// Dock 存在时光标移动每次都会 set_model 到这里——反复调用造成无谓 CPU。
    /// 区域句柄所有权归窗口（系统释放），此处仅保存引用用于 EqualRgn。
    last_region: Option<HRGN>,
    /// App 层事件处理器；返回新的命中模型以同步区域与命中数据。
    ///
    /// 返回 `None` 表示丢弃本事件、保持当前命中模型：App 在模态菜单 / 属性页等
    /// 嵌套消息循环期间会再入本回调（定时器、悬停、注入事件），此时 `&mut Runtime`
    /// 借用已被外层占用，再入必然借用冲突崩溃，必须让事件静默丢弃。
    handler: Option<Box<dyn FnMut(OverlayEvent) -> Option<HitModel>>>,
    /// 编辑框（重命名/待办输入）背景画刷：`WM_CTLCOLOREDIT` 返回它让 EDIT 用深色底。
    edit_brush: HBRUSH,
    /// create 时的父/owner 窗口（与 `OverlayWindow.owner` 同源、不可变）：wnd_proc 处理
    /// `WM_MOUSELEAVE` 回落桌面带时需要它计算「owner 正上方」锚点，而 wnd_proc 拿不到
    /// OverlayWindow 本体。
    owner: HWND,
    /// 控制中心会话是否激活（业务位，由 app 的 `set_console_open` 单一出口推送）。
    ///
    /// 会话激活期间光标离开窗口不回落桌面带（生命周期由开/关配对管理）；非会话期的
    /// 点击唤起（点栅栏临时用一下）随光标离开表面而结束。
    console_session: bool,
}

/// overlay 窗口。
pub struct OverlayWindow {
    pub hwnd: HWND,
    /// 隐藏的焦点代理窗口（独立顶层，离屏 1×1）：内联编辑需要键盘焦点，
    /// 但 overlay 必须保持 `WS_EX_NOACTIVATE`（激活它会把桌面壳层提到应用之上）。
    /// 聚焦代理只负责「成为前台进程」，键盘焦点仍给 overlay。
    proxy: HWND,
    /// 窗口在虚拟屏幕上的左上角（物理像素；可含负值，如副屏在主屏左/上方时）。
    pub x: i32,
    pub y: i32,
    /// 覆盖的虚拟屏幕尺寸（物理像素）。
    pub width: u32,
    pub height: u32,
    /// create 时的父/owner 窗口（持有 DefView 的 WorkerW，无 DefView 时为 Progman）。
    /// 仅用于收起控制中心时把窗口插回 owner 之后、落回桌面带；overlay 与 owner 的
    /// 生命周期被所有权机制绑定（owner 销毁则 owned overlay 一并销毁），句柄不会悬空。
    owner: HWND,
    state: *mut WindowState,
}

/// 「只动 Z 序」的 `SetWindowPos` flags：提权（`HWND_TOP`）与回落桌面带（锚点插入）共用。
/// **绝不能加 `SWP_NOZORDER`**——它会忽略 `hWndInsertAfter`，两个方向的锚点都会失效
/// （提权纹丝不动、回落落不进桌面带）。以内部 `u32` 聚合而非 `|` 运算符：windows-rs 的
/// `BitOr` 不是 const fn，无法在 `const` 上下文调用（E0015），值与 `|` 写法完全一致。
const SWP_ZORDER_ONLY: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(
    SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0 | SWP_NOREDRAW.0 | SWP_NOOWNERZORDER.0,
);

/// 把窗口提到普通 Z 序带顶部（用户前台会话的提权）。
///
/// overlay 以 WorkerW 为 owner 锚在桌面带，默认永远低于普通应用窗口；用户主动操作
/// （唤出控制中心、点击栅栏/面板表面）时需要临时浮于其上（盖住浏览器等普通窗口，
/// 仍低于真正的 TOPMOST 窗口）。
///
/// 必须经前台锁（AttachThreadInput）：本进程常驻后台，裸调 `SetWindowPos(HWND_TOP)`
/// 会被系统静默拒绝（返回 TRUE 但 Z 序纹丝不动，已实测）——与 `raise_to_foreground`
/// 同一套手法。只动 Z 序：`SWP_NOACTIVATE` 不激活——激活会把桌面壳层提到应用之上，
/// 且系统不接受激活（键盘输入走隐藏焦点代理）；几何与 owner 层级一律不动。
/// 已在顶部时为无害空操作（不缓存「已提权」标志：被其它程序盖住时标志必然失真）。
fn raise_hwnd_to_normal_top(hwnd: HWND) {
    with_foreground_lock(|| unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_ZORDER_ONLY);
    });
}

/// 把窗口插回 owner（WorkerW）**正上方**，落回桌面带（用户前台会话结束）。
///
/// 注意 `SetWindowPos` 的 `hWndInsertAfter` 语义是「被定位窗口插到该窗口**之下**」：
/// 直接传 owner 会把窗口插到 WorkerW 之下、壁纸之后（栅栏整体不可见不可点，已实测），
/// 因此锚点必须取「owner 当前正上方的窗口」——把窗口恰好放进 owner 之上刚空出的位置，
/// 与壁纸/图标/其它 WorkerW 的相对次序无关。owner 可能是子窗口（`SHELLDLL_DefView`），
/// 先 `GA_ROOT` 归一到顶层域再取其上一窗；owner 之上若无窗口（理论边界），跳过恢复
/// 保持现状（留在普通带优于误落到壁纸之后）。已在桌面带时锚点即窗口自身，无害空操作。
/// owner 若已销毁，所有权机制会连带销毁 owned 窗口，不存在悬空句柄的运行态；
/// `SetWindowPos` 失败仅忽略（容错口径与 `resize` 一致）。
fn restore_hwnd_desktop_band(hwnd: HWND, owner: HWND) {
    unsafe {
        let root = GetAncestor(owner, GA_ROOT);
        let Ok(anchor) = GetWindow(root, GW_HWNDPREV) else {
            return;
        };
        let _ = SetWindowPos(hwnd, Some(anchor), 0, 0, 0, 0, SWP_ZORDER_ONLY);
    }
}

/// 在「本线程已挂到前台线程」的保护下执行 `f`（前台锁）。
///
/// 非前台进程的 `SetForegroundWindow` / `SetWindowPos(HWND_TOP)` 会被系统直接拒绝或
/// 静默忽略。经典解法是把本线程暂时挂到当前前台线程，设完再解挂——无需真正激活
/// overlay，桌面壳层仍保持在应用之下。
fn with_foreground_lock(f: impl FnOnce()) {
    unsafe {
        let cur = GetCurrentThreadId();
        let fg_hwnd = GetForegroundWindow();
        let fg = GetWindowThreadProcessId(fg_hwnd, None);
        let attached = cur != fg && fg != 0 && AttachThreadInput(cur, fg, true).0 != 0;
        f();
        if attached {
            let _ = AttachThreadInput(cur, fg, false);
        }
    }
}

impl OverlayWindow {
    /// 让 overlay 获得键盘输入（前台交给隐藏代理，overlay 本体不被激活/提层）。
    pub fn focus_for_input(&self) {
        self.with_foreground_lock(|| unsafe {
            let _ = SetForegroundWindow(self.proxy);
            let _ = SetFocus(Some(self.hwnd));
        });
    }

    /// 把本线程提到前台（前台交给隐藏焦点代理），供随后弹出的模态 UI 使用。
    ///
    /// WinBosk 常驻后台，进程通常不是前台进程；此时直接弹 `TrackPopupMenu` / `MessageBoxW`
    /// 会被系统拒绝前台化——菜单点外部不关闭，对话框不获焦、被前台窗口盖住。
    /// 任何需要「把某个窗口提到前台」的场景都必须走它。
    pub fn raise_to_foreground(&self) {
        self.with_foreground_lock(|| unsafe {
            let _ = SetForegroundWindow(self.proxy);
        });
    }

    /// 控制中心会话开始：把 overlay 从桌面带临时提升到普通 Z 序带顶部（`HWND_TOP`）。
    ///
    /// 仅 app 的 `set_console_open` 单一出口调用；点击唤起走 wnd_proc 内的
    /// `raise_hwnd_to_normal_top`（约束与实测依据见该函数注释）。收起时必须配对调用
    /// `restore_desktop_band`。
    pub fn raise_console(&self) {
        raise_hwnd_to_normal_top(self.hwnd);
    }

    /// 控制中心会话结束：把 overlay 插回 owner（WorkerW）**正上方**，落回桌面带。
    ///
    /// 仅 app 的 `set_console_open` 单一出口调用；锚点语义与实测依据见
    /// `restore_hwnd_desktop_band` 注释。
    pub fn restore_desktop_band(&self) {
        restore_hwnd_desktop_band(self.hwnd, self.owner);
    }

    /// 推送控制中心会话位（业务状态，仅 app 的 `set_console_open` 单一出口调用）。
    ///
    /// 会话激活期间 `WM_MOUSELEAVE` 不回落桌面带（生命周期由开/关配对管理）；
    /// 非会话期的点击唤起随光标离开表面回落。
    pub fn set_console_session(&self, active: bool) {
        unsafe { (*self.state).console_session = active };
    }

    /// 弹出菜单要用的 owner 窗口（可激活的焦点代理），并已把它提到前台。
    ///
    /// `TrackPopupMenu` 要求 owner 在弹出前**已经**是前台窗口，否则点击菜单外区域
    /// 菜单不会消失（一直悬在屏幕上，只能靠选中某项或 Esc 收场）。overlay 本体带
    /// `WS_EX_NOACTIVATE`——激活它会把桌面壳层提到应用之上，且系统压根不接受激活，
    /// 所以前台与 owner 都交给可激活的焦点代理窗口。
    ///
    /// **不要**拿它当 `MessageBoxW` 的 owner：代理是离屏 1×1（`-32000,-32000`），
    /// 对话框会被居中到屏幕外。对话框应先 `raise_to_foreground()` 抢前台，
    /// owner 仍用 overlay 本体（覆盖整屏，居中位置正常）。
    pub fn menu_owner(&self) -> HWND {
        self.raise_to_foreground();
        self.proxy
    }

    /// 在「本线程已挂到前台线程」的保护下执行 `f`。
    ///
    /// 前台锁：非前台进程的 `SetForegroundWindow` 会被系统直接拒绝。经典解法是把
    /// 本线程暂时挂到当前前台线程，设完前台再解挂——无需真正激活 overlay，
    /// 桌面壳层仍保持在应用之下。
    ///
    /// 任何需要「把某个窗口提到前台」的场景都必须走它：App 层的 Shell 右键菜单
    /// （`shell_menu`）要把自己的宿主窗口前置，否则 `TrackPopupMenu` 弹出的菜单
    /// 点击外部不会关闭。
    pub fn with_foreground_lock(&self, f: impl FnOnce()) {
        with_foreground_lock(f);
    }

    /// 注册底层全局热键（供 App 层统一动态配置）。
    pub fn register_hotkey_raw(
        &self,
        id: i32,
        modifiers: u32,
        vk: u32,
    ) -> windows::core::Result<()> {
        unsafe {
            RegisterHotKey(
                Some(self.hwnd),
                id,
                windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS(modifiers),
                vk,
            )
        }
    }

    /// 注销底层全局热键。
    pub fn unregister_hotkey_raw(&self, id: i32) -> windows::core::Result<()> {
        unsafe { UnregisterHotKey(Some(self.hwnd), id) }
    }

    /// 在桌面壳层下创建覆盖整个虚拟屏幕的 overlay 窗口。
    pub fn create(parent: HWND) -> Result<Self> {
        let hmodule = unsafe { GetModuleHandleW(None)? };
        let hinstance = HINSTANCE(hmodule.0);
        ensure_class(hinstance);

        let (vx, vy, vw, vh) = virtual_screen();

        // 窗口直接定位在虚拟屏原点 (vx, vy)：客户端 (0,0) = 虚拟屏 (0,0)，栅栏的
        // 虚拟屏幕坐标即可直接作为客户端坐标绘制/命中，无需任何换算。
        // 旧版减去 `parent_rect.left/top` 会把窗口挪偏——单屏下 (0,0) 减了无感，
        // 副屏在主屏左/上（vx/vy≠0）时窗口与合成器坐标同时错位，正是多屏栅栏位置
        // 错乱、模糊栅栏对不上边框的根因之一。这里只保留父窗口的 z 序/所有权关系。

        // 编辑框暗色背景画刷（重命名/待办输入共用），与玻璃卡片面板填充色完全一致
        // （面板填充 [0.062,0.086,0.133]≈RGB(16,22,34)），输入框与面板无缝一体，
        // 不再是一块突兀的深色方块；失败时退回空刷，编辑框用系统默认白底。
        let edit_brush = unsafe { CreateSolidBrush(COLORREF(0x00_22_16_10)) }; // RGB(16,22,34)
        let state = Box::new(WindowState {
            model: HitModel::default(),
            drag: None,
            hovered: None,
            console_hovered: None,
            last_press_in_console: false,
            last_console_zone: None,
            last_cursor: None,
            last_region: None,
            handler: None,
            edit_brush,
            owner: parent,
            console_session: false,
        });
        let state_ptr = Box::into_raw(state);

        let hwnd = unsafe {
            CreateWindowExW(
                // `WS_EX_NOACTIVATE`：点击栅栏/小组件不激活窗口、不把桌面壳层提到
                // 应用之上（桌面层级永远在正常应用下面）。键盘输入走隐藏焦点代理。
                // `WS_EX_TOOLWINDOW`：不进任务栏/Alt+Tab——日常入口是托盘图标
                // （默认折叠在通知区隐藏图标里），右键托盘即「WinBosk 控制中心」。
                // `WS_EX_NOREDIRECTIONBITMAP`：无重定向位图——WinRT 合成器直连窗口
                // （CreateDesktopWindowTarget 要求），且 BackdropBrush 才能采样到
                // 窗口背后真实的桌面（真·实时模糊的前置）。
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                PCWSTR(wide(CLASS_NAME).as_ptr()),
                // 窗口标题：任务栏按钮/悬停提示显示「WinBosk」（留空会退回进程名 winbosk.exe）
                PCWSTR(wide("WinBosk").as_ptr()),
                WS_POPUP,
                vx,
                vy,
                vw,
                vh,
                Some(parent),
                None,
                Some(hinstance),
                Some(state_ptr as *const core::ffi::c_void),
            )?
        };
        // 创建完成、消息泵启动前写入状态，wnd_proc 从此刻起可安全读取
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize) };
        OVERLAY_HWND.store(hwnd.0, Ordering::Relaxed);
        // 显式设置任务栏/Alt+Tab 图标（大+小），确保运行中的任务栏按钮显示 WinBosk 图标。
        // LoadIconW 返回共享句柄，无需释放；资源缺失时返回空图标，WM_SETICON 接受空值
        // 并退回默认图标，不会出错。
        let hicon = app_icon(hinstance);
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(hicon.0 as isize)),
            );
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(hicon.0 as isize)),
            );
        }

        // 托盘图标（通知区，默认折叠在「隐藏的图标」里）：右键 = 控制中心菜单，
        // 左键单击 = 切换控制中心开合。日常入口不再占任务栏按钮。
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = hicon;
        let tip = wide("WinBosk 桌面栅栏");
        for (i, c) in tip.iter().take(127).enumerate() {
            nid.szTip[i] = *c;
        }
        let _ = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };

        // 立即用一个空区域：首个栅栏命中模型到来之前，整个窗口对鼠标不可见，
        // 不会出现「全屏死区」。首个模型到达后区域会被替换为栅栏并集。
        let empty = unsafe { CreateRectRgn(0, 0, 0, 0) };
        unsafe { SetWindowRgn(hwnd, Some(empty), false) };

        let _shown = unsafe { ShowWindow(hwnd, SW_SHOWNA) };

        // 隐藏焦点代理：独立顶层窗口（离屏 1×1，不进任务栏），只负责让进程成为前台，
        // 键盘焦点仍给 overlay（NOACTIVATE 本体不会因点击被激活/提层）。
        ensure_proxy_class(hinstance);
        let proxy = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(wide(PROXY_CLASS).as_ptr()),
                PCWSTR(wide("WinBoskFocusProxy").as_ptr()),
                WS_POPUP,
                -32000,
                -32000,
                1,
                1,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };
        unsafe {
            let _ = ShowWindow(proxy, SW_SHOWNOACTIVATE);
        }

        // 接受文件拖放（WM_DROPFILES）：把任意文件/文件夹/快捷方式拖进栅栏。
        // 区域裁剪不拦截拖放——系统按窗口可见区域（region）决定拖放目标，
        // 拖到栅栏并集内命中本窗口，拖到区域外仍落到桌面。
        unsafe { DragAcceptFiles(hwnd, true) };

        // 库同步定时器：周期触发 `SyncLibrary`（见 `WM_TIMER` 处理）
        let _ = unsafe { SetTimer(Some(hwnd), SYNC_LIBRARY_TIMER, SYNC_LIBRARY_MS, None) };

        Ok(Self {
            hwnd,
            proxy,
            x: vx,
            y: vy,
            width: vw as u32,
            height: vh as u32,
            owner: parent,
            state: state_ptr,
        })
    }

    /// 应用命中模型：更新命中数据并把窗口区域裁剪为栅栏并集（区域外点击穿透）。
    pub fn set_model(&self, model: HitModel) {
        let state = unsafe { &mut *self.state };
        state.model = model;
        apply_region(self.hwnd, &state.model, &mut state.last_region);
    }

    /// 设置用户交互事件处理器。回调返回新的命中模型（由 App 根据新布局生成），
    /// overlay 随即更新命中数据与窗口区域。
    pub fn set_event_handler(&self, handler: Box<dyn FnMut(OverlayEvent) -> Option<HitModel>>) {
        unsafe { &mut *self.state }.handler = Some(handler);
    }

    /// 启用/停用动画节拍定时器。App 在启动动画时启用、全部结束后停用，
    /// 保证空闲时 0% CPU。
    ///
    /// 时钟**必须有兜底**：见 [`anim_clock`] ——一旦时钟失效而没有退路，
    /// `AnimTick` 永不再来，App 的补间会永远停在半途、且无从自愈。
    ///
    /// 幂等：已在目标态时直接返回——动画进行中每建一个补间都会 arm 一次，
    /// 不去重会重置周期相位，反而拉长第一个间隔。
    pub fn set_anim_active(&self, active: bool) {
        if ANIM_TIMER_ARMED.load(Ordering::Relaxed) == active {
            return;
        }
        if !active {
            disarm_anim_clock(self.hwnd);
        } else if let Some(h) = anim_clock() {
            // 负 due = 相对时间，单位 100ns；`lPeriod` 让它自动周期性重武装。
            let due: i64 = -(ANIM_MS as i64) * 10_000;
            let rc = unsafe { SetWaitableTimer(h, &due, ANIM_MS as i32, None, None, false) };
            if rc.is_err() {
                tracing::warn!("节拍定时器武装失败（{rc:?}），降级到 SetTimer/WM_TIMER");
                arm_fallback_clock(self.hwnd);
            }
        } else {
            arm_fallback_clock(self.hwnd);
        }
        ANIM_TIMER_ARMED.store(active, Ordering::Relaxed);
    }

    /// 显示拓扑变化后把 overlay 重设到新的虚拟屏幕（移动 + 缩放）。
    /// `(vx, vy)` 为新虚拟屏原点（副屏在左/上时可负），`w×h` 为虚拟屏尺寸。
    /// 客户端坐标始终 = 虚拟屏幕坐标，无需换算（见 `create`）。
    pub fn resize(&mut self, vx: i32, vy: i32, w: u32, h: u32) {
        self.x = vx;
        self.y = vy;
        self.width = w;
        self.height = h;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                vx,
                vy,
                w as i32,
                h as i32,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOREDRAW,
            );
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        // 先销毁窗口（同步触发 WM_DESTROY 及后续消息），再回收状态，
        // 避免窗口销毁期间 wnd_proc 引用已释放的内存。
        unsafe {
            let _ = KillTimer(Some(self.hwnd), SYNC_LIBRARY_TIMER);
        }
        // 走统一入口拆动画节拍定时器（它还要关闭句柄，不只是停表）。
        self.set_anim_active(false);
        OVERLAY_HWND.store(std::ptr::null_mut(), Ordering::Relaxed);
        let h = HANDLE(ANIM_TIMER_HANDLE.swap(std::ptr::null_mut(), Ordering::Relaxed));
        if !h.is_invalid() {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
        let _ = unsafe { DestroyWindow(self.hwnd) };
        let _ = unsafe { DestroyWindow(self.proxy) };
        // 移除托盘图标
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ID;
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
        unsafe {
            let st = Box::from_raw(self.state);
            if !st.edit_brush.0.is_null() {
                let _ = DeleteObject(st.edit_brush.into());
            }
            if let Some(rgn) = st.last_region {
                let _ = DeleteObject(rgn.into());
            }
        }
    }
}

/// 虚拟屏幕范围（屏幕坐标，物理像素）。
fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// 加载 exe 内嵌主图标（资源 ID 1 = winbosk.ico）。`LoadIconW` 返回系统共享图标句柄，
/// 不需要（也不能）手动 `DestroyIcon`；资源缺失时返回空图标，调用方回落默认图标。
fn app_icon(hinstance: HINSTANCE) -> HICON {
    // MAKEINTRESOURCE(1)：资源 ID 1 = winbosk.ico（不是真正的指针，clippy 误报时放行）
    #[allow(clippy::manual_dangling_ptr)]
    unsafe { LoadIconW(Some(hinstance), PCWSTR(1 as *const u16)) }.unwrap_or_default()
}

fn ensure_class(hinstance: HINSTANCE) {
    CLASS_REGISTERED.get_or_init(|| {
        let class_name = wide(CLASS_NAME);
        let wc = WNDCLASSW {
            // CS_DBLCLKS：没有它双击会被拆成两次 WM_LBUTTONDOWN，
            // WM_LBUTTONDBLCLK 永远不会到达。
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            // 主图标 = exe 内嵌的 winbosk.ico（资源 ID 1）。WNDCLASSW 无 hIconSm 字段，
            // 小图标在窗口创建后经 WM_SETICON(ICON_SMALL) 显式设置（任务栏按钮优先取它）。
            hIcon: app_icon(hinstance),
            hCursor: Default::default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
        };
        let _atom = unsafe { RegisterClassW(&wc) };
        // 类重名时返回 0（唯一实例，忽略）
    });
}

/// 注册焦点代理窗口类（纯 DefWindowProc，不接收任何特殊消息）。
fn ensure_proxy_class(hinstance: HINSTANCE) {
    PROXY_CLASS_REGISTERED.get_or_init(|| {
        let class_name = wide(PROXY_CLASS);
        let wc = WNDCLASSW {
            style: Default::default(),
            lpfnWndProc: Some(proxy_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: Default::default(),
            hCursor: Default::default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
        };
        let _atom = unsafe { RegisterClassW(&wc) };
    });
}

/// 焦点代理窗口过程：全部消息交给默认处理（只是前台占位，不处理交互）。
unsafe extern "system" fn proxy_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 把命中模型的栅栏矩形并集设为窗口区域。区域外：
/// - 点击/命中直接落到下方窗口（点击穿透的关键）；
/// - 该区域同时限制窗口的可视范围——场景恰好只画这些栅栏，无可见裁剪。
///
/// `SetWindowRgn` 接管区域所有权：旧区域由系统自动释放，新建的区域交给窗口后
/// 不可再手动删除；中间产生的单矩形区域在合并进结果后立即删除。
///
/// `cached` 保存上次交给窗口的区域句柄（所有权在窗口）。区域几何与上次完全相同时
/// 直接跳过 SetWindowRgn——该调用会让窗口失效、强制重绘/重合成，悬停/光标移动期间
/// 区域几乎总是不变，反复调用只会增加 CPU 与重绘抖动（Dock 场景尤其明显）。
///
/// `bRedraw` **必须是 false**：本窗口是 `WS_EX_NOREDIRECTIONBITMAP` + DirectComposition
/// 视觉树（无 GDI 客户区、无 WM_PAINT 绘制），区域只服务命中穿透与 DWM 可视裁剪。
/// 传 true 会走「整窗失效 → DWM 拆掉再重合成」路径：拖动时区域每帧都变（被拖栅栏
/// 矩形在并集里），**全部栅栏**（不只被拖的那个）都会跟着闪一下，观感就是「刷新了一
/// 遍」。内容更新已由 App 层 `present` + `RequestCommitAsync` 完成，这里不需要系统
/// 再强制重绘。区域几何本身仍立即生效（命中与可视裁剪不依赖 bRedraw）。
///
/// 新暴露区域可见内容的正确性还依赖一条 App 层顺序不变量——**内容提交（present）
/// 必须先于命中模型/区域更新**：首帧路径是 main.rs 先 `compositor.present` 再
/// `overlay.set_model`；事件路径按「App 处理交互 → 重绘 → 返回新命中模型（overlay
/// 据此更新区域）」的契约在 handle_event 内先重绘再返回模型。因此区域扩张瞬间，
/// 新暴露区域依赖的内容已先行提交到 DWM；若未来颠倒该顺序，`bRedraw=false` 会露出
/// 一帧陈旧内容（审查发现的时序风险，已由既有不变量静态关闭，此处落字为证）。
fn apply_region(hwnd: HWND, model: &HitModel, cached: &mut Option<HRGN>) {
    let Some(rgn) = build_region(model) else {
        return;
    };
    if let Some(prev) = cached {
        unsafe {
            if EqualRgn(*prev, rgn) == TRUE {
                // 区域没变：丢弃新句柄，保留旧句柄副本。
                let _ = DeleteObject(rgn.into());
                return;
            }
        }
    }
    unsafe {
        // 创建一份私有副本存入 cached，供后续 EqualRgn 比对。
        // 原因：SetWindowRgn 调用成功后，Windows 内核会接管传入 rgn 的所有权，
        // 并在后续 SetWindowRgn 或窗口销毁时由系统释放该句柄。应用程序绝不能再次
        // 将该句柄传给 GDI 函数查询或重复 DeleteObject。
        let copy = CreateRectRgn(0, 0, 0, 0);
        if CombineRgn(Some(copy), Some(rgn), None, RGN_COPY) != RGN_ERROR {
            if let Some(old) = cached.replace(copy) {
                let _ = DeleteObject(old.into());
            }
        } else {
            let _ = DeleteObject(copy.into());
            if let Some(old) = cached.take() {
                let _ = DeleteObject(old.into());
            }
        }
        SetWindowRgn(hwnd, Some(rgn), false);
    };
}

/// 把全部栅栏矩形 + 侧边栏工具提示 + 内联编辑框 + 占位框 + 控制台面板合并成一个
/// 区域（RGN_OR 并集）。无有效矩形时返回 None。
fn build_region(model: &HitModel) -> Option<HRGN> {
    let mut acc: Option<HRGN> = None;
    for f in &model.fences {
        add_rect(&mut acc, f.body);
        if let Some(tt) = f.tooltip {
            add_rect(&mut acc, tt);
        }
    }
    // 编辑框可能伸出栅栏（如侧边栏贴边时的旁侧编辑框）：并入区域，
    // 否则框内点击穿透到桌面、光标定位收不到。
    if let Some(er) = model.edit_rect {
        add_rect(&mut acc, er);
    }
    // 占位框：大于收起后的可见体，不并入就被区域裁掉（完全看不见）。
    // 它不进入 `model.fences`，故不会产生命中热区。
    for r in &model.reserved {
        add_rect(&mut acc, *r);
    }
    if let Some(c) = &model.console {
        add_rect(&mut acc, c.rect);
    }
    acc
}

/// 把一个矩形并入区域（并集）。零尺寸矩形跳过；中间单矩形区域用完即删。
fn add_rect(acc: &mut Option<HRGN>, r: RectF) {
    let x1 = r.x as i32;
    let y1 = r.y as i32;
    let x2 = (r.x + r.w) as i32;
    let y2 = (r.y + r.h) as i32;
    if x2 <= x1 || y2 <= y1 {
        return;
    }
    let rect = unsafe { CreateRectRgn(x1, y1, x2, y2) };
    match acc {
        None => *acc = Some(rect),
        Some(dst) => {
            unsafe { CombineRgn(Some(*dst), Some(*dst), Some(rect), RGN_OR) };
            unsafe {
                let _ = DeleteObject(rect.into());
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            // 区域外的点根本不会进入本窗口（区域已裁剪为栅栏并集），
            // 能到达这里的一定在栅栏内 → HTCLIENT。状态未就绪时穿透兜底。
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(HTTRANSPARENT as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        // 合成器接管绘制，擦除由合成器完成
        WM_ERASEBKGND => LRESULT(1),
        // 边缘/角标缩放光标：像普通窗口一样给拖拽手势反馈
        WM_SETCURSOR => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            let cur = cursor_at(&state.model, pt.x as f32, pt.y as f32);
            unsafe {
                let _ = SetCursor(Some(cur));
            }
            LRESULT(1)
        }
        // 文件拖放：把路径交给 App 加入命中栅栏
        WM_DROPFILES => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            let hdrop = HDROP(wparam.0 as *mut core::ffi::c_void);
            let mut pt = POINT::default();
            unsafe {
                let _ = DragQueryPoint(hdrop, &mut pt);
            }
            let paths = drop_paths(hdrop);
            unsafe { DragFinish(hdrop) };
            if !paths.is_empty() {
                // 用整栅栏矩形（标题/主体/手柄，含边缘）判定落点——拖到标题栏也应进该栅栏
                let px = pt.x as f32;
                let py = pt.y as f32;
                let fence = state
                    .model
                    .fences
                    .iter()
                    .find(|f| {
                        f.body.contains(px, py)
                            || f.title.contains(px, py)
                            || f.grip.contains(px, py)
                    })
                    .map(|f| f.id)
                    .or_else(|| state.model.fences.first().map(|f| f.id));
                if let Some(fence) = fence {
                    emit_event(hwnd, state, OverlayEvent::FilesDropped { fence, paths });
                }
            }
            LRESULT(0)
        }
        // 滚轮滚动：消息发给光标下的窗口（焦点窗口在别的线程时系统自动重定向）。
        // 高字是有符号滚轮刻度（WHEEL_DELTA=120 一格）。滚到栅栏/滚动条上才滚动。
        WM_MOUSEWHEEL => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if delta != 0 {
                let px = pt.x as f32;
                let py = pt.y as f32;
                // 控制台面板优先：滚轮滚动待办列表
                if let Some(c) = &state.model.console {
                    if c.rect.contains(px, py) {
                        emit_event(hwnd, state, OverlayEvent::ConsoleScroll { delta });
                        return LRESULT(0);
                    }
                }
                if let Some(f) = state.model.fences.iter().find(|f| f.body.contains(px, py)) {
                    emit_event(
                        hwnd,
                        state,
                        OverlayEvent::FenceScroll { fence: f.id, delta },
                    );
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            // 鼠标消息 lParam 是客户端坐标；overlay 恰好覆盖虚拟屏幕且无边框，
            // 客户端坐标即虚拟屏幕坐标，可直接命中测试。
            let (mx, my) = client_point(lparam);
            match msg {
                WM_LBUTTONDOWN => {
                    // 用户按下即唤起（用户前台会话）：消息能到达本窗口即说明点击落在
                    // 窗口区域内（栅栏 ∪ 控制台 ∪ 占位框），属于对 Sylva 表面的主动
                    // 操作——`WS_EX_NOACTIVATE` 使系统永远不会因点击激活/提层本窗口，
                    // 必须在此重断言顶部（被其它程序盖住后点击露出部分是常态场景）。
                    // 不缓存「已提权」标志，已在顶部时为无害空操作。
                    raise_hwnd_to_normal_top(hwnd);
                    on_button_down(hwnd, state, mx, my);
                }
                WM_LBUTTONUP => on_button_up(hwnd, state, mx, my),
                WM_LBUTTONDBLCLK => on_double_click(hwnd, state, mx, my),
                _ => unreachable!(),
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };

            // 消息合并 (Coalescing)：高回报率电竞鼠标 (1000Hz+) 高速滑动时会向消息队列注入
            // 成百上千条 WM_MOUSEMOVE。若不合并，每帧都触发排版、渲染与 WinRT 模糊，会导致
            // 消息队列雪崩式阻塞并被系统判定为 AppHangB1。通过 PeekMessageW 消费并跳过过时的
            // 中间鼠标移动，仅对队列中最新的一帧做处理。
            let mut latest_lparam = lparam;
            unsafe {
                let mut peek_msg = std::mem::zeroed();
                while PeekMessageW(
                    &mut peek_msg,
                    Some(hwnd),
                    WM_MOUSEMOVE,
                    WM_MOUSEMOVE,
                    PM_REMOVE,
                )
                .as_bool()
                {
                    latest_lparam = peek_msg.lParam;
                }
            }

            let (mx, my) = client_point(latest_lparam);
            // 请求 WM_MOUSELEAVE：光标离开窗口时清除 Dock 放大（避免放大「粘」住）
            unsafe {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            }
            // 非拖拽期间跟踪悬停：目标变化才上报（避免每帧重绘）。
            if state.drag.is_none() {
                let key = hover_key(&state.model, mx, my);
                if key != state.hovered {
                    state.hovered = key;
                    match key {
                        Some((f, Some(i))) => {
                            tracing::trace!(fence = f, icon = i, "悬停进入图标");
                            emit_event(hwnd, state, OverlayEvent::HoverEnter { fence: f, icon: i });
                        }
                        _ => emit_event(hwnd, state, OverlayEvent::HoverLeave),
                    }
                }
                // 控制台控件悬停（展开面板内才生效；折叠胶囊 / 空白处 = None）
                let cz = state
                    .model
                    .console
                    .as_ref()
                    .filter(|c| c.rect.contains(mx, my))
                    .and_then(|c| console_zone_at(c, mx, my));
                if cz != state.console_hovered {
                    state.console_hovered = cz;
                    emit_event(hwnd, state, OverlayEvent::ConsoleHover { zone: cz });
                }
            } else if state.console_hovered.is_some() {
                // 拖拽期间不跟踪悬停（控制台热区会随面板宽度移动，逐帧重算没有意义），
                // 但必须**清掉**进入拖拽前留下的那个：否则拖动面板 / 拖边改宽时，
                // 进入拖拽那一刻鼠标下的控件（多半是路径）会全程亮着高亮与下划线。
                // （`ConsoleMove` / `ConsoleResize` / 拖图标都会走到这个分支。）
                state.console_hovered = None;
                emit_event(hwnd, state, OverlayEvent::ConsoleHover { zone: None });
            }
            // 光标位置变化 → 连续 Dock 放大（位置未变不上报，拖拽期间禁止上报以避免每帧冗余事件与重绘）
            if state.drag.is_none() && state.last_cursor != Some((mx, my)) {
                state.last_cursor = Some((mx, my));
                emit_event(hwnd, state, OverlayEvent::CursorMove { x: mx, y: my });
            }
            on_mouse_move(hwnd, state, mx, my);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // 捕获被外部夺走 = 拖拽会话被迫终止（正常松开走 WM_LBUTTONUP）。不清掉的
            // 话 state.drag 残留，会把随后迟到的 WM_MOUSELEAVE（ReleaseCapture 后约
            // 31ms 才到）拦在 drag 门控上——点击唤起的提权就会滞留在普通带顶部。清掉
            // 之后迟到的 leave 恢复生效，回落桌面带。布局持久化仍以真实 WM_LBUTTONUP
            // 为准，此处只回收渲染层拖拽状态（孤儿状态回收）。
            //
            // 悬停旁路镜像必须与 App 层同批清除（旁路数据对齐）：下面的事件会把 App 层
            // 的 `rt.cursor` / `rt.hover` / `rt.console_hover` 清掉，但渲染层镜像若不清，
            // capture 被夺而光标仍停在图标上时，后续 WM_MOUSEMOVE 不会重发 HoverEnter
            // （渲染层以为悬停从未离开）——悬停高亮 / Dock 放大滞留失效态，直到光标
            // 离开再进入。重复的 HoverLeave / ConsoleHover 事件是幂等的，App 层以覆盖
            // 写处理。此处刻意**不**回联 `restore_hwnd_desktop_band`：桌面带回落由拖拽
            // 结束后迟到的 WM_MOUSELEAVE 承担（光标可能仍在表面，保持提权，移开即回落），
            // 届时本分支已清 drag，leave 分支的 `drag.is_none()` 门控能放行。
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            if state.drag.take().is_some() {
                // 镜像 WM_MOUSELEAVE 的悬停清理（语义逐行一致，理由见该分支注释）。
                state.hovered = None;
                state.last_cursor = None;
                if state.console_hovered.is_some() {
                    state.console_hovered = None;
                    emit_event(hwnd, state, OverlayEvent::ConsoleHover { zone: None });
                }
                emit_event(hwnd, state, OverlayEvent::HoverLeave);
                emit_event(hwnd, state, OverlayEvent::CursorLeave);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            // 清空悬停 + 光标（清除 Dock 放大）
            state.hovered = None;
            state.last_cursor = None;
            // 控制台悬停必须**上报**清除，不能只把本地镜像置空：App 层的 `rt.console_hover`
            // 才是绘制高亮的唯一来源，而光标已离开窗口、不会再有 `WM_MOUSEMOVE` 来纠正它，
            // 于是离开前那个控件的下划线 / 高亮会一直「粘」着不灭。
            // （`HoverLeave` / `CursorLeave` 都只管图标悬停与 Dock 放大，不清控制台悬停。）
            if state.console_hovered.is_some() {
                state.console_hovered = None;
                emit_event(hwnd, state, OverlayEvent::ConsoleHover { zone: None });
            }
            emit_event(hwnd, state, OverlayEvent::HoverLeave);
            emit_event(hwnd, state, OverlayEvent::CursorLeave);
            // 「前台会话」随光标离开表面而结束：点击唤起的提权（见 WM_LBUTTONDOWN /
            // WM_RBUTTONDOWN 处注释）在光标移回其它窗口时回落桌面带。控制中心会话
            // 不受此影响——生命周期由 app 的 `set_console_open` 开/关配对管理
            // （`console_session` 位）；拖拽进行中也不回落（capture 期间光标可能瞬时
            // 划出区域，中途回落会让拖拽物掉到其它窗口后面）。已在桌面带时为无害空操作。
            if !state.console_session && state.drag.is_none() {
                restore_hwnd_desktop_band(hwnd, state.owner);
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return LRESULT(0);
            }
            let state = unsafe { &mut *ptr };
            let (mx, my) = client_point(lparam);
            // 右键同样属于对表面的主动操作：先唤起再弹菜单（见 WM_LBUTTONDOWN 处注释），
            // 否则 Shell 菜单弹出在 Sylva 之下、点外部还可能被系统以前台化为由拒绝关闭。
            raise_hwnd_to_normal_top(hwnd);
            // 命中：控制台 → 图标 → 栅栏；未命中任何交互目标时吞掉。
            if let Some(c) = &state.model.console {
                if c.rect.contains(mx, my) {
                    return LRESULT(0);
                }
            }
            if let Some(icon) = state.model.icons.iter().find(|i| i.rect.contains(mx, my)) {
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::ContextMenu {
                        fence: icon.fence,
                        icon: Some(icon.icon),
                        pos: (mx, my),
                    },
                );
            } else if let Some(f) = state.model.fences.iter().find(|f| f.body.contains(mx, my)) {
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::ContextMenu {
                        fence: f.id,
                        icon: None,
                        pos: (mx, my),
                    },
                );
            }
            LRESULT(0)
        }
        // 全局热键触发
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_QUIT => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_CONSOLE => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::ConsoleToggle);
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_DESKTOP => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::DesktopToggle);
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_AUTO_ORGANIZE => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::AutoOrganize);
            }
            LRESULT(0)
        }
        // 托盘图标回调：右键 → 控制中心菜单；左键单击 → 切换控制中心开合
        WM_TRAY if lparam.0 as u32 == WM_RBUTTONUP => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::TrayMenu);
            }
            LRESULT(0)
        }
        // 左键单击：一次点击即开/关控制中心。双击不另作处理，并用双击时限去重，
        // 否则双击派发的两次 WM_LBUTTONUP 会「打开又立刻关上」。
        WM_TRAY if lparam.0 as u32 == WM_LBUTTONUP => {
            let now = now_ms();
            let last = LAST_TRAY_CLICK_MS.swap(now, Ordering::Relaxed);
            if now.saturating_sub(last) >= unsafe { GetDoubleClickTime() } as u64 {
                let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
                if !ptr.is_null() {
                    let state = unsafe { &mut *ptr };
                    emit_event(hwnd, state, OverlayEvent::TrayToggle);
                }
            }
            LRESULT(0)
        }
        // 库同步定时器：周期触发，App 同步移除已被删除的库文件对应栅栏项
        WM_TIMER if wparam.0 == SYNC_LIBRARY_TIMER => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::SyncLibrary);
            }
            LRESULT(0)
        }
        // 兜底节拍：拿不到可等待定时器时由 `SetTimer` 发来（见 `arm_fallback_clock`）。
        WM_TIMER if wparam.0 == ANIM_TIMER_FALLBACK => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::AnimTick);
            }
            LRESULT(0)
        }
        // 动画节拍：由 `run_message_loop` 等到的高分辨率定时器投递（见 `WM_APP_ANIM_TICK`）。
        // App 推进控制台/待办行动画并重绘（无动画时 App 自行停表）。
        WM_APP_ANIM_TICK => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::AnimTick);
            }
            LRESULT(0)
        }
        // 所在显示器 DPI 变化（Per-Monitor v2）：`wParam` 低位 = 新 x-DPI。
        // App 重算主题缩放 + 重新夹屏 + 重绘，栅栏/文字实时跟随缩放（不再等 DWM 拉伸）。
        WM_DPICHANGED => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::DpiChanged {
                        dpi: (wparam.0 & 0xFFFF) as u32,
                    },
                );
            }
            LRESULT(0)
        }
        // 显示拓扑/分辨率变化：虚拟屏宽高可能已变，App 重算并夹回超屏栅栏。
        WM_DISPLAYCHANGE => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::DisplayChange);
            }
            LRESULT(0)
        }
        // —— 键盘输入与快捷键录制：键盘直达 App（overlay 获得焦点时）——
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::KeyDown {
                        vk: wparam.0 as u32,
                        ctrl: is_ctrl_down(),
                        shift: is_shift_down(),
                        alt: is_alt_down(),
                        win: is_win_down(),
                    },
                );
            }
            LRESULT(0)
        }
        WM_CHAR => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::Char {
                        ch: wparam.0 as u16,
                    },
                );
            }
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::ImeStart);
            }
            LRESULT(0)
        }
        WM_IME_COMPOSITION => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                let flags = lparam.0 as u32;
                if flags & GCS_RESULTSTR.0 != 0 {
                    let text = ime_string(hwnd, GCS_RESULTSTR);
                    emit_event(hwnd, state, OverlayEvent::ImeEnd);
                    emit_event(hwnd, state, OverlayEvent::ImeResult { text });
                } else if flags & GCS_COMPSTR.0 != 0 {
                    let text = ime_string(hwnd, GCS_COMPSTR);
                    emit_event(hwnd, state, OverlayEvent::ImeCompose { text, caret: 0 });
                }
            }
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::ImeEnd);
            }
            LRESULT(0)
        }
        WM_IME_SETCONTEXT => {
            // 关闭系统默认合成框（文本由 D2D 内联编辑自己绘制），候选窗口仍跟随光标
            let lp = lparam.0 & !(0x8000_0000isize);
            unsafe { DefWindowProcW(hwnd, msg, wparam, LPARAM(lp)) }
        }
        WM_KILLFOCUS => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                emit_event(hwnd, state, OverlayEvent::OverlayFocusLost);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // 编辑框（重命名/待办输入，owner = 本窗口）绘制背景/文字色。
        // 返回暗色画刷 + 设置文字色，消除默认纯白底；底色与面板填充色一致。
        WM_CTLCOLOREDIT => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if ptr.is_null() {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let state = unsafe { &mut *ptr };
            let hdc = HDC(lparam.0 as *mut core::ffi::c_void);
            unsafe {
                let _ = SetTextColor(hdc, COLORREF(0x00_FA_F2_EE)); // RGB(238,242,250)
                let _ = SetBkColor(hdc, COLORREF(0x00_22_16_10)); // RGB(16,22,34) 面板底色
            }
            LRESULT(state.edit_brush.0 as isize)
        }
        // 外部信号：干净退出消息循环（wnd_proc 跑在主线程，PostQuitMessage 投递到主队列）
        WM_APP_QUIT => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // 外部关闭请求（用户点关闭按钮 / Alt+F4 / 任务管理器「结束任务」发 WM_CLOSE）：
        // 走干净退出，让 RAII 恢复被隐藏的真实桌面图标——硬杀会跳过 Drop，桌面图标
        // 永久消失（卸载程序不再强杀，用户手动关闭即走到这里）。
        WM_CLOSE => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // App 注入事件：就地重命名提交后触发一次完整重绘 + 命中模型重建。
        // `lParam` 是一个 `Box<OverlayEvent>`（发送方 into_raw，这里 from_raw 并释放）。
        WM_WINBOSK_INJECT if lparam.0 != 0 => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
            if !ptr.is_null() {
                let state = unsafe { &mut *ptr };
                let ev = unsafe { Box::from_raw(lparam.0 as *mut OverlayEvent) };
                emit_event(hwnd, state, *ev);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 控制台面板内命中的控件（先匹配的生效；控件矩形已含「仅面板内」的前提）。
///
/// **必须排除零面积矩形**：未显示的控件用 `RectF::default()` = `(0,0,0,0)` 占位，
/// 而 `RectF::contains` 含边界，于是客户端原点 `(0,0)` 会命中这些占位矩形，凭空报出一个
/// 根本不存在的控件（如未展开的「图标尺寸」「侧边栏停靠位置」）。这既会让单击误触发，
/// 也会让随后 `on_double_click` 的连击校验在原点重复触发幽灵控件。
fn console_zone_at(c: &ConsoleHit, mx: f32, my: f32) -> Option<ConsoleZone> {
    c.zones
        .iter()
        .find(|(_, r)| r.w > 0.0 && r.h > 0.0 && r.contains(mx, my))
        .map(|(z, _)| *z)
}

/// 控制台面板右/下边缘与右下角的缩放区（面板锚定左上角，只缩放不位移）。
/// 缩放区判定在标题栏移动之前，保证抓右/下边即缩放（标准窗口行为）。
fn console_resize_zone_at(c: &ConsoleHit, mx: f32, my: f32) -> Option<ResizeZone> {
    let right = c.rect.x + c.rect.w;
    let bottom = c.rect.y + c.rect.h;
    if mx >= right - CORNER_RESIZE && my >= bottom - CORNER_RESIZE {
        return Some(ResizeZone::BottomRight);
    }
    if mx >= right - EDGE_RESIZE {
        return Some(ResizeZone::Right);
    }
    if my >= bottom - EDGE_RESIZE {
        return Some(ResizeZone::Bottom);
    }
    None
}

/// 该控制台控件是否允许被快速连击重复触发。
///
/// 判据：**重复触发不产生任何额外副作用**。本列表刻意与 `scene.rs` 里真正会
/// `zones.push` 的控件一一对应，只放行「把同一个值再设一遍」这类幂等设置项。
///
/// 被排除的都是重复执行有真实副作用的控件：
/// - `AddFence`：一次双击会多建一个栅栏；
/// - `RemoveFence`：一次双击会连删两个栅栏（虽有确认弹窗，也不该被一次手势触发两次）；
/// - `ChangeStoragePath`：会弹两次文件夹选择器（模态）；
/// - `OpenStoragePath`：会拉起两个资源管理器窗口；
/// - `ResetStoragePath`：解除外部链接本身幂等，但重复触发毫无收益，不给它开后门；
/// - `AutoOrganize`：会跑两遍一键整理；
/// - `DesktopToggle`：虽是纯状态翻转，但每次翻转都伴随 `restore_icons`/`hide_icons` 与
///   整屏淡入淡出补间，重复触发会出现「桌面闪一下又回来」的可见抖动；
/// - `Close`：会改变面板几何，原坐标随即失效。
///
/// `Expand` / `Tab` 目前不会被 `zones.push`（属死控件），故不列入——新增变体默认落到
/// `false`（fail-safe），需要放行时再显式加进来。
///
/// 注意：栅栏收展按钮同样是状态翻转，但它走 `collapse_target_at` 单独处理，不经过本白名单。
fn console_zone_is_repeatable(zone: ConsoleZone) -> bool {
    matches!(
        zone,
        ConsoleZone::FenceSelect(_)
            | ConsoleZone::FenceLayout(_)
            | ConsoleZone::FenceIconSize(_)
            | ConsoleZone::FenceStyle(_)
            | ConsoleZone::FenceSidebarPos(_)
            | ConsoleZone::FenceTint(_)
            | ConsoleZone::FenceRulePreset(_)
    )
}

/// 按下：命中控制台控件/标题栏、边缘/角标开始缩放；命中栅栏标题栏/空白开始移动。
/// 拖拽类动作都捕获鼠标以跟踪拖出窗口的移动。
fn on_button_down(hwnd: HWND, state: &mut WindowState, mx: f32, my: f32) {
    // 每次按下都重置控制台记忆：只有「紧接的下一次连击」才允许复用上一次的命中，
    // 中间只要按过别处（或压根没命中面板），记忆立即失效。
    state.last_press_in_console = false;
    state.last_console_zone = None;
    // 内联编辑框优先：点编辑框内部 = 把光标定位到点击处，不落到下面的栅栏/图标
    // （否则会触发「点击别处提交编辑」，用户根本无法点击文本）。
    if let Some(er) = state.model.edit_rect {
        if er.contains(mx, my) {
            emit_event(hwnd, state, OverlayEvent::EditCaret { x: mx });
            return;
        }
    }
    // 控制台优先：面板上的点击不落到栅栏/图标。控件（关闭/勾选/删除/添加）优先于
    // 标题栏拖动——关闭按钮就在标题栏内，先判控件才能「点 × 即关」而非开始拖动。
    if let Some(c) = &state.model.console {
        if c.rect.contains(mx, my) {
            // 面板整体消费了这次按下（无论命中控件、标题栏还是空白）：
            // 随后的 WM_LBUTTONDBLCLK 一律不得穿透到底层桌面。
            state.last_press_in_console = true;
            if let Some(zone) = console_zone_at(c, mx, my) {
                // 记住本次命中，供随后的 WM_LBUTTONDBLCLK 判断是否仍指向同一控件。
                state.last_console_zone = Some(zone);
                emit_event(hwnd, state, OverlayEvent::ConsoleClick { zone });
                return;
            }
            // 右/下边缘与右下角 → 缩放（抓边即缩放，标准窗口行为）
            if let Some(zone) = console_resize_zone_at(c, mx, my) {
                state.drag = Some(DragState {
                    kind: DragKind::ConsoleResize(zone),
                    fence: usize::MAX,
                    start: (mx, my),
                    orig: c.rect,
                    pressed_icon: None,
                    ctrl: false,
                });
                unsafe {
                    let _ = SetCursor(Some(zone_cursor(zone)));
                }
                unsafe { SetCapture(hwnd) };
                return;
            }
            if c.title.contains(mx, my) {
                state.drag = Some(DragState {
                    kind: DragKind::ConsoleMove,
                    fence: usize::MAX,
                    start: (mx, my),
                    orig: c.rect,
                    pressed_icon: None,
                    ctrl: false,
                });
                unsafe {
                    let cur = LoadCursorW(None, IDC_SIZEALL).unwrap_or_default();
                    let _ = SetCursor(Some(cur));
                }
                unsafe { SetCapture(hwnd) };
                return;
            }
            return; // 面板空白：吞掉点击
        }
    }
    // 栅栏标题栏折叠/展开按钮优先于缩放与移动
    for f in &state.model.fences {
        if let Some(btn) = f.collapse_btn {
            if btn.contains(mx, my) {
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::FenceCollapseToggle { fence: f.id },
                );
                return;
            }
        }
    }
    // 优先级：缩放区域 > 标题栏 > 空白。
    for f in &state.model.fences {
        if f.body.contains(mx, my) {
            if let Some(zone) = resize_zone_at(f, mx, my) {
                state.drag = Some(DragState {
                    kind: DragKind::Resize(zone),
                    fence: f.id,
                    start: (mx, my),
                    orig: f.body,
                    pressed_icon: None,
                    ctrl: false,
                });
                // 拖拽期间 SetCapture 不再发 WM_SETCURSOR，这里锁定一次缩放光标
                unsafe {
                    let _ = SetCursor(Some(zone_cursor(zone)));
                }
                unsafe { SetCapture(hwnd) };
                return;
            }
            break;
        }
    }
    for f in &state.model.fences {
        if f.title.contains(mx, my) {
            // 侧边栏图标上点击不启动拖动（留给 mouseup 作为单击打开文件）
            let on_icon = state
                .model
                .icons
                .iter()
                .any(|i| i.fence == f.id && i.rect.contains(mx, my));
            if on_icon {
                break; // 跳到下方 body+图标处理
            }
            state.drag = Some(DragState {
                kind: DragKind::Move,
                fence: f.id,
                start: (mx, my),
                orig: f.body,
                pressed_icon: None,
                ctrl: false,
            });
            unsafe {
                let cur = LoadCursorW(None, IDC_SIZEALL).unwrap_or_default();
                let _ = SetCursor(Some(cur));
            }
            unsafe { SetCapture(hwnd) };
            return;
        }
    }
    // 兜底：按下栅栏主体。
    // - 空白处 → 框选（橡皮筋多选，资源管理器行为；拖动过程中移动栅栏改走标题栏/图标）。
    // - 图标上 → 拖动栅栏；位移小时作为「单击选中」，位移大时仍是拖动。
    for f in &state.model.fences {
        if f.body.contains(mx, my) {
            let pressed_icon = state
                .model
                .icons
                .iter()
                .find(|i| i.fence == f.id && i.rect.contains(mx, my))
                .map(|i| i.icon);
            if pressed_icon.is_none() {
                let ctrl = is_ctrl_down();
                state.drag = Some(DragState {
                    kind: DragKind::Select,
                    fence: f.id,
                    start: (mx, my),
                    orig: f.body,
                    pressed_icon: None,
                    ctrl,
                });
                // 空白按下即清空选择（Explorer 行为）；拖拽中再逐帧上报框选结果
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::SelectDrag {
                        fence: f.id,
                        rect: (mx, my, 0.0, 0.0),
                        selected: Vec::new(),
                    },
                );
                unsafe { SetCapture(hwnd) };
                return;
            }
            state.drag = Some(DragState {
                kind: DragKind::Move,
                fence: f.id,
                start: (mx, my),
                orig: f.body,
                pressed_icon,
                ctrl: is_ctrl_down(),
            });
            unsafe {
                let cur = LoadCursorW(None, IDC_SIZEALL).unwrap_or_default();
                let _ = SetCursor(Some(cur));
            }
            unsafe { SetCapture(hwnd) };
            return;
        }
    }
}

/// Ctrl 是否处于按下状态（框选/多选判定）。
fn is_ctrl_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
}

/// Shift 是否处于按下状态。
fn is_shift_down() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

/// Alt 是否处于按下状态。
fn is_alt_down() -> bool {
    unsafe { GetKeyState(VK_MENU.0 as i32) < 0 }
}

/// Win 徽标键是否处于按下状态。
fn is_win_down() -> bool {
    unsafe { GetKeyState(VK_LWIN.0 as i32) < 0 || GetKeyState(VK_RWIN.0 as i32) < 0 }
}

/// 两个矩形是否相交（含边接触）——框选命中判定。
fn rect_overlap(a: RectF, b: RectF) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// 计算框选矩形命中的图标：只在本栅栏内取与矩形相交的图标（布局下标）。
fn band_selection(model: &HitModel, fence: usize, band: RectF) -> Vec<(usize, usize)> {
    model
        .icons
        .iter()
        .filter(|i| i.fence == fence && rect_overlap(i.rect, band))
        .map(|i| (i.fence, i.icon))
        .collect()
}

/// 点所在栅栏的缩放区域（角优先、边其次）。不在任何缩放区返回 None。
/// 折叠栅栏仅保留标题栏，禁止任何边或角缩放，防止破坏自适应高度或原始高度。
fn resize_zone_at(f: &FenceHit, mx: f32, my: f32) -> Option<ResizeZone> {
    if f.collapsed {
        return None;
    }
    let left = f.body.x;
    let right = f.body.x + f.body.w;
    let bottom = f.body.y + f.body.h;
    // 角（优先级最高）
    if mx >= right - CORNER_RESIZE && my >= bottom - CORNER_RESIZE {
        return Some(ResizeZone::BottomRight);
    }
    if mx <= left + CORNER_RESIZE && my >= bottom - CORNER_RESIZE {
        return Some(ResizeZone::BottomLeft);
    }
    if mx >= right - CORNER_RESIZE && my <= f.body.y + CORNER_RESIZE {
        return Some(ResizeZone::TopRight);
    }
    // 边
    if mx >= right - EDGE_RESIZE {
        return Some(ResizeZone::Right);
    }
    if mx <= left + EDGE_RESIZE {
        return Some(ResizeZone::Left);
    }
    if my >= bottom - EDGE_RESIZE {
        return Some(ResizeZone::Bottom);
    }
    None
}

/// 缩放区域对应的系统光标。
fn zone_cursor(zone: ResizeZone) -> HCURSOR {
    let id = match zone {
        ResizeZone::Left | ResizeZone::Right => IDC_SIZEWE,
        ResizeZone::Bottom => IDC_SIZENS,
        ResizeZone::BottomRight => IDC_SIZENWSE,
        ResizeZone::BottomLeft | ResizeZone::TopRight => IDC_SIZENESW,
    };
    unsafe { LoadCursorW(None, id).unwrap_or_default() }
}

/// 光标处的形状（参考 Windows 资源管理器：只有标题栏/缩放区有特殊光标）：
/// 边缘/角 → 对应 resize 光标；标题栏 → 移动光标；图标/空白一律普通箭头。
/// 侧边栏：图标上保持箭头（点击打开），空白处才显示移动把手。
fn cursor_at(model: &HitModel, mx: f32, my: f32) -> HCURSOR {
    // 控制台面板：边缘/角 = resize 光标，控件/空白 = 箭头，标题栏 = 移动光标
    if let Some(c) = &model.console {
        if c.rect.contains(mx, my) {
            if let Some(zone) = console_resize_zone_at(c, mx, my) {
                return zone_cursor(zone);
            }
            if c.title.contains(mx, my) && console_zone_at(c, mx, my).is_none() {
                return unsafe { LoadCursorW(None, IDC_SIZEALL).unwrap_or_default() };
            }
            return unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };
        }
    }
    let Some(f) = model
        .fences
        .iter()
        .find(|f| f.body.contains(mx, my) || f.title.contains(mx, my))
    else {
        return unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };
    };
    if let Some(zone) = resize_zone_at(f, mx, my) {
        return zone_cursor(zone);
    }
    // 侧边栏：图标上保持箭头（单击打开），只有空白处显示移动光标
    let on_icon = model
        .icons
        .iter()
        .any(|i| i.fence == f.id && i.rect.contains(mx, my));
    if on_icon {
        return unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };
    }
    // 非缩放区：标题栏是移动把手，正文/图标保持箭头（像资源管理器）。
    if f.title.contains(mx, my) {
        return unsafe { LoadCursorW(None, IDC_SIZEALL).unwrap_or_default() };
    }
    unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() }
}

/// 悬停目标：先图标后栅栏（图标优先）。未命中任何栅栏时返回 None。
fn hover_key(model: &HitModel, mx: f32, my: f32) -> Option<(usize, Option<usize>)> {
    // 鼠标在控制台面板上时不悬停任何栅栏/图标（面板浮于栅栏之上）
    if let Some(c) = &model.console {
        if c.rect.contains(mx, my) {
            return None;
        }
    }
    for icon in &model.icons {
        if icon.rect.contains(mx, my) {
            return Some((icon.fence, Some(icon.icon)));
        }
    }
    for f in &model.fences {
        if f.body.contains(mx, my) {
            return Some((f.id, None));
        }
    }
    None
}

/// 移动：拖动中连续上报新位置/新尺寸。目标一律用**原始矩形 + 鼠标总位移**
/// （`drag.orig + (鼠标 - 按下点)`），即「原始目标」——App 层对每个原始目标
/// 独立做碰撞/吸附。若用上一帧已生效矩形做增量，吸附是粘性的：被吸住后
/// 每帧的小增量（几像素 < 吸附阈值）都会再次触发吸附，栅栏永远跳不出去。
/// 原始目标保证：鼠标一旦移出吸附阈值（超过 24px），吸附自然解除。
fn on_mouse_move(hwnd: HWND, state: &mut WindowState, mx: f32, my: f32) {
    let Some(drag) = state.drag else {
        return;
    };
    let orig = drag.orig;
    match drag.kind {
        DragKind::Move => {
            // 侧边栏图标上按下 + 移动超阈值 → 切换为拖动排序模式
            if let Some(icon_idx) = drag.pressed_icon {
                let is_sidebar = state
                    .model
                    .fences
                    .iter()
                    .find(|f| f.id == drag.fence)
                    .map(|f| f.is_sidebar)
                    .unwrap_or(false);
                let dx = mx - drag.start.0;
                let dy = my - drag.start.1;
                if is_sidebar && (dx * dx + dy * dy) > REORDER_THRESHOLD * REORDER_THRESHOLD {
                    state.drag = Some(DragState {
                        kind: DragKind::SidebarReorder,
                        fence: drag.fence,
                        start: drag.start,
                        orig: drag.orig,
                        pressed_icon: Some(icon_idx),
                        ctrl: false,
                    });
                    emit_event(
                        hwnd,
                        state,
                        OverlayEvent::SidebarReorderDrag {
                            fence: drag.fence,
                            icon: icon_idx,
                            mx,
                            my,
                        },
                    );
                    return;
                }
            }
            // 原始目标左上角 = 按下时矩形左上角 + 鼠标相对按下点的位移
            let raw_x = orig.x + (mx - drag.start.0);
            let raw_y = orig.y + (my - drag.start.1);
            let event = OverlayEvent::FenceMove {
                fence: drag.fence,
                pos: (raw_x, raw_y),
            };
            emit_event(hwnd, state, event);
        }
        DragKind::SidebarReorder => {
            if let Some(icon_idx) = drag.pressed_icon {
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::SidebarReorderDrag {
                        fence: drag.fence,
                        icon: icon_idx,
                        mx,
                        my,
                    },
                );
            }
        }
        DragKind::Select => {
            // 橡皮筋框选：从按下点到当前点围成矩形，框中的本栅栏图标全部选中
            let (x1, y1) = drag.start;
            let band = RectF {
                x: x1.min(mx),
                y: y1.min(my),
                w: (mx - x1).abs(),
                h: (my - y1).abs(),
            };
            let selected = band_selection(&state.model, drag.fence, band);
            let event = OverlayEvent::SelectDrag {
                fence: drag.fence,
                rect: (band.x, band.y, band.w, band.h),
                selected,
            };
            emit_event(hwnd, state, event);
        }
        DragKind::ConsoleMove => {
            // 目标左上角 = 按下时面板左上角 + 鼠标相对按下点的位移
            let raw_x = orig.x + (mx - drag.start.0);
            let raw_y = orig.y + (my - drag.start.1);
            emit_event(
                hwnd,
                state,
                OverlayEvent::ConsoleMove {
                    pos: (raw_x, raw_y),
                },
            );
        }
        DragKind::ConsoleResize(zone) => {
            // 面板锚定左上角：只调宽高（宽/高可随动，左上角不动）
            let (dx, dy) = (mx - drag.start.0, my - drag.start.1);
            let mut r = orig;
            match zone {
                ResizeZone::Right => r.w += dx,
                ResizeZone::Bottom => r.h += dy,
                ResizeZone::BottomRight => {
                    r.w += dx;
                    r.h += dy;
                }
                _ => {}
            }
            emit_event(
                hwnd,
                state,
                OverlayEvent::ConsoleResize {
                    rect: (r.x, r.y, r.w, r.h),
                },
            );
        }
        DragKind::Resize(zone) => {
            let (dx, dy) = (mx - drag.start.0, my - drag.start.1);
            // 原始目标矩形：被拖的边跟随鼠标，锚定边保持按下时位置
            let mut r = orig;
            match zone {
                ResizeZone::Right => r.w += dx,
                ResizeZone::Bottom => r.h += dy,
                ResizeZone::BottomRight => {
                    r.w += dx;
                    r.h += dy;
                }
                ResizeZone::Left => {
                    r.x += dx;
                    r.w -= dx;
                }
                ResizeZone::BottomLeft => {
                    r.x += dx;
                    r.w -= dx;
                    r.h += dy;
                }
                ResizeZone::TopRight => {
                    r.y += dy;
                    r.h -= dy;
                    r.w += dx;
                }
            }
            let event = OverlayEvent::FenceResize {
                fence: drag.fence,
                zone,
                rect: (r.x, r.y, r.w, r.h),
            };
            emit_event(hwnd, state, event);
        }
    }
}

/// 枚举 `WM_DROPFILES` 携带的文件路径。
fn drop_paths(hdrop: HDROP) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..n {
            let len = DragQueryFileW(hdrop, i, None);
            let mut buf = vec![0u16; len as usize + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            out.push(String::from_utf16_lossy(&buf[..end]));
        }
    }
    out
}

/// 松开：结束拖拽会话。位移小于阈值且按在图标上 → 视为「单击选中」；
/// 否则是拖动（无论是否移动，都通知 App 持久化布局）。
fn on_button_up(hwnd: HWND, state: &mut WindowState, mx: f32, my: f32) {
    if let Some(drag) = state.drag.take() {
        if drag.kind == DragKind::Move {
            let dx = mx - drag.start.0;
            let dy = my - drag.start.1;
            let moved = dx * dx + dy * dy;
            let t = CLICK_DRAG_THRESHOLD;
            if moved < t * t {
                if let Some(icon) = drag.pressed_icon {
                    emit_event(
                        hwnd,
                        state,
                        OverlayEvent::IconClicked {
                            fence: drag.fence,
                            icon,
                            ctrl: drag.ctrl,
                        },
                    );
                }
            }
        } else if drag.kind == DragKind::Select {
            // 框选结束：清除橡皮筋显示（选择结果已在最后一次 SelectDrag 中生效）
            emit_event(hwnd, state, OverlayEvent::SelectEnd);
        } else if drag.kind == DragKind::ConsoleMove {
            // 控制台拖动结束：App 持久化位置
            emit_event(hwnd, state, OverlayEvent::ConsoleDragEnd);
        } else if let DragKind::ConsoleResize(_) = drag.kind {
            // 控制台缩放结束：App 持久化尺寸
            emit_event(hwnd, state, OverlayEvent::ConsoleResizeEnd);
        } else if drag.kind == DragKind::SidebarReorder {
            // 侧边栏拖动排序结束：通知 App 执行重排
            if let Some(icon_idx) = drag.pressed_icon {
                // 计算目标位置：根据光标位置在侧边栏中的相对偏移推算插入下标
                let to = compute_reorder_target(&state.model, drag.fence, icon_idx, mx, my);
                emit_event(
                    hwnd,
                    state,
                    OverlayEvent::SidebarReorderEnd {
                        fence: drag.fence,
                        from: icon_idx,
                        to,
                    },
                );
            }
        }
        // 框选/控制台拖拽不是栅栏布局变化，不触发栅栏持久化
        if matches!(
            drag.kind,
            DragKind::Move | DragKind::Resize(_) | DragKind::SidebarReorder
        ) {
            emit_event(
                hwnd,
                state,
                OverlayEvent::FenceDragEnd { fence: drag.fence },
            );
        }
        unsafe {
            let _ = ReleaseCapture();
        }
    }
}

/// 根据光标位置计算侧边栏图标重排的目标插入位置。
/// 返回值为插入下标（0..=图标总数）：图标将被插入到该下标之前。
fn compute_reorder_target(
    model: &HitModel,
    fence_id: usize,
    _from: usize,
    mx: f32,
    my: f32,
) -> usize {
    // 收集该栅栏的所有图标，按 x 坐标排序（横向侧边栏）或 y 坐标排序（纵向）
    let mut icons: Vec<_> = model.icons.iter().filter(|i| i.fence == fence_id).collect();
    if icons.is_empty() {
        return 0;
    }
    // 判断侧边栏方向：如果图标 x 变化大于 y 变化，为横向（top），否则纵向（left/right）
    let first = &icons[0];
    let is_horizontal = if icons.len() > 1 {
        let second = &icons[1];
        (second.rect.x - first.rect.x).abs() > (second.rect.y - first.rect.y).abs()
    } else {
        true // 单图标默认横向
    };
    if is_horizontal {
        // 横向：按 x 排序，找光标 x 落在哪两个图标之间
        icons.sort_by(|a, b| a.rect.x.partial_cmp(&b.rect.x).unwrap());
        for (i, icon) in icons.iter().enumerate() {
            let mid = icon.rect.x + icon.rect.w / 2.0;
            if mx < mid {
                return i;
            }
        }
        icons.len()
    } else {
        // 纵向：按 y 排序，找光标 y 落在哪两个图标之间
        icons.sort_by(|a, b| a.rect.y.partial_cmp(&b.rect.y).unwrap());
        for (i, icon) in icons.iter().enumerate() {
            let mid = icon.rect.y + icon.rect.h / 2.0;
            if my < mid {
                return i;
            }
        }
        icons.len()
    }
}

/// 双击图标：把下标交给 App 打开对应项。
fn on_double_click(hwnd: HWND, state: &mut WindowState, mx: f32, my: f32) {
    // 控制台面板：既阻断双击穿透到底层桌面，也让幂等控件「跟手」。
    //
    // 判据是「上一次**按下**是否被控制台面板消费」，而不是「当前坐标是否还在面板矩形内」：
    // 点「关闭」会收起面板（`console_open=false` + 收起补间），补间结束后
    // `scene.console` 直接变成 `None`（见 `scene.rs` 的 `console_open || panel > 0.01`）；
    // 拖动面板也会让它从光标底下移开。这些情形下按坐标判定都会失效并放行穿透，
    // **双击关闭按钮就会顺手打开正下方的一个桌面文件**。
    //
    // 落在**可重复控件**上的第二次按下要照常生效：与栅栏收展按钮同理，Windows 把快速连击
    // 的第二次按下变成 WM_LBUTTONDBLCLK，若一律吞掉，面板按钮也会「不跟手」。两道闸门保证安全：
    //   1. 必须与上一次按下命中**同一个**控件——面板行数随选中栅栏/布局动态增减，回流后
    //      同一坐标可能已经换成别的控件；
    //   2. 该控件必须可重复触发（见 `console_zone_is_repeatable`），否则「移出栅栏」会被一次双击触发两次。
    if state.last_press_in_console {
        if let Some(c) = &state.model.console {
            if let Some(zone) = console_zone_at(c, mx, my) {
                if state.last_console_zone == Some(zone) && console_zone_is_repeatable(zone) {
                    emit_event(hwnd, state, OverlayEvent::ConsoleClick { zone });
                }
            }
        }
        return;
    }
    // 面板打开时，落在面板矩形内的双击同样不得穿透（标题栏、空白处等）
    if let Some(c) = &state.model.console {
        if c.rect.contains(mx, my) {
            return;
        }
    }
    // 编辑框内双击同样不穿透（WM_LBUTTONDBLCLK 独立到达，需单独守卫）
    if let Some(er) = state.model.edit_rect {
        if er.contains(mx, my) {
            return;
        }
    }
    for icon in &state.model.icons {
        if icon.rect.contains(mx, my) {
            emit_event(
                hwnd,
                state,
                OverlayEvent::IconDoubleClicked {
                    fence: icon.fence,
                    icon: icon.icon,
                },
            );
            return;
        }
    }
    // 未命中图标：先判折叠/展开按钮，再判标题栏空白处（两者都是收起/展开）。
    match collapse_target_at(&state.model, mx, my) {
        Some(CollapseTarget::Button(fence)) => {
            // 按钮必须在这里也响应：Windows 对「快速连击」只派发 DOWN → UP → DBLCLK → UP，
            // 第二次按下不会再来 WM_LBUTTONDOWN。原先此处把落在按钮上的 DBLCLK 直接吞掉，
            // 于是快速点按每两次只有一次生效（按钮「不跟手」）。按钮的语义是「按一次切换
            // 一次」，与单击完全一致，所以 DBLCLK 照常切换即可，无需任何防抖/时间窗去重。
            emit_event(hwnd, state, OverlayEvent::FenceCollapseToggle { fence });
        }
        Some(CollapseTarget::Title(fence)) => {
            emit_event(hwnd, state, OverlayEvent::FenceTitleDoubleClicked { fence });
        }
        None => {}
    }
}

/// 未命中图标时，双击（或快速连击）落在哪个收展目标上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollapseTarget {
    /// 标题栏右侧的收展按钮（按一次切换一次）。
    Button(usize),
    /// 标题栏空白处（双击切换）。
    Title(usize),
}

/// 解析收展命中目标。按钮绘制在标题栏内部，故必须**先判按钮**再判标题栏，
/// 否则按钮会被标题栏抢走；且每个栅栏至多返回一个目标，避免同一次点击既算
/// 按钮又算标题栏而切换两次。
fn collapse_target_at(model: &HitModel, mx: f32, my: f32) -> Option<CollapseTarget> {
    for f in &model.fences {
        if let Some(btn) = f.collapse_btn {
            if btn.contains(mx, my) {
                return Some(CollapseTarget::Button(f.id));
            }
        }
        if f.title.contains(mx, my) {
            return Some(CollapseTarget::Title(f.id));
        }
    }
    None
}

/// 把事件交给 App 回调，并用回调返回的新命中模型更新区域与命中数据。
fn emit_event(hwnd: HWND, state: &mut WindowState, event: OverlayEvent) {
    if let Some(handler) = &mut state.handler {
        // None = 模态期间再入被丢弃，保持当前命中模型（见 `set_event_handler` 注释）
        if let Some(model) = handler(event) {
            state.model = model;
            apply_region(hwnd, &state.model, &mut state.last_region);
        }
    }
}

/// 读取 IME 合成/结果字符串（GCS_COMPSTR / GCS_RESULTSTR）。
fn ime_string(hwnd: HWND, index: IME_COMPOSITION_STRING) -> String {
    let ctx = unsafe { ImmGetContext(hwnd) };
    if ctx.0.is_null() {
        return String::new();
    }
    let len = unsafe { ImmGetCompositionStringW(ctx, index, None, 0) };
    let mut out = String::new();
    if len > 0 {
        let mut buf = vec![0u16; (len as usize) / 2];
        unsafe {
            ImmGetCompositionStringW(
                ctx,
                index,
                Some(buf.as_mut_ptr() as *mut _ as *mut core::ffi::c_void),
                len as u32,
            );
        }
        out = String::from_utf16_lossy(&buf);
    }
    unsafe {
        let _ = ImmReleaseContext(hwnd, ctx);
    }
    out
}

/// 鼠标消息 lParam → 客户端坐标（低/高 16 位有符号）。
fn client_point(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 as u16) as i16 as i32;
    let y = ((lparam.0 >> 16) as u16) as i16 as i32;
    (x as f32, y as f32)
}

/// 取动画节拍定时器句柄（惰性创建，长期复用）。
///
/// 优先 `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`（Win10 1803+）：它不受系统
/// 15.625ms 定时器栅格约束，实测节拍稳定 16ms。失败则退化为普通可等待定时器
/// （节拍仍比 `WM_TIMER` 规整），两者都不成才返回 `None` 让调用方走 `SetTimer` 兜底。
fn anim_clock() -> Option<HANDLE> {
    let cached = HANDLE(ANIM_TIMER_HANDLE.load(Ordering::Relaxed));
    if !cached.is_invalid() {
        return Some(cached);
    }
    // 最小权限：只需改状态 + 等待，不要 TIMER_ALL_ACCESS。
    let rights = (TIMER_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE).0;
    let h = unsafe {
        CreateWaitableTimerExW(
            None,
            PCWSTR::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            rights,
        )
        .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, rights))
        .unwrap_or_default()
    };
    if h.is_invalid() {
        tracing::warn!("可等待定时器创建失败，动画节拍降级到 SetTimer/WM_TIMER");
        return None;
    }
    ANIM_TIMER_HANDLE.store(h.0, Ordering::Relaxed);
    Some(h)
}

/// 拆除节拍时钟（两种实现都要能停）。
fn disarm_anim_clock(hwnd: HWND) {
    if ANIM_CLOCK_FALLBACK.swap(false, Ordering::Relaxed) {
        unsafe {
            let _ = KillTimer(Some(hwnd), ANIM_TIMER_FALLBACK);
        }
        return;
    }
    let h = HANDLE(ANIM_TIMER_HANDLE.load(Ordering::Relaxed));
    if !h.is_invalid() {
        unsafe {
            let _ = CancelWaitableTimer(h);
        }
    }
}

/// 兜底时钟：`SetTimer`/`WM_TIMER`。节拍退回 15.6ms 栅格（≈41fps），
/// 但至少补间能走完——时钟彻底停摆会让面板永远卡在半开状态。
fn arm_fallback_clock(hwnd: HWND) {
    unsafe {
        let _ = SetTimer(Some(hwnd), ANIM_TIMER_FALLBACK, ANIM_MS, None);
    }
    ANIM_CLOCK_FALLBACK.store(true, Ordering::Relaxed);
}

/// 处理消息直到收到 `WM_QUIT`（`PostQuitMessage`）。返回后线程退出。
///
/// 两种等待形态：
/// - **空闲**（无动画）：阻塞式 `GetMessageW`，线程挂起、0% CPU；
/// - **动画中**（`ANIM_TIMER_ARMED`）：`MsgWaitForMultipleObjectsEx` 同时等消息与
///   高分辨率节拍定时器。不能直接沿用 `SetTimer`/`WM_TIMER`——那套走系统 ~15.6ms
///   栅格，请求 16ms 实测得到 15/30ms 交替的节拍，240ms 补间只有 10 帧。
pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let h = HANDLE(ANIM_TIMER_HANDLE.load(Ordering::Relaxed));
            if ANIM_TIMER_ARMED.load(Ordering::Relaxed) && !h.is_invalid() {
                // 节拍与消息任一就绪即返回（无新消息时不会因「队列非空」空转）。
                let rc = MsgWaitForMultipleObjectsEx(
                    Some(std::slice::from_ref(&h)),
                    INFINITE,
                    QS_ALLINPUT,
                    MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0),
                );
                if rc.0 == WAIT_FAILED.0 {
                    // 等待本身失败（句柄异常等）：绝不能带着坏句柄重进本分支——
                    // 那会以 100% CPU 空转。降级到兜底时钟并回到阻塞等待。
                    tracing::error!("动画节拍等待失败，降级到 SetTimer/WM_TIMER");
                    ANIM_TIMER_ARMED.store(false, Ordering::Relaxed);
                    arm_fallback_clock(HWND(OVERLAY_HWND.load(Ordering::Relaxed)));
                    continue;
                }
                if rc.0 == WAIT_OBJECT_0.0 {
                    let _ = PostMessageW(
                        Some(HWND(OVERLAY_HWND.load(Ordering::Relaxed))),
                        WM_APP_ANIM_TICK,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                // 排空：等待只在新消息到达时返回，必须一次取净再回到等待。
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            } else if GetMessageW(&mut msg, None, 0, 0).0 != 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else {
                return; // WM_QUIT
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectf_contains_checks_rectangle() {
        let r = RectF {
            x: 100.0,
            y: 200.0,
            w: 300.0,
            h: 400.0,
        };
        assert!(r.contains(100.0, 200.0));
        assert!(r.contains(400.0, 600.0));
        assert!(!r.contains(99.0, 200.0));
        assert!(!r.contains(100.0, 601.0));
    }

    #[test]
    fn client_point_sign_extension() {
        // 负坐标（主屏左侧的副屏）：低/高 16 位带符号
        let lp = LPARAM(0xFF0C_FE0Cu32 as i32 as isize); // 低 16 位 x=-500，高 16 位 y=-244
        let (x, y) = client_point(lp);
        assert_eq!(x, -500.0);
        assert_eq!(y, -244.0);
    }

    #[test]
    fn drag_resolve_delta_from_orig() {
        // 模拟移动拖拽的坐标运算：始终相对按下时的原始矩形，避免累计漂移。
        let orig = RectF {
            x: 120.0,
            y: 90.0,
            w: 400.0,
            h: 240.0,
        };
        let start = (300.0, 150.0);
        let now = (360.0, 220.0);
        let pos = (orig.x + (now.0 - start.0), orig.y + (now.1 - start.1));
        assert_eq!(pos, (180.0, 160.0));

        let size = (orig.w + (now.0 - start.0), orig.h + (now.1 - start.1));
        assert_eq!(size, (460.0, 310.0));
    }

    #[test]
    fn build_region_unions_without_panic() {
        // 纯 GDI 区域运算（无需 GPU/窗口）：验证合并逻辑可跑且能正确释放中间资源。
        let model = HitModel {
            fences: vec![
                FenceHit {
                    body: RectF {
                        x: 10.0,
                        y: 10.0,
                        w: 100.0,
                        h: 80.0,
                    },
                    title: RectF {
                        x: 10.0,
                        y: 10.0,
                        w: 100.0,
                        h: 40.0,
                    },
                    grip: RectF {
                        x: 84.0,
                        y: 64.0,
                        w: 26.0,
                        h: 26.0,
                    },
                    id: 0,
                    tooltip: None,
                    is_sidebar: false,
                    collapse_btn: None,
                    collapsed: false,
                },
                FenceHit {
                    body: RectF {
                        x: 60.0,
                        y: 50.0,
                        w: 120.0,
                        h: 90.0,
                    },
                    title: RectF {
                        x: 60.0,
                        y: 50.0,
                        w: 120.0,
                        h: 40.0,
                    },
                    grip: RectF {
                        x: 154.0,
                        y: 114.0,
                        w: 26.0,
                        h: 26.0,
                    },
                    id: 1,
                    tooltip: None,
                    is_sidebar: false,
                    collapse_btn: None,
                    collapsed: false,
                },
            ],
            icons: vec![],
            console: None,
            edit_rect: None,
            reserved: vec![],
        };
        let rgn = build_region(&model).expect("有栅栏就应有区域");

        // 并入一个落在全部栅栏之外的占位框：区域必须随之改变——占位框大于收起后的
        // 可见体，不进区域就会被 `SetWindowRgn` 裁掉（完全看不见）。
        let only_reserved = HitModel {
            fences: vec![],
            icons: vec![],
            console: None,
            edit_rect: None,
            reserved: vec![RectF {
                x: 400.0,
                y: 400.0,
                w: 120.0,
                h: 200.0,
            }],
        };
        let rgn_reserved = build_region(&only_reserved).expect("只有占位框也应构成区域");
        unsafe {
            assert!(
                EqualRgn(rgn, rgn_reserved) != TRUE,
                "占位框必须并入窗口区域，否则会被区域裁掉"
            );
            let _ = DeleteObject(rgn.into());
            let _ = DeleteObject(rgn_reserved.into());
        }
    }

    #[test]
    fn collapsed_fence_disallows_resize() {
        let f = FenceHit {
            body: RectF {
                x: 100.0,
                y: 100.0,
                w: 200.0,
                h: 36.0,
            },
            title: RectF {
                x: 100.0,
                y: 100.0,
                w: 200.0,
                h: 36.0,
            },
            grip: RectF {
                x: 274.0,
                y: 110.0,
                w: 26.0,
                h: 26.0,
            },
            id: 0,
            tooltip: None,
            is_sidebar: false,
            collapse_btn: Some(RectF {
                x: 270.0,
                y: 105.0,
                w: 20.0,
                h: 20.0,
            }),
            collapsed: true,
        };

        // 折叠栅栏无论在右下角、底部还是右边缘，均禁止判定为缩放区域
        assert_eq!(resize_zone_at(&f, 298.0, 134.0), None);
        assert_eq!(resize_zone_at(&f, 200.0, 135.0), None);
        assert_eq!(resize_zone_at(&f, 298.0, 115.0), None);
    }

    /// 构造单栅栏命中模型：标题栏 (100,100) 200x36，收展按钮 (270,105) 20x20 —— 按钮落在标题栏内部。
    fn model_with_collapse_button(collapse_btn: Option<RectF>) -> HitModel {
        HitModel {
            fences: vec![FenceHit {
                body: RectF {
                    x: 100.0,
                    y: 100.0,
                    w: 200.0,
                    h: 200.0,
                },
                title: RectF {
                    x: 100.0,
                    y: 100.0,
                    w: 200.0,
                    h: 36.0,
                },
                grip: RectF {
                    x: 274.0,
                    y: 274.0,
                    w: 26.0,
                    h: 26.0,
                },
                id: 0,
                tooltip: None,
                is_sidebar: false,
                collapse_btn,
                collapsed: false,
            }],
            icons: vec![],
            console: None,
            edit_rect: None,
            reserved: vec![],
        }
    }

    /// 构造单栅栏命中模型（(100,100) 200x200，标题栏高 36）+ 一个落在栅栏之外的占位框。
    fn model_with_reserved(reserved: Vec<RectF>) -> HitModel {
        HitModel {
            reserved,
            ..model_with_collapse_button(None)
        }
    }

    #[test]
    fn reserved_rect_joins_region_but_is_not_a_hit_target() {
        // 占位框只是「可见的碰撞提示」：并入窗口区域以能绘制，但绝不能被当成
        // 栅栏/图标命中——否则会出现看不见却能点的幽灵热区。
        let model = model_with_reserved(vec![RectF {
            x: 100.0,
            y: 100.0,
            w: 200.0,
            h: 400.0,
        }]);
        // 落在标题栏之外、占位框之内的点：不可命中任何栅栏/图标/折叠目标
        assert_eq!(hover_key(&model, 200.0, 450.0), None);
        assert_eq!(collapse_target_at(&model, 200.0, 450.0), None);
        // 同一模型并入占位框后仍能构建区域（说明它只影响区域，不影响命中集合）
        let rgn = build_region(&model).expect("应能构建区域");
        unsafe {
            let _ = DeleteObject(rgn.into());
        }
    }

    #[test]
    fn collapse_target_button_beats_title() {
        // 按钮完全落在标题栏内：必须先判按钮，否则收展按钮会被标题栏抢走，
        // 变成「双击标题栏」而非「按一次切换一次」。
        let model = model_with_collapse_button(Some(RectF {
            x: 270.0,
            y: 105.0,
            w: 20.0,
            h: 20.0,
        }));
        assert_eq!(
            collapse_target_at(&model, 280.0, 115.0),
            Some(CollapseTarget::Button(0))
        );
    }

    #[test]
    fn collapse_target_falls_back_to_title() {
        // 标题栏左侧空白（不在按钮内）→ 标题栏目标
        let model = model_with_collapse_button(Some(RectF {
            x: 270.0,
            y: 105.0,
            w: 20.0,
            h: 20.0,
        }));
        assert_eq!(
            collapse_target_at(&model, 150.0, 115.0),
            Some(CollapseTarget::Title(0))
        );
    }

    #[test]
    fn collapse_target_without_button_is_title() {
        // 侧边栏 Dock 不生成收展按钮（collapse_btn = None）：同一位置退化为标题栏目标，
        // 且绝不能凭空报出 Button。
        let model = model_with_collapse_button(None);
        assert_eq!(
            collapse_target_at(&model, 280.0, 115.0),
            Some(CollapseTarget::Title(0))
        );
    }

    #[test]
    fn collapse_target_outside_fences_is_none() {
        let model = model_with_collapse_button(Some(RectF {
            x: 270.0,
            y: 105.0,
            w: 20.0,
            h: 20.0,
        }));
        assert_eq!(collapse_target_at(&model, 500.0, 500.0), None);
    }

    #[test]
    fn repeatable_console_zones_are_the_pure_setters() {
        // 纯设置项：重复触发与触发一次结果相同，允许快速连击的第二次按下生效
        for zone in [
            ConsoleZone::FenceSelect(2),
            ConsoleZone::FenceLayout(FenceLayout::List),
            ConsoleZone::FenceIconSize(72.0),
            ConsoleZone::FenceStyle(FenceStyle::Glass),
            ConsoleZone::FenceSidebarPos(SidebarPosition::Left),
            ConsoleZone::FenceTint(Some([1.0, 0.0, 0.0])),
            ConsoleZone::FenceRulePreset(None),
        ] {
            assert!(console_zone_is_repeatable(zone), "{zone:?} 应为可重复控件");
        }
    }

    #[test]
    fn side_effecting_console_zones_are_never_repeated() {
        // 重复执行有真实副作用的控件：一次双击绝不能触发两次
        for zone in [
            ConsoleZone::Close,
            ConsoleZone::AddFence,
            ConsoleZone::RemoveFence,
            ConsoleZone::ChangeStoragePath,
            ConsoleZone::AutoOrganize,
            // 纯状态翻转，但每次翻转都伴随图标层 ShowWindow 与整屏淡入淡出，会闪
            ConsoleZone::DesktopToggle,
            ConsoleZone::AutostartToggle,
            // 目前不会被 zones.push 的死控件：默认 fail-safe 拒绝
            ConsoleZone::Expand,
            ConsoleZone::Tab(1),
            // 设置页与快捷键控件
            ConsoleZone::ToggleSettingsPage,
            ConsoleZone::HotkeyRecord(winbosk_core::hotkey::HotkeyAction::ConsoleToggle),
            ConsoleZone::HotkeyClear(winbosk_core::hotkey::HotkeyAction::ConsoleToggle),
            ConsoleZone::HotkeyResetDefault,
        ] {
            assert!(!console_zone_is_repeatable(zone), "{zone:?} 不得被重复触发");
        }
    }

    #[test]
    fn console_zone_at_ignores_zero_area_placeholders() {
        // 未显示的控件用 RectF::default() = (0,0,0,0) 占位；RectF::contains 含边界，
        // 若不排除零面积矩形，客户端原点 (0,0) 会命中这些并不存在的「幽灵控件」。
        let c = ConsoleHit {
            rect: RectF {
                x: 0.0,
                y: 0.0,
                w: 300.0,
                h: 200.0,
            },
            title: RectF {
                x: 0.0,
                y: 0.0,
                w: 300.0,
                h: 30.0,
            },
            zones: vec![
                (ConsoleZone::FenceIconSize(32.0), RectF::default()),
                (
                    ConsoleZone::FenceSelect(0),
                    RectF {
                        x: 10.0,
                        y: 40.0,
                        w: 100.0,
                        h: 20.0,
                    },
                ),
            ],
        };
        // 原点不再命中占位控件
        assert_eq!(console_zone_at(&c, 0.0, 0.0), None);
        // 正常控件仍可命中
        assert_eq!(
            console_zone_at(&c, 50.0, 50.0),
            Some(ConsoleZone::FenceSelect(0))
        );
    }
}
