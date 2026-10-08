# 控制中心临时提权（唤出置顶 / 收起归位）实施计划

背景：overlay 以 WorkerW 为 owner 锚定在桌面带（见 `crates/render/src/overlay.rs` 的 `create`
与 `crates/shell/src/takeover.rs::overlay_parent`），栅栏与控制中心因此永远被普通应用窗口
（浏览器等）遮挡。这是桌面整理器的正确默认层级，但用户**显式唤出**控制中心时，面板被浏览器
盖住导致看不清、点不准。本计划为「控制中心会话」引入临时 Z 序提权：唤出即浮于普通窗口之上，
收起立即落回桌面带。

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

- **G1 唤出提权**：用户主动展开控制中心面板的瞬间，overlay 提升到普通 Z 序带顶部
  （`HWND_TOP`）——盖住浏览器等普通窗口，仍低于真正的 TOPMOST 窗口（任务栏等）。
- **G2 收起归位**：用户收起面板的瞬间，overlay 插回 owner（WorkerW）**正上方**，精确落回
  桌面带，恢复「栅栏属于桌面」的原有层级语义（锚点语义见 §2）。
- **G3 单一出口**：全部 5 处开/关路径收敛为 app 层唯一状态变迁函数，`desk.console_open`
  的写入与 overlay Z 序在同一函数内同步，杜绝组合态漂移。
- **G4 保持激活语义**：提权用 `SWP_NOACTIVATE`，不激活 overlay 本体，键盘输入继续走
  既有「隐藏焦点代理」通路，桌面壳层不被提到应用之上。

### Non-Goals

- 不改 WorkerW/Progman 层级结构：不 `SetParent`、不重挂、不销毁（AGENTS.md 约束 #3）。
- 不给控制中心创建独立 HWND——它继续绘制在 overlay 的 DirectComposition 视觉树内；
  提权的是 overlay 本体。
- 不给栅栏本体提供提权：拖拽、悬停、编辑不触发 Z 序变化。
- 不持久化提权状态：重启后 overlay 始终在桌面带，`console_open` 的持久化语义不变；
  提权仅存在于「本会话显式唤出」期间。
- 不改控制中心的视觉、动画与 `console_size`/`console_pos` 逻辑。

## 2. 状态契约与接口设计 (Contracts & Data Models)

### render 层（`crates/render/src/overlay.rs`）

```rust
pub struct OverlayWindow {
    // ...既有字段不变...
    /// create 时的父/owner 窗口（持有 DefView 的 WorkerW，无 DefView 时为 Progman）。
    /// 仅用于收起控制中心时把窗口插回 owner 之后、落回桌面带。
    owner: HWND,
}

impl OverlayWindow {
    /// 控制中心唤出：把 overlay 提到普通 Z 序带顶部。
    /// 不激活（SWP_NOACTIVATE，激活会把桌面壳层提到应用之上）、不动几何。
    pub fn raise_console(&self);

    /// 控制中心收起：插回 owner 之后，回落桌面带。
    pub fn restore_desktop_band(&self);
}
```

单位与度量口径：

- Z 序操作统一 `SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOREDRAW | SWP_NOOWNERZORDER`，
  只动 `hWndInsertAfter`；几何与激活一律不动（几何归 `resize`，激活归 `focus_for_input`/
  `raise_to_foreground`，二者职责不混）。
- **`hWndInsertAfter` 方向语义（实测确立）**：被定位窗口插到锚点窗口**之下**。
  - `raise_console`：锚点 `HWND_TOP`（普通带顶部），且**必须**经 `with_foreground_lock`
    （AttachThreadInput）——WinBosk 是后台进程，裸调 `SetWindowPos(HWND_TOP)` 会被系统
    静默拒绝（返回 TRUE 但 Z 序不动，已实测复现）；与 `raise_to_foreground` 同一套手法。
  - `restore_desktop_band`：目标位置是「owner **正上方**」，因此锚点取「owner 当前正上方
    的窗口」= `GetWindow(GetAncestor(owner, GA_ROOT), GW_HWNDPREV)`；直接传 owner 会把
    overlay 插到 WorkerW 之下、壁纸之后（栅栏整体不可见不可点，已实测复现）。`GA_ROOT`
    归一处理 owner 为子窗口（`SHELLDLL_DefView`）的拓扑；锚点不存在（owner 之上无窗口的
    理论边界）时跳过恢复、保持现状——留在普通带优于误落到壁纸之后。
- 二者均为一次性同步调用，无返回值传播（`SetWindowPos` 失败仅忽略，容错口径与 `resize`
  一致）。
- 坐标/尺寸零参数（NOMOVE/NOSIZE），不存在 DPI 口径问题。

### app 层（`crates/app/src/main.rs`）

```rust
/// 控制中心开合的单一状态变迁出口：写状态 → 持久化 → 同步 overlay Z 序 → 驱动补间。
/// 仓库内禁止在其它位置写 `desk.console_open`。
pub(crate) fn set_console_open(rt: &mut Runtime, open: bool);
```

时序（同一事务内，顺序固定）：

1. `rt.desk.console_open = open`（状态事实源先行）；
2. `rt.store.save(&rt.desk)`（失败仅 `tracing::warn!`，与现状一致，不阻断）；
3. Z 序同步：`open` → `raise_console()`；`!open` → `restore_desktop_band()`；
4. `start_panel_tween(rt, if open { 1.0 } else { 0.0 })`（补间只管视觉，不管层级）。

调用点改写（行为逐一等价替换，仅新增第 3 步）：

| 现位置 | 触发 | 改写为 |
| :--- | :--- | :--- |
| `main.rs` `ConsoleZone::Close` | 点面板「关闭」 | `set_console_open(rt, false)` |
| `main.rs` `ConsoleZone::Expand` | 点胶囊展开 | `set_console_open(rt, true)` |
| `main.rs` `OverlayEvent::ConsoleToggle` | 热键 Ctrl+Alt+T | `set_console_open(rt, !open)` |
| `main.rs` `OverlayEvent::TrayToggle` | 托盘左键单击 | `set_console_open(rt, !open)` |
| `context_menu.rs` `MENU_TRAY_CONSOLE` | 托盘菜单项 | `set_console_open(rt, !open)` |

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

- **常驻后台与同步引擎**：`SyncLibrary` 4s 定时器、动画定时器、托盘去重（`LAST_TRAY_CLICK_MS`）
  均不感知 Z 序，零交互。`SetWindowPos` 是一次性同步调用，不引入任何周期性开销。
- **空闲性能归零律**：提权/回落不启定时器、`SWP_NOREDRAW` 抑制无效重绘；窗口内容是 WinRT
  合成树，DWM 自动按新层级重采样 BackdropBrush（GPU 合成，与现状同构，无 CPU 增量）。
  补间结束后既有 `advance_anim` 机制照常停用 `AnimTick`，回到 0% CPU。
- **wnd_proc 内再入**：`set_console_open` 在 `handle_event` 内执行，而 `handle_event` 运行于
  事件回调闭包中（wnd_proc 栈上）。`SetWindowPos` 会同步派发 `WM_WINDOWPOSCHANGING/CHANGED`
  形成再入 wnd_proc——这两个消息不被 wnd_proc 特殊处理（走 `DefWindowProc`），且嵌套事件
  本就被 `HANDLING` 标志丢弃（`main.rs` 事件回调首行），无 `RefCell` 借用冲突风险。
- **孤儿状态回收律**：owner（WorkerW）若被系统销毁，Windows 所有权语义会一并销毁 owned
  overlay——「overlay 存活而 owner 悬空」在运行态不存在，`owner` 句柄与既有所有权假设同源，
  无新增幽灵句柄路径。`restore_desktop_band` 调用失败仅忽略。**注意**：Win+D（显示桌面）
  不重排桌面带内部相对次序（实测），不能作为恢复失位的自愈兜底——恢复正确性只能靠锚点
  语义本身（见 §2）与人工验证矩阵第 2 例的「栅栏仍可见」断言。
- **显示器拓扑变化**：`DisplayChange` 仅 `resize`（`SWP_NOZORDER`，不动 Z 序）——提权状态
  跨拓扑变化保持，与 `console_open=true` 一致；不重探测 WorkerW（现状即如此，owner 生命周期
  由所有权机制保证）。
- **启动语义**：`console_open` 持久化为 `true` 时，启动后面板展开但 overlay 仍在桌面带，
  **不**提权——提权语义限定为「本会话用户显式唤出」，此时点任意开关路径即进入正确的同步状态。

## 4. 分层改动清单 (Implementation Steps)

自底向上，共 2 层 3 个文件：

1. **render 层**（`crates/render/src/overlay.rs`）
   - `OverlayWindow` 增加私有字段 `owner: HWND`；`create` 记录传入的 `parent`（`Ok(Self{..})`
     处补一行）。
   - 新增 `raise_console()`（经 `with_foreground_lock`，insert-after `HWND_TOP`）与
     `restore_desktop_band()`（锚点 = `GetWindow(GetAncestor(owner, GA_ROOT), GW_HWNDPREV)`，
     即 owner 正上方窗口；无锚点时跳过），flags 见 §2。注释写明 `hWndInsertAfter` 的
     「插到之下」方向语义与后台进程 `HWND_TOP` 静默拒绝这两个实测约束。
   - imports 增补：`HWND_TOP`、`SWP_NOMOVE`、`SWP_NOSIZE`、`GetAncestor`、`GetWindow`、
     `GA_ROOT`、`GW_HWNDPREV`。
2. **app 层**（`crates/app/src/main.rs`）
   - 新增 `pub(crate) fn set_console_open(rt: &mut Runtime, open: bool)`（置于 `handle_event`
     之前），实现按 §2 时序。
   - `ConsoleZone::Close` / `ConsoleZone::Expand` / `ConsoleToggle` / `TrayToggle` 四处
     改写为调用该函数（删除各自重复的状态写入+保存+补间代码）。
3. **app 层**（`crates/app/src/context_menu.rs`）
   - `MENU_TRAY_CONSOLE` 分支改写为 `set_console_open(rt, !rt.desk.console_open)`。

## 5. 防御性自查清单 (Defensive Invariants)

对照「隐性动力学自查六律」逐项：

| 律 | 针对性防御 |
| :--- | :--- |
| 1 常驻后台冲突 | Z 序不参与任何轮询/同步引擎的判定；`SyncLibrary`、托盘去重、动画定时器零感知。 |
| 2 空闲归零 | 提权/回落无定时器、`SWP_NOREDRAW`；补间结束路径复用既有停表机制，空闲仍 0% CPU。 |
| 3 孤儿回收 | `owner` 与 overlay 生命周期被 Windows 所有权绑定（owner 死则 overlay 死），恢复路径无悬空句柄运行态；恢复锚点按 §2 语义精确落位，失败静默。 |
| 4 语义精准 | 唯一新增状态 `owner` 来自 `create` 参数（结构性事实），非下标/位置推断。 |
| 5 状态全集 | `console_open` 全仓库仅 `set_console_open` 一处写入（另有 serde 默认值），状态与 Z 序同函数同步，无「开而未提 / 关而未落」组合态。 |
| 6 旁路对齐 | Z 序是 `console_open` 的运行时影子，由单一出口强制对齐；审查断言：`grep -rn "console_open\s*=" crates/app` 仅命中 `set_console_open` 函数体。 |

已知并接受的行为（写入代码注释与用户文档口径）：

- 提权期间**栅栏与胶囊一并**浮于普通窗口之上——单 overlay 窗口架构的必然结果，视为
  「控制中心会话」的一致语义；区域裁剪保证非栅栏/面板区域照常穿透点击。
- 收起瞬间回落桌面带，随后的折叠补间可能被浏览器遮挡——收起即视为用户结束会话，可接受。
- 全屏独占应用下 overlay 本就不可见（现状），提权不改变该事实。

## 6. 验证与交付门禁 (Verification Gates)

自动化门禁（全绿为交付前提）：

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

静态断言：

- `grep -rn "console_open\s*=" crates/app` → 仅 `set_console_open` 函数体一处写入。
- `grep -rn "HWND_TOPMOST\|WS_EX_TOPMOST" crates` → 零命中（本方案只用 `HWND_TOP`，
  不引入 TOPMOST 带语义）。

运行冒烟：

```powershell
$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe
```

手动用例矩阵（浏览器最大化作为遮挡参照）：

| # | 操作 | 期望 |
| :--- | :--- | :--- |
| 1 | Ctrl+Alt+T 打开 | 面板浮于浏览器之上；任务栏（TOPMOST）仍可见；键盘输入正常 |
| 2 | 再按 Ctrl+Alt+T 收起 | 立即回落桌面带，被浏览器遮挡（胶囊同）；**栅栏本体仍可见**（未被壁纸吞没——审查确立的关键断言） |
| 3 | 托盘左键 / 托盘菜单项 / 点胶囊 / 点「关闭」 | 与 1/2 同语义，四路径行为一致 |
| 4 | 提权状态下点面板外区域 | 穿透点击到浏览器（区域裁剪不变） |
| 5 | 提权状态下 Win+D | 普通带窗口最小化、桌面露出；浏览器还原后重新盖住面板属**良性态**（等价启动时 `console_open=true`），关-开控制中心即恢复同步 |
| 6 | 提权状态下改分辨率/拔显示器 | overlay 重设几何且保持提权，无错位（`SWP_NOZORDER`） |

对抗性审查要点（独立干净上下文子代理）：

- `SetWindowPos` 在 wnd_proc 栈内同步再入的借用与重入安全（`HANDLING` 守卫覆盖论证）。
- `owner` 句柄生命周期与所有权销毁语义的论证是否成立。
- 5 处改写是否严格等价（持久化失败容错、补间参数、事件顺序）。
- 是否存在本计划遗漏的 `console_open` 写入路径或会令 Z 序与状态脱钩的新路径。

## 7. 审查修正记录 (Review Corrections)

独立对抗审查（Win32 实证法：`rustc` 单文件探针复刻 overlay 的窗口样式、所有权关系与
SWP flags，6 组实验交叉验证）推翻了本计划初版的两处前提，已修正并落实：

1. **[致命→已修] `restore_desktop_band` 锚点方向**：初版传 `self.owner` 作
   `hWndInsertAfter`，实测把 overlay 插到 WorkerW **之下**（`GW_HWNDPREV(overlay)==owner`），
   一次开→关后栅栏永久不可见不可点，且 Win+D 不重排桌面带内部次序、无自愈。修正为锚点
   取「owner 正上方窗口」（`GA_ROOT` 归一 + `GW_HWNDPREV`，无锚点跳过）。
2. **[严重→已修] 后台进程 `HWND_TOP` 静默拒绝**：初版裸调 `SetWindowPos(HWND_TOP)`，
   实测返回 TRUE 但 Z 序不动（进程非前台时）；套用仓库既有 `with_foreground_lock` 后
   同一调用成功提权。修正为 `raise_console` 全程经 `with_foreground_lock`。
3. **[低→有意统一] 持久化失败日志口径**：`TrayToggle` 与 `MENU_TRAY_CONSOLE` 原为
   `let _ =` 静默容错，收敛进 `set_console_open` 后统一为 `tracing::warn!`，与其余三处
   调用点既有口径一致——属有意统一，非行为回归。

## 8. 第二迭代：点击唤起与离开回落 (Iteration 2: Click-to-Raise & Leave-to-Restore)

### 8.1 场景与缺陷

第一迭代只在 `set_console_open` 的状态迁移瞬间提权一次。用户唤出控制中心后被其它程序
激活（非全屏），提权中的 overlay 被盖住；此后点击露出的栅栏/面板边缘——点击确实落在
Sylva 表面（交互有效），但 `WS_EX_NOACTIVATE` 使系统永远不会因点击激活/提层它，而提权
调用只在开/关时发生——**用户主动点击却无法把 Sylva 重新带到前面**。典型诉求：不全屏使
用其它程序、想临时打开某个栅栏里的桌面应用。

### 8.2 契约扩展

- **点击唤起（render 层窗口机制）**：`WM_LBUTTONDOWN` / `WM_RBUTTONDOWN` 到达 wnd_proc
  即意味着点击落在窗口区域（= 栅栏 ∪ 控制台 ∪ 占位框）内，属于用户对 Sylva 表面的主动
  操作 → 先 `raise_hwnd_to_normal_top(hwnd)` 再分发。不缓存「已提权」标志——被其它程序盖住
  时标志必然失真，每次按下重断言（已在顶部时为无害空操作）。滚轮不触发（语义收紧到
  「按下 = 意图」）。
- **离开回落（条件恢复）**：`WM_MOUSELEAVE` 时，若「控制中心会话」**未激活**且**无拖拽
  进行中**（`state.drag.is_none()`，覆盖 Move/Select/Resize/ConsoleMove/ConsoleResize/
  SidebarReorder 全部六类），落回桌面带。
- **会话位（业务位单向推送）**：`WindowState` 新增 `console_session: bool` 与
  `owner: HWND`（create 时的 parent，供 wnd_proc 恢复用；与 `OverlayWindow.owner` 同源
  不可变，无失同步面）。app 的 `set_console_open` 在 Z 序同步的同一事务里调
  `overlay.set_console_session(open)` 推送。控制中心会话期间离开不回落（生命周期仍由
  开/关配对管理）；非会话期的点击唤起（点栅栏临时用一下）随光标离开 Sylva 表面而结束
  ——「临时」语义由光标位置界定，无需新增退出事件。
- **自由函数提取**：`raise_hwnd_to_normal_top(hwnd)` 与 `restore_hwnd_desktop_band(hwnd, owner)`
  从方法中抽出供 wnd_proc 复用；`raise_console` / `restore_desktop_band` 成为薄委托，
  语义升级为「用户前台会话」的提权/归位（不限于控制中心开合）。

### 8.3 动力学推演

- **拖拽中不回落**：拖动栅栏/框选/面板缩放时光标可能瞬时离开区域（capture 下消息仍到
  overlay，leave 可能触发），`drag.is_none()` 门控阻断之；拖放结束后光标仍在表面 → 保持
  提权，移开即回落。
- **从栅栏启动应用**：双击图标 → 应用成为前台（普通带顶部，自然盖住 Sylva）；光标移向
  新应用 → leave → 回落。连续启动多个应用期间光标未离开表面则保持提权——无需在启动
  路径上显式回落。
- **自愈性**：用户点击其它程序窗口 → 该窗口激活升到普通带顶部、重新盖住 Sylva（良性，
  「最后点击者在前」的常规窗口语义）；再次点击 Sylva 露出部分即重新唤起。
- **幂等**：已在顶部时 `HWND_TOP` 为空操作；已在桌面带（owner 正上方）时恢复的锚点即
  overlay 自身（复验实验证实无害）。
- **空闲归零**：唤起/回落均为用户交互驱动的一次性调用，不引入任何定时器。
- **已知边界**：内联编辑（InlineEdit）是纯 D2D 绘制、编辑区属于 overlay 命中模型与窗口
  区域，**不存在子窗口**，点击编辑区与点击栅栏同走 wnd_proc（唤起/回落语义一致）；
  `WM_CTLCOLOREDIT`/`edit_brush` 是无消费者的遗留防御路径。注意（审查实证）：**若**未来
  给 overlay 挂真实子窗口（EDIT 等），光标悬停子窗口会触发父窗口 `WM_MOUSELEAVE`——届时
  必须把 `console_session`/拖拽门控扩展覆盖子窗口悬停场景，否则非会话期编辑中面板会被
  回落到桌面带。

### 8.4 验证补充

在 §6 矩阵之上追加：

| # | 操作 | 期望 |
| :--- | :--- | :--- |
| 7 | 唤出控制中心 → 激活其它程序盖住它 → 点击露出的面板/栅栏边缘 | Sylva 整体重回普通带顶部（本迭代的缺陷修复） |
| 8 | 控制中心收起状态点栅栏 → 光标移到其它程序窗口 | 点击瞬间提权；移出 Sylva 表面即回落桌面带 |
| 9 | 点栅栏并拖动 → 拖拽中光标短暂划出区域 | 拖拽全程保持提权，不中途回落 |
| 10 | 点栅栏 → 双击图标启动应用 → 光标移向新应用 | 应用前台盖住 Sylva；光标离开后 Sylva 回落桌面带 |

### 8.5 审查修正记录 (Iteration 2 Review Record)

独立对抗审查（静态全分支核对 + rustc 探针实证：后台进程前台锁提权、桌面带空操作恢复、
capture 下 TME_LEAVE 时序、子窗口悬停触发父级 leave）判定可合入，零必须修复项；三项
低严重度发现的处置：

1. **[已修] §8.3「内联编辑框是子窗口 EDIT」前提失实**：InlineEdit 实为纯 D2D 绘制
   （编辑区在命中模型/窗口区域内），`WM_CTLCOLOREDIT` 是无消费者的遗留防御路径。
   §8.3 已按实证改写，并把「未来引入真实子窗口必须扩展门控」写成显式约束。
2. **[已修] capture 被外部夺走时 `state.drag` 残留**：拖拽中 capture 丢失 → 无
   `WM_LBUTTONUP` → drag 残留拦住迟到的 leave（ReleaseCapture 后约 31ms 到达）→
   点击唤起的提权滞留普通带顶部。新增 `WM_CAPTURECHANGED` 分支清 `state.drag`
   （孤儿状态回收），迟到的 leave 恢复回落；布局持久化仍以真实 `WM_LBUTTONUP` 为准。
3. **[记录在案] 「裸调 HWND_TOP 静默拒绝」是环境依赖行为**：本审查机（Win11 26200）
   探针未复现静默拒绝。保留 `with_foreground_lock` 作为无条件纵深防御（迭代一的
   实测约束在其它环境成立），不改变实现。
4. **[已修] WM_CAPTURECHANGED 与 WM_MOUSELEAVE 的悬停旁路镜像不对齐**：初版仅 emit
   CursorLeave 未重置渲染层 hovered/last_cursor/console_hovered，capture 被夺而光标
   仍在图标上时悬停高亮滞留失效态；已镜像 leave 的悬停清理并补发
   HoverLeave/ConsoleHover{zone:None}（取舍与「不回联桌面带」的理由见 §9.5）。

## 9. 第二迭代同批附带变更记录 (Adjacent Changes)

以下五项不在 §8 契约范围内，但与第二迭代同批进入工作区。按「scope creep 必须显式
记录」的审查要求补录于此——每项写明症状/动机、改法与验证锚点，使 diff 与 spec
一一对应。

### 9.1 compositor 表面扩张迟滞重写（`SURFACE_GROW` + `covering_decision`）

- **症状**：拖动边界上的栅栏（或占位框进出场）时内容包围盒持续外扩，旧逻辑只要超出
  `SURFACE_PAD = 16` 余量就重建表面——拖动中每移几像素重建一次，重建瞬间新表面未就绪，
  **全部栅栏**闪一帧。与 §9.2 的 bRedraw 闪烁同症状家族，同批修复。
- **改法**：扩张改按大块量子 `SURFACE_GROW = 256` 一次撑开，让整段拖动落在同一表面上。
  尺寸决策抽成纯函数 `covering_decision`（盖住判定仍用 16px probe；收缩基准改为
  `bbox + SURFACE_GROW` 而非 16px probe——否则小栅栏刚按 GROW 撑开就会被判 oversized
  缩回，「撑开→缩回→再撑开」闪烁回潮）。收缩基准永远从当前 `bbox` 计算、不从当前表面
  计算，故不会「水涨船高」。`ensure_covering` 只剩 GPU 资源操作；`keep_large`（Dock 在场
  禁止收缩）语义不变。
- **验证锚点**：`compositor.rs` tests 模块 5 个纯几何单测（撑开不被立刻缩回、逃逸 GROW
  余量按量子再撑、大幅缩小按 PAD 收缩、keep_large 不收缩、决策是稳定不动点），零 GPU
  依赖，CI 可跑。

### 9.2 `apply_region` 的 `SetWindowRgn` bRedraw true→false

- **症状**：overlay 是 `WS_EX_NOREDIRECTIONBITMAP` + DirectComposition 视觉树窗口（无
  GDI 客户区、无 WM_PAINT），`bRedraw=true` 仍会走「整窗失效 → DWM 拆掉再重合成」路径；
  拖动时区域每帧都变，**全部栅栏**（不只被拖的）跟着闪一下，观感即「刷新了一遍」。
- **改法**：`bRedraw=false`。区域几何本身生效（命中穿透与 DWM 可视裁剪）不依赖
  bRedraw；内容更新本就由 App 层 `present` + `RequestCommitAsync` 承担。
- **正确性依赖的顺序不变量**（审查发现的时序风险，已静态关闭并落字到 overlay.rs 与
  main.rs 注释）：内容提交必须先于命中模型/区域更新——首帧路径 main.rs 先 `present`
  再 `set_model`；事件路径按「App 处理交互 → 重绘 → 返回新命中模型」契约在
  `handle_event` 内先重绘。颠倒该顺序会露出一帧陈旧内容。
- **验证**：顺序不变量为静态代码核对（两条调用路径均成立）；拖动观感（无闪烁）属
  交互项，留 §6 冒烟矩阵桌面复验。

### 9.3 控制中心字号与桌面栅栏文字解耦

- **动机**：控制中心是管理界面，文字需整体比桌面栅栏同款大两号；此前直接共用
  `title`/`label`，两侧字号互相牵连。
- **改法**：`Theme` 新增 `console_title`(20) / `console_label`(16) 两个 `TextStyle`；
  `TextFormats` 相应扩充 `console_title_bold` / `console_close`（关闭按钮「✕」）/
  `console_label` / `console_detail`。detail 级字号 = `DETAIL_SIZE_RATIO`(0.72) ×
  `console_label.size`——比例常量与 `detail_style()` 派生构造是全链路唯一真源，app 层
  宽度预算共用之。`apply_theme_scale` 同步缩放新字段（物理像素口径不变，AGENTS.md
  约束 #9）。**几何常量（行高 36 / 按钮高 24 / 行距 30）不随字号放大**：16px 实际字形
  高（约 1.32em ≈ 21px）仍装得进 24px 按钮带。
- **验证锚点**：theme.rs 单测断言 console 字号大于桌面同款；scene.rs 宽度预算不变量
  测试以 `CONSOLE_W` / `CONSOLE_MIN_W` 参数化，常量一改即被重验。

### 9.4 `CONSOLE_W` 320→368（plan 09 Non-Goal 5 显式取代）

- **动机**：字号加大两号后「更改文件位置…」「恢复默认」两个动作按钮实测变宽
  （100·s→128·s、64·s→80·s），320 默认宽下后果提示放不下（两模式分别缺口约 18 / 32px），
  违反 plan 09「默认宽度下提示常显」的主契约——宽度必须跟着字号走。
- **改法**：`CONSOLE_W` 320→368（+48）。复算后提示预算余量 30 / 16px（复算表见
  plan 09 §9.2）；最小宽 `CONSOLE_MIN_W` 不动，窄面板仍按「整条不画」优雅降级。
- **与 plan 09 的关系**：plan 09 Non-Goal 5 是**该计划自身**的范围声明（禁止 plan 09
  的改动顺带动宽度），不是对面板宽度的永久冻结；本迭代因 §9.3 字号解耦触发它，按
  plan 09 §9 修订记录显式取代，预算表同步复算。

### 9.5 `WM_CAPTURECHANGED` 超出 §8.5.2 最小范围的事件

§8.5 第 2 条的最小修复是「清 `state.drag`」。实现额外镜像了 `WM_MOUSELEAVE` 的悬停
旁路清理（`hovered` / `last_cursor` / `console_hovered` 置空 + 补发
`ConsoleHover{zone:None}` / `HoverLeave` / `CursorLeave`）：

- **动机（旁路数据对齐）**：只清 App 层（`CursorLeave` → `rt.cursor` / `rt.hover`）而
  不清渲染层镜像，光标仍在图标上时 `HoverEnter` 不会重发，悬停高亮 / Dock 放大滞留
  失效态直到离开再进入。镜像清理后，下一次 `WM_MOUSEMOVE` 重新建立悬停。
- **幂等性**：capture 被夺后迟到的 `WM_MOUSELEAVE` 会再发一遍同组事件——App 层悬停
  状态是覆盖写，重复事件无害（与现状一致，非新增风险）。
- **刻意不做**：不在 `WM_CAPTURECHANGED` 里回联 `restore_hwnd_desktop_band`。拖拽异常
  终止时光标通常仍在表面，按 §8.2 契约应保持提权；桌面带回落由迟到的 `WM_MOUSELEAVE`
  承担（其 `drag.is_none()` 门控此时已放行）。
