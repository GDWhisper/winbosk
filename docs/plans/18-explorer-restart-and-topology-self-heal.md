# 实施计划 18：Explorer 重启自愈 + 显示拓扑瞬变回滚

> 触发事件（2026-10-29）：`Explorer.EXE` 于 02:07:23 因 WPS 的 Shell 扩展
> `qingnse64.dll` 崩溃（`0xc0000005`）并自动重启。现场表现为「WinBosk 好像退出了」：
>
> - 托盘图标永久消失（新任务栏上没有本程序图标）；
> - 真实桌面图标重新出现（新 explorer 的 `SysListView32` 是可见的），与栅栏同屏「双份」；
> - 栅栏仍可点击——overlay 是 `WS_POPUP` 顶层窗口，owner（旧 WorkerW）被内核强杀后
>   owner 被清 0（`GetWindow(GW_OWNER)=0`），窗口本身存活。
>
> 同一实例在 13:31 从睡眠唤醒时，虚拟屏先报 **1024×768**、77 秒后才回到 3840×2160；
> `WM_DISPLAYCHANGE` 的夹屏被立即写盘，导致 6 个栅栏全部挤进左上 1024 宽的竖带，
> 其中两个 bounds 完全相同（`544,0,480,330`）互相完全遮挡。
>
> 本文把这两类「外部环境变化」造成的运行期状态失真收敛成可自愈、可回滚的机制。

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

- **G1 托盘图标可重建**：Explorer 重启后，收到 `TaskbarCreated` 广播立即重新登记托盘图标
  （`NIM_ADD` 幂等），无需重启进程。
- **G2 桌面接管自愈**：运行期识别「接管已失效」（旧 `Progman`/`WorkerW`/`SysListView32`
  句柄已死，或本应隐藏的图标层又被显示出来），重做整套接管：重新 `probe` 层级、隐藏真实
  图标、把 overlay 换挂到新 `WorkerW`/`Progman`、重推 Z 序、重登托盘图标。
- **G3 退出兜底恢复「当前那份」句柄**：`IconGuard`（进程退出兜底）与 `Runtime`（运行期自愈）
  共享同一份层级句柄；自愈换新后，退出时恢复的必须是新隐藏的那份。
- **G4 拓扑瞬变不落盘**：`WM_DISPLAYCHANGE` 引起的夹屏只在内存生效，不再立即持久化；
  拓扑回到变化前的配置且用户未改过布局时，整体回滚位置。
- **G5 不欠新债**：不新增周期定时器；自愈判据挂在既有 4s `SyncLibrary` 心跳上（约束 10 不变），
  且判据本身只做几次 `IsWindow`/`IsWindowVisible`（无 I/O、无分配）。

### Non-Goals

- **不改 `with_foreground_lock`（`AttachThreadInput`）在任何路径上的用法**：plan 11 的实测约束
  （模态菜单需要前台、且该调用是 imm32→MSCTF 的已知入口）原样保留。
- **不做主线程 IME/TSF 假死的自愈**：Win32 同步调用无法安全中断；本次只记录现场栈
  （`DefWindowProc → imm32!ImmSetActiveContext → MSCTF!CtfImeProcessKey → NtWaitForSingleObject`），
  不在本计划范围。
- **不改 `crates/core` 的布局/碰撞算法**：`Fence::collision_rect` 口径不动。
- **不改 `DpiChanged` 的持久化语义**：DPI 变化通常是用户显式改设置，保留「夹屏即保存」。
- **不做拓扑快照的跨进程持久化**：快照只存在内存，进程重启后按磁盘上的原始布局恢复。

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 共享接管句柄（G2/G3）

```rust
// crates/app/src/main.rs
/// 桌面接管句柄的共享单元：`IconGuard`（退出兜底）与 `Runtime`（运行期自愈）持有同一份。
pub(crate) type SharedHierarchy = Rc<Cell<DesktopHierarchy>>;

struct IconGuard { hierarchy: SharedHierarchy }
```

`DesktopHierarchy` 是 `Copy`，用 `Cell` 而不是 `RefCell`：读侧（`restore_icons`/`hide_icons`）
永远只需要取一份值，不存在跨借用嵌套，避免运行期自愈与退出守卫撞上 `BorrowMutError`。

### 2.2 失效判据（纯事实，不缓存状态位）

```rust
/// 接管是否已失效。命中任意一条即为失效：
/// 1. 接管时记录的任一窗口句柄已不存在（Explorer 重启的第一事实）；
/// 2. 非「原始桌面」模式下，本该隐藏的 `SysListView32` 变成可见（新 explorer 的默认状态）。
fn takeover_stale(rt: &Runtime) -> bool;
```

判据必须取「句柄是否还活着」而不是缓存的状态位：Explorer 重启没有可靠的送达通知
（`TaskbarCreated` 可能早于任务栏就绪，explorer 也可能被强制结束），死句柄是唯一
无可辩驳的事实。`desktop_mode == true` 时跳过第 2 条——原始桌面模式下图标层本就可见。

### 2.3 自愈出口

```rust
/// 重做桌面接管。返回是否真的发生了接管切换。
/// `probe()` 失败（explorer 还没起来）时保持现状，等下一次 4s 心跳重试。
fn ensure_desktop_takeover(rt: &mut Runtime) -> bool;
```

执行顺序（顺序敏感）：

1. `winbosk_shell::takeover::probe()` 重新探测；
2. `desktop_mode == false` 时 `hide_icons()`（新 explorer 的图标层默认可见）；
3. 替换共享句柄 `rt.hierarchy.set(h)`，并 `set_exit_listview(h.list_view)` 更新退出兜底；
4. `OverlayWindow::reown(h.overlay_parent())` 换 owner（旧 owner 已销毁）；
5. `OverlayWindow::add_tray_icon()` 幂等重登托盘；
6. `sync_front_band(rt)` 按「控制中心展开 / 栅栏前置」的单出口语义重推 Z 序；
7. **复核**：若仍判定失效（跨进程隐藏失败），记 `takeover_retry_at` 并在 30s 内不再重试——
   换 owner/重登托盘都是跨进程调用，不能变成 4s 心跳上的常驻动作。

**广播内不做跨进程调用**：`TaskbarCreated` 派发期间 explorer 正等待所有顶层窗口返回，
此刻 `probe`/`NIM_ADD`/`ShowWindow` 都可能与它互等。窗口过程只投递一条本进程消息
（`WM_APP_SHELL_RESTART`），回到自己的队列后再重登托盘并发出 `ExplorerRestarted`。

**`probe()` 必须带超时**：它内部的 `WM_SPAWN_WORKERW` 是跨进程同步消息，运行期由心跳
周期性调用，explorer 繁忙时无超时会直接钉死主线程（`SendMessageTimeoutW` +
`SMTO_ABORTIFHUNG`，500ms 上限；超时只放弃这次触发，照常继续探测）。

两条失败保护：`probe()` 失败、或原本管着图标层而新层级暂时没有 `SysListView32`
（explorer 还没把桌面建好）时，都**不**写入半接管状态。失败按退避阶梯重试
（10s→20s→40s→…封顶 300s，只用既有 4s 心跳，不新增周期源）；`TaskbarCreated` 是
「explorer 已就绪」的强信号，会清掉退避立即重试。托盘重登失败单独记 `tray_icon_lost`，
由心跳补登。

### 2.4 拓扑快照（G4）

```rust
/// 显示拓扑瞬变期间的「临时夹屏」快照。
struct TopologySnapshot {
    origin: (f32, f32),
    vw: f32,
    vh: f32,
    /// 夹屏前的栅栏位置，按 **fence id** 记录（期间可能增删栅栏，下标不稳定）。
    bounds: Vec<(u64, Rect)>,
}
```

- 只在「本次拓扑变化确实改动了栅栏位置」时建立；已有快照时不覆盖（保留最初的原始布局）。
- 新的拓扑与快照记录完全一致（origin + vw + vh）时：按 id 还原位置、丢弃快照；
  侧边栏不还原（由 `reanchor_fences` 重新停靠），随后仍调用一次 `reanchor_fences` 归一化。
- 用户拖动/缩放栅栏（`FenceDragEnd`）即视为「用户在当前拓扑下的显式布局」：只把**被拖的那一个**
  从快照里摘掉（其余栅栏继续待回滚），之后拓扑恢复不再回滚它。
- 夹屏本身**不落盘**，且这条不变量必须覆盖**所有**保存点：4s 心跳的 `reconcile_fences`、
  控制中心开关、折叠状态等任意一次保存都会带上 `desk.bounds`。因此 `desk.json` 的写入
  收敛到唯一入口 `persist(rt)`：快照待回滚时，用快照里的原始位置覆盖被夹屏栅栏再写盘。
  磁盘上的 `desk.json` 因此始终保持变化前的布局，进程重启后自然恢复。

### 2.5 渲染层新增接口

```rust
// crates/render/src/overlay.rs
impl OverlayWindow {
    /// Explorer 重启后换挂 owner（旧 owner 随旧 explorer 销毁）。同步 OverlayWindow.owner
    /// 与 WindowState.owner（`WM_MOUSELEAVE` 回落桌面带读后者）。
    pub fn reown(&mut self, new_owner: HWND);
    /// 重新登记托盘图标（幂等）。`TaskbarCreated` 与 App 自愈路径共用。
    pub fn add_tray_icon(&self);
}

pub enum OverlayEvent {
    /// Explorer 重启（收到 `TaskbarCreated` 广播）。App 据此重做桌面接管。
    ExplorerRestarted,
}
```

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

- **常驻后台冲突（4s 心跳）**：自愈检查放在既有 `SyncLibrary` 分支里，只增加 3~4 次
  `IsWindow`/`IsWindowVisible`（用户态即返回的廉价查询）。只有判定失效才会执行 `probe()`
  （一次 `SendMessageW(Progman, WM_SPAWN_WORKERW)`）与 `hide_icons()`（一次跨进程 `ShowWindow`），
  都是低频事件。不会改变 4s 心跳的既有语义（`redraw = changed || healed`）。
- **空闲性能归零**：`TaskbarCreated` 是事件驱动，不新增周期源；快照/回滚是一次性状态变更，
  不触发重绘以外的工作。约束 10 的 `QueryThreadCycleTime` 判据不变。
- **孤儿状态回收**：Explorer 重启后旧句柄全部失效；`probe()` 返回 `None` 时**不**写入半接管状态
  （不换 owner、不 hide），日志留一条 warn，下一次心跳自然重试。
- **旁路数据对齐**：`last_layout_h` 仍按下标与 `fences` 对齐（本计划不改数组结构）；
  `TopologySnapshot.bounds` 按 **fence id** 对齐，栅栏增删不会把位置套到错误的栅栏上。
- **状态全集校验**：`desktop_mode`（原始桌面）为真时，既不隐藏新图标层，也不把
  「图标层可见」判为失效——否则自愈会与原桌面模式互相打架（每 4s 互相翻转一次）。
- **语义精准定位**：`reown` 用新探测到的 `overlay_parent()`，不用下标/位置假设；
  快照还原按 id 查找，找不到即跳过（栅栏已删除场景）。
- **跨层约束**：新增接口只在 `render → app` 方向使用；`crates/core` 零改动。

## 4. 分层改动清单 (Implementation Steps)

### 4.1 `crates/render/src/overlay.rs`（驱动适配层）

- 新增静态 `TASKBAR_CREATED: AtomicU32`（`RegisterWindowMessageW("TaskbarCreated")`，0 = 注册失败）。
- 把 `create` 内的托盘登记提取为 `fn add_tray_icon(hwnd: HWND)`，`create` 与 `TaskbarCreated`
  分支、`OverlayWindow::add_tray_icon` 三处共用。
- `WndProc` 新增分支：`msg == TASKBAR_CREATED` → 重登托盘图标 + `emit_event(ExplorerRestarted)`；
  日志留痕。
- 新增 `OverlayWindow::reown`：更新 `self.owner` 与 `WindowState.owner`，并尝试
  `SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, new)`；失败只 warn（Z 序锚点仍用新句柄，
  不依赖所有权机制生效）。
- 新增 `OverlayEvent::ExplorerRestarted`。

### 4.2 `crates/app/src/main.rs`（组装层）

- `SharedHierarchy` + `IconGuard`/`Runtime.hierarchy` 改造；启动处构造 `Rc<Cell<_>>` 并克隆给守卫。
- 新增 `TopologySnapshot`、`takeover_stale`、`ensure_desktop_takeover`、
  `capture_fence_bounds`、`restore_fence_bounds`。
- `persist(rt)`：**唯一的布局持久化入口**，快照待回滚时用快照 bounds 覆盖夹屏结果；
  全部运行期 `rt.store.save(&rt.desk)` 调用点改为 `persist(rt)`。
- `OverlayEvent::ExplorerRestarted { tray_ok }` 分支：清退避，`ensure_desktop_takeover`
  成功后强制重绘一帧；失败时 `tray_icon_lost` 交由心跳补登。
- `SyncLibrary` 分支：心跳里先做失效检查，自愈成功则重绘；`tray_icon_lost` 时补登托盘。
- `DisplayChange` 分支重写：拓扑匹配快照 → 回滚；否则夹屏并建立快照；不再直接保存。
- `FenceDragEnd { fence }` 分支：只把被拖栅栏移出快照（空了才丢快照），再 `persist`。
- `toggle_desktop` 改用 `rt.hierarchy.get()`。
- `set_exit_listview` 改为 `None` 时写 0（自愈后新层级无 DefView 也不能留旧句柄）。
- 新增单元测试：拓扑匹配判据、按 id 还原（含缺失 id）、回滚幂等。

### 4.3 文档

- `AGENTS.md` 的「边界与禁区 / 常见任务配方」补两条口径：
  Explorer 重启自愈与拓扑瞬变不落盘；避免后续重构「顺手」把 `store.save` 加回 `DisplayChange`。

## 5. 防御性自查清单 (Defensive Invariants)

| 风险 | 防御设计 |
| :--- | :--- |
| Explorer 反复崩溃 → 每 4s 重探 | `probe` 失败 / 半成品层级不写状态；成功才切换。失败与「切换后仍失效」都走 30s 限频，`TaskbarCreated` 可提前解除 |
| 自愈误判（用户正常使用中） | 判据只认「句柄已死」与「本应隐藏却可见」两条事实；`desktop_mode` 为真时跳过后者 |
| 退出兜底恢复到旧句柄 | 共享 `Cell`，自愈与 `set_exit_listview` 在同一事务内更新 |
| 拓扑快照套错栅栏 | 按 fence id 对齐；缺失 id 跳过；删除/新增栅栏不影响其余项 |
| 快照长期滞留 | 拓扑回到快照配置即消费；用户拖动/缩放即清空；不会无限增长（至多一份） |
| 夹屏不落盘导致「真换小屏后重启又跑出去」 | 启动路径的 `resolve_overlaps → settle_move` 会把超出当前屏幕/相互重叠的栅栏推回并保存，重启后仍可见；运行期只是不把瞬态当永久 |
| 自愈路径引入 IME/前台锁 | `reown` 只做 `SetWindowLongPtr` + `sync_front_band`；不触碰 IME 关联与 `focus_for_input` |
| 托盘重复登记 | `NIM_ADD` 对同一 `(hWnd, uID)` 是替换语义，幂等 |

## 6. 验证与交付门禁 (Verification Gates)

### 自动化

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

- 新增单测：`topology_snapshot_matches_exact_geometry`、
  `restore_fence_bounds_aligns_by_id_and_is_idempotent`、`explorer_restart_clears_drag_state`、
  `persist_view_uses_snapshot_bounds_for_clamped_fences`、`snapshot_forgets_only_the_dragged_fence`、
  `takeover_retry_delay_backs_off_and_caps`。
- 既有 `keeps_drag_hint` / `carries_icon_drag_state` 测试覆盖新事件变体（默认落入清理侧）。

### 手工验证（桌面环境，已完成 1-2）

1. **托盘重建**：运行实例中 `taskkill /f /im explorer.exe` 后重新拉起 explorer，
   确认托盘图标回来（`Shell_NotifyIconGetRect` 返回 `S_OK`）、栅栏仍可点、真实桌面图标
   被重新隐藏（`SysListView32` 不可见）。
2. **接管自愈**：同上，观察日志出现一次「桌面接管已失效，重新接管」与一次
   「overlay 已换挂到新的桌面 owner」（`prev=0`），且 `desk.json` 的 mtime 不被自愈改动。
3. **退出兜底**：自愈后按 `Ctrl+Shift+F10` 退出，确认真实桌面图标恢复（恢复的是新句柄）。
4. **拓扑瞬变**（未做真机验证，需改分辨率；按 id 回滚与写盘替换由单测覆盖）：
   运行中把分辨率改为 1024×768 再改回，确认栅栏位置回滚、`desk.json` 的 `bounds` 未被
   改写；期间拖动任一栅栏后再次切换分辨率，确认该栅栏不再回滚（用户已显式布局）。

### 独立审查

- 实施完成后派发**无本次会话上下文**的子代理做对抗性走查：重点检查
  `IsWindow` 判据的误判面、自愈与 4s 心跳的交互、快照与栅栏增删的对齐、
  退出路径的句柄更新时序。