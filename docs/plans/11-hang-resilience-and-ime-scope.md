# 假死韧性（主线程看门狗 + IME 关联收窄）实施计划

> **本文件自包含**：不依赖任何会话上下文即可实施。写法对齐 `docs/plans/10.1-exec-brief.md`
> （可直接执行的最小切片）：§0 是实测 ground truth，§1-§6 是契约与门禁，附录 A 记录已落地的前置改动。
> 若与 `AGENTS.md` 冲突，以 `AGENTS.md` 为准。

## 0. 背景与现场结论（ground truth，2026-09-28 实测）

**事件**：release 实例（PID 27040，12:40:37 启动）在 15:24:16 本地点击栅栏内图标后假死；
进程存活、`Responding=False`、49 线程全部 Wait、3 小时累计 CPU 6.7 秒。

**主线程阻塞点**（只读采样，未改动目标进程）：

- `ntdll!ZwWaitForSingleObject+0x14`，参数 = 「无名 Event（handle 0x4C）+ 无超时 + 非 alertable」。
- 调用链（由内向外，函数名来自实时导出表解析）：
  `ntdll` 内部 → `MSCTF!CtfImeDispatchDefImeMessage` / `CtfImeToAsciiEx` / `CtfImeCreateThreadMgr`
  → `imm32!CtfImmDispatchDefImeMessage` / `ImmSetActiveContext` / `ImmLockIMC`
  → `user32!SendMessageW` / `CallWindowProcW` / `gapfnScSendMessage`
  → `ntdll!KiUserCallbackDispatcher` → `RTSSHooks64.dll!RTSSCBTProc`（RivaTuner 注入的 CBT 钩子）
  → `winbosk.exe`（消息循环）→ `ntdll!RtlUserThreadStart`。
- 已排除：临界区死锁（全栈无 `LockSemaphore==0x4C` 的 CS）、渲染层故障（栈上无 D2D/DComp/DWM/GPU 帧，
  且渲染失败时 4s 心跳日志会继续写）、"打开文件"路径（`launch_fence_icon` 的 `打开桌面图标` 未出现）。
- 结论：**卡死在 Windows 输入法/TSF（imm32 ↔ MSCTF）的同步等待里**，非本仓库代码死锁。
  触发面是「焦点/激活 + IME 上下文」链路（点击路径每次按下都 `AttachThreadInput`；`WM_IME_SETCONTEXT`
  转发 `DefWindowProc`）。
- 现场 dump：`%TEMP%\winbosk-hang-20260928-pid27040.dmp`（34.2 MB，`MDMP`，18 个流，
  `MINIDUMP_TYPE = 0x1965`）——**只在本机临时目录，未纳入仓库**。

**日志为何无痕**：单击路径此前零 info 埋点；阻塞发生在系统 DLL 的同步调用内部，应用的点击处理函数
从未返回；release 无控制台，`eprintln!` 也不可见。已由前置改动（附录 A）补上埋点。

**仍未解决的两项**（本计划的范围）：

- **d**：主线程停摆后**无法自证、无法取证**——没有任何机制在停摆时落日志/dump；心跳一停，日志就断流。
- **b**：普通点击仍可能经 `WM_IME_SETCONTEXT` 转发进入 `imm32`/`MSCTF`（IME 上下文默认与 overlay 关联）。

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

- **G1（d1，必做）停摆可自证**：主线程消息循环停摆超过阈值时，由**非主线程**写出一条 `error` 级日志
  （含停摆时长与最后心跳时刻），使"卡死"在日志里留下痕迹——这是本次事件最缺的东西。
- **G2（d2，可选、需用户拍板）停摆可取证**：同一次触发里落一份 minidump 到 `<data_dir>/dumps/`，
  用户可直接把文件交出来定位（等价于本次人工抓取的 dump）。
- **G3（b）普通交互不触发 IME 关联**：非编辑期让 overlay 与 IME 上下文**解除关联**，
  只有内联编辑会话期间才关联；会话结束立即断开。
- **G4 不欠新债**：新增机制不引入周期性唤醒（守住 `AGENTS.md` 约束 10），空闲期主线程唤醒次数不变。

### Non-Goals

- **不做主线程自愈**：Win32 同步调用无法安全中断；托盘/热键/菜单都在主线程，停摆时它们一并失效。
  本计划只做"能看见、能取证"，不做"自动恢复"（要恢复只能重启进程）。
- **不改 `with_foreground_lock` 在控制中心/Shell 菜单/模态框路径上的用法**：plan 05 §2 的实测约束
  （`TrackPopupMenu` 需要 owner 已在前台）与本次问题正交。
- **不改渲染/合成/布局/命中模型口径**，`crates/core` 零改动。
- **不引入 tokio/异步运行时/新重型依赖**（`AGENTS.md` 禁区）；d2 只允许新增 `dbghelp` 绑定。
- **不做 IME 视觉/候选窗/输入法选择逻辑**；不改内联编辑的文本语义。
- **不新增周期性定时器**：看门狗由主线程既有 4s 心跳"续期"，不自己轮询。

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 d：看门狗

```rust
// crates/app/src/watchdog.rs（新模块）

/// 主线程心跳（单调毫秒）。**只由主线程写**（SyncLibrary 分支）、看门狗只读。
/// 0 = 尚未开始（启动早期），此时按 spawn 时刻起算。
static MAIN_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

/// 停摆阈值：心跳 4s ⇒ 30s = 连续缺 7 个心跳（正常负载下不可能）。
const STALL_MS: u64 = 30_000;

/// 启动看门狗线程并武装一次性可等待定时器。返回后无需调用方持有任何句柄。
pub(crate) fn spawn(data_dir: &std::path::Path);

/// 主线程每次心跳调用：刷新时间戳 + 重置定时器（相对时间 = 单次，不周期重排）。
pub(crate) fn heartbeat();
```

**工作机制（零周期唤醒的关键）**：

1. `spawn` 创建 `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` 或普通可等待定时器（复用 `crates/render`
   动画时钟同款 API），**相对时间** `-STALL_MS`（负数 = 相对，单次触发，不自动重排），
   然后 `std::thread::spawn` 一个线程 `WaitForSingleObject(timer, INFINITE)`。
2. 主线程每次 4s 心跳调用 `heartbeat()`：`MAIN_HEARTBEAT_MS.store(now_mono_ms())` +
   `SetWaitableTimer(timer, -STALL_MS)`。**这不是新增唤醒源**——它挂在既有心跳上，只是多两次廉价调用。
3. 主线程正常 ⇒ 定时器每次都在到期前被续期，永不触发，看门狗线程始终 blocked（0 CPU）。
   主线程停摆 ⇒ 30s 后定时器自然到期 ⇒ 看门狗线程醒来：
   - 读 `MAIN_HEARTBEAT_MS`，`now - last >= STALL_MS` 时：
     a. **先写日志**（`tracing::error!`，含停摆毫秒数与最后心跳毫秒数）——tracing 的 appender 是
        独立线程，主线程停摆不影响落盘；
     b. 若启用 d2：`MiniDumpWriteDump` 写 `<data_dir>/dumps/hang-<unix秒>.dmp`
        （`MINIDUMP_TYPE = 0x1965`：DataSegs|HandleData|UnloadedModules|IndirectlyReferencedMemory|
        ProcessThreadData|FullMemoryInfo|ThreadInfo，实测 34 MB 量级）；
     c. `dumps/` 只保留最近 3 份（超出删最旧），避免反复卡死撑爆磁盘；
     d. 线程退出（**一次性**语义：本次事件的取证已经拿到，不做反复 dump）。
4. 单调时钟口径：以 `std::time::Instant` 的进程内基准换算毫秒（`static BASE: OnceLock<Instant>`），
   不使用系统墙钟（可被 NTP/手动改时间跳变误判）。

**已知风险（必须写进代码注释）**：若主线程卡死时持有进程堆锁，看门狗线程的任何分配（含 tracing
格式化、dbghelp 调用）也可能阻塞——因此**顺序固定为「先日志、后 dump」**，并在 d2 里
`spawn` 阶段就 `LoadLibrary(dbghelp.dll)` 预热，避免取证时才做首次加载。

### 2.2 b：IME 关联收窄

```rust
// crates/render/src/overlay.rs，OverlayWindow
impl OverlayWindow {
    /// 编辑会话开始：把 IME 上下文关联到 overlay（**必须在 SetFocus 之前**）。
    pub fn ime_attach(&self);
    /// 编辑会话结束：解除关联，系统此后不再向本窗口派发 WM_IME_*。
    pub fn ime_detach(&self);
}
```

- 实现：`ImmAssociateContext(hwnd, ctx)`——`ime_attach` 用 `ImmCreateContext()` 建一个上下文并关联
  （或保存创建时的默认上下文并回填）；`ime_detach` 关联 `NULL` 并保存/释放句柄。
  **不要**在 `ime_detach` 里 `ImmDestroyContext` 掉正在使用的上下文——编辑期可能正在组合。
- 状态机（与 `rt.edit` 严格成对，**唯一出入点**）：

  | 事件 | 关联动作 | 说明 |
  | :--- | :--- | :--- |
  | 进入内联编辑（`focus_overlay` 调用点：`editing.rs` 的 `open_rule_input` / 重命名入口） | `ime_attach()` → 再 `focus_for_input()` | 顺序敏感：先关联再给焦点，否则 IME 按旧状态激活 |
  | 提交 / 取消 / 点击别处提交（`dismiss_edit` 全部出口） | `focus` 交还后 `ime_detach()` | 覆盖 `edit_committed` / `Esc` / 失焦三条路径 |
  | 窗口失焦（`OverlayFocusLost`） | `ime_detach()` | 防御性幂等 |
  | 程序退出（`run` 返回前） | `ime_detach()` | 与 `IconGuard` 同层的收尾 |
  | 启动时 | `ime_detach()`（创建窗口后立即调用） | 常态即"未关联"，普通点击不再进 MSCTF |

- 幂等：`ime_attach`/`ime_detach` 各自记录当前状态（`Option<HIMC>`），重复调用为空操作。
- 验收判据（可 `grep` 日志）：普通点击/拖动/右键全程日志**不出现** `IME：上下文设置转发`；
  进入重命名后出现；退出重命名后停止出现。

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

- **常驻后台与同步引擎**：看门狗不新增周期源——它的定时器由**既有 4s `SyncLibrary` 心跳**续期，
  触发后不重排。`SyncLibrary` 的库同步/空闲修剪逻辑零改动，只是分支末尾多两行。
- **空闲性能归零律**：看门狗线程全程 blocked 在 `WaitForSingleObject(INFINITE)`；主线程新增的
  `store` + `SetWaitableTimer` 各为纳秒级调用，不改变 `QueryThreadCycleTime` 判据（非心跳桶仍为 0）。
- **孤儿状态回收律**：`dumps/` 目录必须有保留上限（3 份）与命名含时间戳，否则反复卡死会无限增长；
  删除失败仅忽略（下次触发再试）。
- **语义精准定位律**：停摆判定唯一依据是主线程心跳时间戳（单一事实源）；不得用 `IsHungAppWindow`
  或窗口句柄状态等间接量（`AGENTS.md` 记录过"间接量失真"的教训：进程级 CPU 口径含 GPU 线程）。
- **状态全集校验律**：IME 关联状态必须与 `rt.edit.is_some()` 复合校验——只判"是否调用过 attach"
  会因为异常路径漏 detach 而永久退化为"IME 常驻关联"（即 b 的目标失效）。断言：
  `grep -rn "ime_attach\|ime_detach" crates` 只应命中 overlay 定义与 app 的成对调用点。
- **旁路数据对齐律**：`MAIN_HEARTBEAT_MS` 单向（主线程写、看门狗读）；无第二写入者，
  不存在跨帧延迟更新问题。
- **窗口销毁/重启**：看门狗线程随进程退出，不需要显式回收；定时器句柄在 `spawn` 里泄漏是有意的
  （进程生命周期内一直需要），但**不得**在 `heartbeat()` 里重复创建句柄。

## 4. 分层改动清单 (Implementation Steps)

自底向上，共 2 层：

1. **render 层**（`crates/render/src/overlay.rs`）
   - 新增 `OverlayWindow::ime_attach()` / `ime_detach()`（`ImmCreateContext` / `ImmAssociateContext`，
     imports 增补对应符号），内部持有 `Option<HIMC>` 保证幂等。
   - `create()` 末尾调用一次 `ime_detach()`（常态未关联）。
2. **app 层**（`crates/app/src/`）
   - 新模块 `watchdog.rs`（§2.1 全部），并在 `main.rs` 的 `mod` 列表登记。
   - `main.rs`：`run()` 里 `data_dir` 就绪后 `watchdog::spawn(&data_dir.join("dumps"))`；
     `SyncLibrary` 分支末尾 `watchdog::heartbeat()`。
   - `main.rs` 的 `focus_overlay()`：改为 `ime_attach()` → `focus_for_input()`；
     `dismiss_edit()` 全部出口 + `OverlayEvent::OverlayFocusLost` 处理 + `run()` 收尾加 `ime_detach()`。
   - `editing.rs`：确认所有编辑入口/出口都经过上面两个函数（现有 `focus_overlay` 调用点 3 处，
     `dismiss_edit` 出口见 §2.2 表格）。
3. **core 层**：无改动（明确声明）。

## 5. 防御性自查清单 (Defensive Invariants)

| 律 | 针对性防御 |
| :--- | :--- |
| 1 常驻后台冲突 | 看门狗定时器由既有 4s 心跳续期，触发后不重排；`SyncLibrary` / 空闲修剪 / 库同步逻辑零改动 |
| 2 空闲归零 | 看门狗线程 blocked；主线程新增仅两次纳秒级调用；不新增周期唤醒（约束 10 判据不变） |
| 3 孤儿回收 | `dumps/` 保留最近 3 份；删除失败忽略；线程随进程退出 |
| 4 语义精准 | 停摆判定只用心跳时间戳（单调时钟）；不用窗口/进程间接状态 |
| 5 状态全集 | IME 关联与 `rt.edit` 成对，覆盖提交/取消/失焦/退出四类出口 + 幂等保护 |
| 6 旁路对齐 | `MAIN_HEARTBEAT_MS` 单写多读；无第二写入者、无跨帧延迟 |

## 6. 验证与交付门禁 (Verification Gates)

自动化门禁（全绿为交付前提，**不并行跑 cargo**）：

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

静态断言：

- `grep -rn "atomic\|SetWaitableTimer" crates/app/src/watchdog.rs` → 定时器只在 `spawn` 创建一次。
- `grep -rn "ime_attach\|ime_detach" crates` → 仅 overlay 定义 + app 的成对调用点（§2.2 表格）。
- `grep -rn "MAIN_HEARTBEAT_MS" crates` → 写入点唯一（`watchdog::heartbeat`）。

手动验证矩阵（浏览器最大化作遮挡参照；先退出旧实例再跑 `$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe`）：

| # | 操作 | 期望 |
| :--- | :--- | :--- |
| 1 | 普通点击栅栏/图标（非编辑） | 日志出现 `左键按下` + `按下：命中…`，**不出现** `IME：上下文设置转发` |
| 2 | 双击栅栏标题进入重命名，输入中文，提交 | 进入后出现 IME 日志；中文正常上屏；提交后日志停止出现 IME 行 |
| 3 | 重命名中途按 Esc / 点空白提交 | 同上，IME 关联解除（再点击无 IME 行） |
| 4 | 人为制造停摆（调试期临时把 `STALL_MS` 调到 3s 并在主线程插 `Sleep`） | 看门狗写出 `error` 日志；d2 启用时 `data_dir/dumps/` 出现 dump |
| 5 | 连续制造 4 次停摆 | `dumps/` 只保留 3 份 |
| 6 | 空闲 5 分钟 | 日志只有 4s 心跳派生行为（60s 一条修剪）与看门狗续期，无新增周期行；任务管理器 CPU 与改动前一致 |

对抗性审查要点（**独立干净上下文子代理**，实施完成后必须执行）：

- 看门狗线程在"主线程持有堆锁"的极端情形下是否会二次阻塞（顺序固定：先日志后 dump 是否足够）。
- `dumps/` 保留策略的边界（同名时间戳、删除失败、目录不可写）。
- IME 关联的**全部**异常出口是否覆盖（含 `RefCell` 借用 panic 路径、窗口销毁、热键退出、`WINBOSK_AUTOSTOP_MS` 自退）。
- b 是否存在把"中文输入不可用"引入正常路径的可能（复合状态校验是否足够）。
- 是否引入任何新的周期唤醒（对照约束 10 的判据，可实测）。

## 7. 关联问题：退出路径滞留（另一份计划）

验证 a/c 埋点时另发现**偶发**问题：日志写完 `已退出` 后进程仍存活（3 次冒烟复现 1 次），
持有单实例互斥 ⇒ 用户"双击重启毫无反应"。它与本计划的 d 天然相接：**主线程停摆判定按心跳时间戳计，
退出收尾期同样适用**（收尾一开始就不再有心跳）。

- 详细计划见 [`docs/plans/12-exit-teardown-stall.md`](12-exit-teardown-stall.md)（分层埋点 + 异步恢复图标 + 15s/45s 限时兜底）。
- 实现 d 时的两条接口约定：
  1. 看门狗在**收尾阶段仍保持武装**，并把 `current_phase()`（plan 12 §2.1 提供）作为停摆日志字段；
  2. **`STALL_MS` 不得低于 15s**——plan 12 的 T1 在 15s 落 warn、T2 在 45s 强制结束，
     dump 窗口是 15~45s；调到 15s 以下会让 dump 落不下来。

## 附录 A：已落地的前置改动（2026-09-28，本次事件当场实施）

这两项已进工作区（未提交），新会话实施本计划时**不要重复做**，并以其日志为验收基准：

- **a：点击唤起改为惰性提权**（`crates/render/src/overlay.rs`）
  - 新增 `is_topmost_window` / `normal_window_above` / `raise_hwnd_to_normal_top_on_press`；
    `WM_LBUTTONDOWN` 改走惰性版：上面没有普通带窗口 → 直接返回；否则**裸调** `HWND_TOP`，
    仅当裸调无效（其它环境的静默拒绝）才退回 `with_foreground_lock`（`AttachThreadInput`）。
  - 右键路径与控制中心 `raise_console` **保留强制版**（plan 05 的实测约束）。
  - 依据：plan 05 §8.5.3 探针实证本机（Win11 26200）裸调未被静默拒绝，前台锁只是跨环境纵深防御。
- **c：埋点**（`crates/render/src/overlay.rs` + `crates/app/src/main.rs`）
  - 按下入口 `左键按下 x/y`；`on_button_down` 每个分支一条 `按下：…`（控制中心/折叠按钮/缩放把手/
    标题拖动/框选/图标/双击图标/未命中/内联编辑区）；
  - `WM_IME_*` 四类各留痕（`IME：组合开始/结束`、取串前后 + 耗时、`IME：上下文设置转发 DefWindowProc`
    + 慢返回 warn）；app 层 `单击图标（选中）`。
  - 日志量：每次点击约 2 行；可用 `RUST_LOG=info,winbosk_render=warn` 压制渲染层。

## 附录 B：卡死现场取证方法（只读，可复用）

1. **定位实例**：`Get-Process winbosk | Select Id,StartTime,Responding,Path`（`Responding=False` = 主线程不再收消息）。
2. **抓 dump**：`dbghelp!MiniDumpWriteDump`（`TYPE=0x1965`）写 `<exe>/../*.dmp`；命令行替代品 =
   任务管理器「创建转储文件」。
3. **只读采样（无 dump 也能定位到函数级）**：
   - 线程枚举 `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)`；
   - 逐线程 `SuspendThread` → `GetThreadContext`（`CONTEXT_CONTROL|CONTEXT_INTEGER`；x64 的 `Rip` 在
     `0xF8`、`Rsp` 在 `0x98`）→ 立刻 `ResumeThread`；
   - 栈扫描：`VirtualQueryEx` 逐个已提交区域 `ReadProcessMemory`（跨保护页区域要分段，整段读会失败），
     扫描 8 字节对齐值里落在模块地址区间的"类返回地址"；
   - **把地址解析成函数名**：读目标进程内存里的 PE 导出表（导出目录 → `AddressOfNames` /
     `AddressOfNameOrdinals` / `AddressOfFunctions`），取"最近的更低 RVA 导出"；
     ntdll 的 `Zw*` 存根可用代码字节 `b8 <imm32>` 反读 syscall 号（本次 `Rax=4` = `NtWaitForSingleObject`）；
   - 句柄定性：`DuplicateHandle` 到本进程 + `NtQueryObject`（`ObjectTypeInformation`）→
     "Event / Mutant / Thread"。
   - 参考实现：本会话用的 Python 采样器（`%TEMP%\sylva-*.py`，易失；需要时按上面 API 清单重写即可）。
4. **判据速查**：`ZwWaitForSingleObject` + 无名 Event + 无超时 = 等一个不会来的事件；
   `ZwWaitForAlertByThreadId`/`RtlWaitOnAddress` = Rust `Mutex`/`park` 一类；`win32u!NtUserMessageCall` = 窗口消息类同步调用。
