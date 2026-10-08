# 退出路径滞留（`已退出` 后进程不散）取证与限时收尾 实施计划

> **本文件自包含**：不依赖任何会话上下文即可实施。写法对齐 `docs/plans/10.1-exec-brief.md`
> 与 `docs/plans/11-hang-resilience-and-ime-scope.md`：§0 是实测 ground truth，§1-§6 是契约与门禁。
> 若与 `AGENTS.md` 冲突，以 `AGENTS.md` 为准。

## 0. 背景与现场结论（ground truth，2026-09-28 实测）

**事件**：验证点击埋点（plan 11 附录 A 的 a/c）时做冒烟运行，**3 次复现 1 次**：日志写到最后一行
`已退出`（`crates/app/src/main.rs:717`）后，进程**仍存活**且不再产任何日志，只能手动结束。

**观测到的形态**：

- 无顶层窗口、`Process.Responding=True`——**这两条都不是判据**：overlay 是 WorkerW 的**子**窗口，
  本就不出现在 `EnumWindows` 顶层列表里；而 `Responding` 在进程"无主窗口"时恒为真。
  该结论已写入本计划，禁止后续再拿它们当"窗口已销毁"的证据。
  （⚠ 2026-09-28 实施时实测纠正：「子窗口」判断有误，overlay 实为顶层窗口，见 §7.3。）
- 累计 CPU 停在 2.11s 不再增长（等锁/等消息特征，非忙等）。
- 同一二进制其余两次以相同命令启动则**干净退出**（进程消失、桌面图标恢复）→ 偶发，非确定性死锁。

**危害（用户可见）**：滞留进程持有单实例互斥（`CreateMutexW`，`main.rs:346`）。此后双击图标重启：
走 `ERROR_ALREADY_EXISTS` 分支 `eprintln!` 后返回（`main.rs:353-356`），release 无控制台 →
**用户看到"双击毫无反应"**，且桌面可能停在"真实图标未恢复"状态。

**为什么现在查不出来**：`已退出` 是收尾开始前的最后一条日志，其后是**纯 RAII drop 链**、全程零埋点，
日志已无法判别卡在哪一步。

**收尾时序（代码事实；`已退出` 之后，drop 顺序 = 声明逆序）**：

| # | 对象（声明位置） | 释放动作 | 潜在阻塞面 |
| :---: | :--- | :--- | :--- |
| 1 | `runtime`（`Rc<RefCell<Runtime>>`，`main.rs:673`） | 仅减一档引用（overlay 的事件处理器闭包仍持一档） | — |
| 2 | `overlay`（`main.rs:454`） | `Drop for OverlayWindow`（`overlay.rs:1053`）：`KillTimer` → `set_anim_active(false)` → `DestroyWindow(hwnd)` → `DestroyWindow(proxy)` → `Shell_NotifyIconW(NIM_DELETE)` → 释放 `WindowState` | `DestroyWindow`（IME/TSF 关联拆链）、`NIM_DELETE`（跨进程发往托盘窗口）、**释放 `WindowState` 时闭包析构 → Rc 归零 → `Runtime`→`Compositor` 析构 → D3D11/D2D/DComp/GPU 驱动 Release 链** |
| 3 | `_guard`（`IconGuard`，`main.rs:450`） | `restore_icons()`（`takeover.rs:58`）= 跨进程同步 `ShowWindow(SysListView32, SW_SHOW)` | 同步等待 explorer 线程处理（经典跨进程阻塞面） |
| 4 | 返回 `main()` 后的局部 | 日志 `WorkerGuard`（`non_blocking` 守卫 → join 日志线程）；`_ole`/`_com` **实为裸 HRESULT，无 Drop** → **本工程从不调用 `OleUninitialize`/`CoUninitialize`** | 日志线程 join；真正的 OLE/COM STA 拆解发生在 `main` 返回后的 CRT/DLL 分离阶段（`combase`/`ole32` 的 `DLL_PROCESS_DETACH`），以及第三方注入 DLL（本机存在 `RTSSHooks64.dll`） |

即四个层级——**本进程窗口销毁 / GPU 资源释放 / 跨进程恢复 / 进程退出尾部**——全都无埋点，也都各有真实阻塞面。

**与 plan 11 的关系**：plan 11 的 d（主线程看门狗，30s 阈值）若在退出阶段仍保持武装，本问题会自动被它记一笔；
本计划补的是「分层埋点 + 非阻塞恢复 + 限时兜底」，让滞留**一次定位**，不必每次靠抓 dump 手工解栈。

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

- **G1 收尾全程可判别**：把收尾拆成具名阶段，每阶段落 `进入 X` / `离开 X（耗时 ms）`；滞留时"最后一条进入"即定位点。
- **G2 恢复图标不再阻塞退出**：`IconGuard::drop` 改走**异步**恢复（`ShowWindowAsync`，投递即返回），
  把跨进程等待从退出路径上摘掉；启动期 `desktop_mode` 路径（`main.rs:569`）保持同步不变。
- **G3 滞留有硬上限**：收尾开始即武装一次性兜底线程——T1（15s）落 `warn`（点名当前阶段）并**兜底投递**一次图标恢复；
  T2（45s）`TerminateProcess(self)`。**"僵尸占互斥 → 双击无反应"由此根除。**
- **G4 单实例拒绝可见**：单实例检查挪到日志初始化**之后**，拒绝启动时写 `warn` 日志（附"是否检测到 overlay 窗口"的**best-effort 提示**），
  release 下"双击无反应"至少留下文件级线索。
- **G5 零新周期唤醒**：兜底线程一次性（`sleep` → 动作 → `sleep` → 终止），不新增任何周期源（`AGENTS.md` 约束 10 判据不变）。

### Non-Goals

- **不做"卡死自愈"**：不重试/回滚已开始的资源释放；只做"限时 + 有痕 + 恢复图标兜底"。
- **不改 `IconGuard` 的守卫语义**（`AGENTS.md` 约束 3）：异步版仍保证"任何退出路径都发出恢复指令"，
  且指令投递后即使本进程立即结束，explorer 仍会在自己的队列里处理它。
- **不改启动期同步 `restore_icons()`**（`desktop_mode` 分支需要立即生效）。
- **不引入 job object / 外部监视进程 / 新运行时依赖**；不动渲染布局/命中的业务逻辑；`crates/core` 零改动。
- **第二实例不主动杀僵尸**（跨进程 `TerminateProcess` 属破坏性动作，需另行授权，见 §6 待定项）。

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 阶段埋点（`crates/app/src/main.rs`，新增约 20 行小工具）

```rust
/// 收尾阶段计时：`begin` 落"进入"，Drop 落"离开 + 耗时 ms"。作用域结束即落"离开"。
struct ExitPhase(&'static str, std::time::Instant);

/// 当前阶段名：唯一写入者是 `ExitPhase`；供兜底线程（及 plan 11 的看门狗）读到"卡在哪一步"。
static EXIT_PHASE: std::sync::Mutex<&'static str> = std::sync::Mutex::new("");
fn note_phase(name: &'static str);              // 中毒 recovering，不 panic
pub(crate) fn current_phase() -> &'static str;
```

阶段名（固定字面量，日志可 grep）：`释放运行时句柄` / `销毁 overlay 窗口（含合成器/GPU 资源）` / `恢复真实桌面图标`。

`run()` 末尾改为**显式 drop + 分阶段日志**，顺序与今日隐式顺序**逐字一致**（不得调换）：

```rust
tracing::info!("已退出（进入收尾）");
arm_exit_deadline(_guard.hierarchy.list_view);          // §2.4，必须先于任何阶段
{ let _p = ExitPhase::begin("释放运行时句柄"); drop(runtime); }
{ let _p = ExitPhase::begin("销毁 overlay 窗口（含合成器/GPU 资源）"); drop(overlay); }
{ let _p = ExitPhase::begin("恢复真实桌面图标"); drop(_guard); }
tracing::info!("收尾完成");
```

- 用**块作用域 + `let _p`**，而非语句尾 `drop(_p)`——阶段对象在块尾析构即落"离开"，无需额外一行。
- `runtime` 显式 drop 只是减一档 Rc（闭包仍持一档），**真正的 `Runtime`/`Compositor` 析构发生在 overlay 的 Drop 内部**
  （`overlay.rs:1078` 的 `Box::from_raw(self.state)`）——故阶段 2 的名字包含合成器/GPU，实现时不要改小它。

### 2.2 overlay 内部细分（`crates/render/src/overlay.rs`，`Drop for OverlayWindow`）

render 层不能依赖 app，**不共用** `ExitPhase`；Drop 内用局部 `Instant` 各测一段，落**同格式** info：

```
退出：停表与句柄清理（ms=…）
退出：DestroyWindow overlay（ms=…）
退出：DestroyWindow proxy（ms=…）
退出：托盘图标移除 NIM_DELETE（ms=…）
退出：释放窗口状态（事件处理器→Runtime→合成器→GPU Release）（ms=…）
```

只加日志，**不改任何释放顺序与 API 调用**。

### 2.3 异步恢复（`crates/shell/src/takeover.rs`）

```rust
impl DesktopHierarchy {
    /// 保持现状：同步 ShowWindow（启动期 `desktop_mode` 需要立即生效）。
    pub fn restore_icons(&self);
    /// 新增：ShowWindowAsync——投递给目标线程后立即返回，绝不在退出路径上等待。
    /// 语义差异：拿不到"先前可见状态"返回值（本工程不使用该返回值）；
    /// explorer 卡死时图标同样恢复不了，但**不会连带把本进程钉死在 exit 上**（同步版会）。
    pub fn restore_icons_async(&self);
}
```

- 复合状态校验与同步版同口径：`Some(lv) && !lv.is_invalid()`（禁止只判 `is_some()`）。
- `IconGuard::drop` 改调异步版；`main.rs:569` 的启动分支保持同步版。

### 2.4 限时兜底（`crates/app/src/main.rs`）

```rust
/// 收尾兜底：一次性线程，只在收尾期武装；不重复、不周期。
const EXIT_WARN_MS: u64 = 15_000;   // T1：warn（点名阶段）+ 兜底投递图标恢复
const EXIT_FORCE_MS: u64 = 45_000;  // T2：强制结束进程

/// 参数：恢复图标所需的原始句柄（`Option<HWND>`，Copy，可跨线程传递）。
fn arm_exit_deadline(list_view: Option<HWND>);
```

行为：

1. `sleep(EXIT_WARN_MS)` → `tracing::warn!(phase = current_phase(), "退出收尾超时：疑似卡在 {phase}…")`；
   随后**兜底投递** `ShowWindowAsync(list_view, SW_SHOW)`（幂等：主线程若已恢复过，多投一次无副作用）。
2. 再 `sleep(EXIT_FORCE_MS - EXIT_WARN_MS)` → `TerminateProcess(GetCurrentProcess(), 0)`。
3. **必须用 `TerminateProcess`，不许用 `ExitProcess`**：`ExitProcess` 需要拿**加载器锁**去跑各 DLL 的
   `DLL_PROCESS_DETACH`；若卡点恰是某个注入 DLL（本机有 `RTSSHooks64.dll`）的 detach，`ExitProcess` 会**同样死锁**。
   `TerminateProcess` 是内核级终止，不需要加载器锁。
4. 武装点：`run_message_loop()` 返回后立刻武装——此时进程已决定退出，兜底不可能误伤正常运行期。
5. 与 plan 11 d 的次序：T1(15s) → 看门狗 dump(30s) → T2(45s)，**取证在前、清场在后**；本计划不依赖 d。

### 2.5 单实例拒绝留痕（`crates/app/src/main.rs`）

- 把 `data_dir` 计算与 `logging::init` **前置**到单实例检查之前（纯路径计算 + 建目录，无业务副作用）；
  日志初始化失败分支的 `eprintln!` + 提前返回行为不变。
- 拒绝分支：

```rust
if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
    // best-effort 提示：仅做窗口检索（见下），不作判据
    let overlay_alive = winbosk_shell::takeover::overlay_window_alive();
    tracing::warn!(overlay_alive, "WinBosk 已在运行，本次启动退出（单实例）");
    if !overlay_alive {
        tracing::warn!("未检测到 WinBosk overlay 窗口：可能是上次退出遗留的进程，请在任务管理器结束 winbosk.exe 后重试");
    }
    return;
}
```

- `overlay_window_alive()`（新增于 `takeover.rs`）：**只做窗口检索，禁止发任何消息**——
  `FindWindowW("Progman")` → `FindWindowExW(progman, "SHELLDLL_DefView")` → 其父（WorkerW）→
  `FindWindowExW(worker, "WinBoskOverlay")`。**不得复用 `probe()`**：它会给 Progman 发 `WM_SPAWN_WORKERW`，
  在被拒的第二实例里既多余、又可能在 explorer 繁忙时把本进程拖住。
  （overlay 是 WorkerW 的子窗口，`FindWindowW`/顶层枚举**必然找不到**——这正是本 helper 存在的理由。）
  （⚠ 2026-09-28 实施时实测纠正：上句及本条下钻链均被证伪——overlay 是 `WS_POPUP` 顶层窗口
  （owner = WorkerW），实际实现改为顶层 `FindWindowW`，见 §7.3；「禁止发消息 / 禁用 `probe()`」
  两条约束不变。）
- 该布尔只是**提示**（僵尸若卡在 `DestroyWindow` 之前，overlay 仍在，会误报 alive）；定案证据是阶段埋点。
- 第二实例多开一次日志句柄（毫秒级后退出），两进程追加同一日志文件在 OS 层安全。

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

- **常驻后台与同步引擎**：兜底线程一次性（`sleep`→动作→`sleep`→终止），无重排、无轮询；
  进程正常退出时它随进程消失。`EXIT_PHASE` 是收尾期专用写点，与业务状态零交集，不触碰 4s 心跳/`SyncLibrary`。
- **空闲性能归零**：新增代码全部位于"消息循环已返回"之后，运行期不执行任何一次；T1/T2 是两次 `sleep`，不占 CPU。
- **孤儿状态回收**：不新增文件类型（详见 `AGENTS.md`/plan 11 的 dumps 保留策略，本计划不涉及）。
- **语义精准定位**：卡点判定唯一依据 = `EXIT_PHASE`（具名阶段）+ `Instant` 时间差；
  **显式禁用** `Responding`、顶层窗口枚举等在本场景已被证伪的间接量（§0）。
- **状态全集校验**：`restore_icons_async` 必须 `Some(lv) && !lv.is_invalid()`；`EXIT_PHASE` 加锁读写在中毒时 recovering。
- **旁路数据对齐**：`EXIT_PHASE` 单写多读（写者只有 `ExitPhase::begin`），无跨帧延迟。
- **与看门狗共存**：T2 的强制终止会跳过 d 的 dump——次序设计（15→30→45）保证 d 先落 dump。
  实现 d 时**不得**把 `STALL_MS` 调到 15s 以下。

## 4. 分层改动清单 (Implementation Steps)

1. **shell 层**（`crates/shell/src/takeover.rs`）：新增 `restore_icons_async()`（import 增补 `ShowWindowAsync`）
   与 `overlay_window_alive()`（复用文件内既有 `find_class_child` / `wide`，零新依赖）。
2. **render 层**（`crates/render/src/overlay.rs`）：`Drop for OverlayWindow` 内 5 段计时 info；不改释放逻辑。
3. **app 层**（`crates/app/src/main.rs`）：
   - `ExitPhase` + `EXIT_PHASE` + `current_phase()` + `arm_exit_deadline()`（约 60 行）；
   - `run()` 末尾改成"显式 drop + 分阶段日志"，顺序不变，并在进入收尾时武装兜底；
   - `IconGuard::drop` 改调 `restore_icons_async()`；
   - `main()`：`data_dir` 计算 + `logging::init` 前置到单实例检查之前；拒绝分支写 `warn`。
4. **core 层**：无改动（明确声明）。

## 5. 防御性自查清单 (Defensive Invariants)

| 律 | 针对性防御 |
| :--- | :--- |
| 1 常驻后台冲突 | 兜底线程一次性；`EXIT_PHASE` 与业务状态无交集；不动 4s 心跳/库同步/空闲修剪 |
| 2 空闲归零 | 新代码全在消息循环返回之后执行；运行期零开销、零新增唤醒 |
| 3 孤儿回收 | 无新文件类型；`ShowWindowAsync` 兜底投递幂等，重复投递无副作用 |
| 4 语义精准 | 卡点判定 = 具名阶段 + 单调时钟；显式不用 `Responding`/窗口枚举（§0 已证伪） |
| 5 状态全集 | 异步恢复同口径校验 `Some(lv) && !lv.is_invalid()`；`EXIT_PHASE` 中毒 recovering |
| 6 旁路对齐 | `EXIT_PHASE` 单写者；显式 drop 顺序 = 原隐式顺序（diff 复核，见 §6 审查要点） |

## 6. 验证与交付门禁 (Verification Gates)

自动化门禁（全绿为交付前提；**不并行跑 cargo**；先退出运行中的实例）：

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

静态断言：

- `grep -n "ExitPhase::begin" crates/app/src/main.rs` → 恰好 3 处，且顺序为 运行时 → overlay → 图标守卫。
- `grep -n "ShowWindowAsync\|ShowWindow(" crates/shell/src/takeover.rs` → 同步版只在 `restore_icons`，
  `restore_icons_async` 用 `ShowWindowAsync`。
- `grep -n "TerminateProcess\|ExitProcess" crates/app/src/main.rs` → 只允许 `TerminateProcess`（兜底）。
- `grep -n "probe()" crates/shell/src/takeover.rs crates/app/src/main.rs` → `overlay_window_alive` 内不得调用 `probe()`。
- `grep -n "CreateMutexW" -A 14 crates/app/src/main.rs` → 单实例检查位于 `logging::init` 之后。

手动验证矩阵（`$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe`）：

| # | 操作 | 期望 |
| :---: | :--- | :--- |
| 1 | 连跑 20 次冒烟 | 每次日志含完整"进入/离开"3 段（app）+ 5 段（render）；`Get-Process winbosk` 无输出（进程必散） |
| 2 | 人为制造收尾阻塞（临时在某阶段插 `Sleep(60000)`） | T1 落 `退出收尾超时：疑似卡在 <阶段名>`；T2 在 45s 内结束进程；图标由兜底投递恢复 |
| 3 | 若自然复现滞留 | **先按 plan 11 附录 B 只读采样主线程栈再结束进程**，栈顶函数与 §0 表格逐层对照定案 |
| 4 | 运行中再启动一个实例（release） | `data/logs/winbosk.log.<date>` 出现 `WinBosk 已在运行…`（带 `overlay_alive`） |
| 5 | 空闲 5 分钟 | 唤醒口径与改动前一致（`AGENTS.md` 约束 10 判据不变） |
| 6 | 正常退出 | `SysListView32` 恢复可见（`IsWindowVisible` 探针）；桌面图标正常 |
| 7 | 运行中双击图标/拖动/右键 | 行为与改动前一致（本计划不触碰交互路径） |

对抗性审查要点（**独立干净上下文子代理**，实施完成后必须执行）：

- 显式 drop 化是否**真的**与原隐式顺序等价；阶段命名是否与实际析构位置相符（Rc 计数陷阱）。
- T1/T2 是否会误伤"正常但缓慢"的退出（正常收尾耗时实测上限 vs 15/45s 余量）。
- 兜底线程与主线程 `IconGuard::drop` 并发投递 `ShowWindowAsync` 的幂等性。
- 第二实例写日志与首实例日志线程并发追加同一文件的完整性。
- `TerminateProcess` 兜底下日志丢失边界（T1 已落 warn，属已知可接受损失，需复核）。
- 是否存在新的周期唤醒（对照约束 10，实测）。

## 7. 实施记录与实测纠正（2026-09-28）

本节由实施会话追加，记录落地形态、相对本计划文本的偏差及其**实测依据**。

### 7.1 已落地与验证

- 分层埋点：app 三层 `ExitPhase`（进入/离开 + ms）+ render 五段（`Drop for OverlayWindow`，
  只加日志未改顺序）；正常收尾实测全程 ~35ms（overlay 段 30ms，其中合成器/GPU 状态释放 25ms）。
- 异步恢复：`restore_icons_async()`（`ShowWindowAsync`）；`IconGuard::drop` 已改走它，
  启动期 `desktop_mode` 分支与运行期切换仍用同步版。
- 单实例留痕：`logging::init` 已前置到互斥检查之前；拒绝分支落 warn + `overlay_alive` 字段。
- 限时兜底：见 7.2。注入 `Sleep(60s)` 实测：T1(15s) warn 点名阶段（"30s 后强制结束"）→ 看门狗(30s)
  error 带 `phase` 字段 → T2(45s) 强杀（wall≈49-50s，进程散）。
- **真实死锁自清场（两次）**：实测期间该偶发死锁在约 1/4 的运行中复发（收尾卡在 overlay Drop 的
  合成器/GPU 释放段）。最终形态下：T1 warn 点名「销毁 overlay 窗口（含合成器/GPU 资源）」→ 看门狗
  带 `phase` 落 error → **T2 于 45s `TerminateProcess` 清场（进程消失、单实例互斥释放）**，
  图标由兜底投递恢复（`SysListView32` 可见）。即 plan 12 的 G3 在真实病理下闭环。
- 正常运行路径累计 15+ 次冒烟全绿：每次收尾 3 段（app）+ 5 段（render）日志齐全、进程必散。
- 空闲口径（约束 10）：60s 空闲窗口内 `winbosk-watchdog` / `winbosk-exit-info` /
  `winbosk-exit-kill` / `winbosk-menu-prime` 的 `QueryThreadCycleTime` 增量**精确为 0**
  （新增线程全程 blocked on event，零唤醒零 CPU）。
- 门禁（2026-09-28）：build / test（76+92+56+25）/ clippy `-D warnings` / fmt `--check` 全绿；
  对抗性审查（独立子代理）无阻断项，其 1 高 5 中 5 低均已处置（见 7.5）。

### 7.2 相对本计划文本的偏差（均为实测强制）

1. **兜底线程改为「启动期创建 + 事件唤醒」；T1/T2 拆成两条一次性线程**
   （`prepare_exit_deadline` / `arm_exit_deadline`）。实测依据（转储 `pid65708`）：收尾期存在
   **加载器锁死锁**——数十个线程卡在 `LdrShutdownThread`，**此刻新建的线程会卡死在
   `LdrInitializeThunk` 永不启动**，§2.4「收尾时 spawn」的线程形同虚设（现场
   `winbosk-exit-deadline` 正卡在这一步，T1/T2 全失效）。启动期创建的线程只等内核事件，
   死锁期间「被唤醒 + 继续执行」不依赖加载器锁；T2 路径零分配零加锁，是唯一能在该死锁
   下仍执行的收尾手段。倒计时仍自「进入收尾」（置位事件）起算，不会误伤运行期。
2. **T1 内先投递图标恢复、后写日志**（日志的分配/写盘可能被堆锁卡住）；kill 线程在
   `TerminateProcess` 前再投一次（幂等）——用户可见的图标恢复不依赖日志路径。
3. **覆盖边界（有意保留）**：兜底只在**进入收尾后**武装。运行期主线程停摆仍只由看门狗落
   error 日志、不做自动终止——主线程存在合法的长同步 I/O（如大目录拖入复制），运行期
   强杀有破坏性；plan 11 亦明确「不做自动恢复」。
4. **武装时机提前**：`arm_exit_deadline()` 改为 `run_message_loop()` 返回后的**第一句**（原在
   `ime_detach`/内存快照/「已退出」日志之后）——这些步骤同样可能卡在死锁上（IME 解除关联走的
   正是 imm32 链），早一步置位多一分覆盖。
5. **启动失败的 Err 路径也纳入覆盖**：新增 `ExitArmGuard`（声明在 overlay 之后 ⇒ 逆序析构先于
   overlay/device），任何 `?` 提前返回都会先武装 T1/T2；`EXIT_LISTVIEW` 改为启动期登记。
6. **恢复投递去重（claim-then-post）**：`post_icon_restore` 先原子 claim 再调 user32——info 线程
   若卡在投递调用自身，kill 线程一次原子交换即跳过、直奔 `TerminateProcess`，T2 的零依赖路径
   不被拖累。
7. 微修正：T1 文案换算真实剩余秒数（「30s 后强制结束」）；`EXIT_PHASE` 中毒时 `into_inner` 取回
   守卫继续写/读（真正的 recovering，对齐 §2.1 要求）；`%APPDATA%` 迁移留在单实例检查**之后**
   （有真实副作用，第二实例不得搬走运行中实例的数据）。

### 7.5 对抗性审查处置（独立子代理，2026-09-28）

结论：**无阻断级问题**；显式 drop 等价性（runtime→overlay→_guard 与声明逆序一致、`drop(runtime)`
只减 Rc、真析构在 overlay Drop 内）、原子序（Release 存 + SetEvent → Wait → Acquire 读）、
并发 `ShowWindowAsync` 幂等性、零周期唤醒（实测四线程 60s cycles 增量精确为 0）均核实无误。
发现及处置：[高] arm 时机 → 见 7.2-4；[中] 早退路径无兜底 → 见 7.2-5；[中] kill 线程先投恢复
再强杀 → 见 7.2-6；[中] T1 文案秒数 → 见 7.2-7；[中] `EXIT_PHASE` 中毒静默 → 见 7.2-7；
[低] `%APPDATA%` 迁移副作用 → 见 7.2-7；[低] `overlay_window_alive` 的提示性边界
（僵尸卡 DestroyWindow 前误报 alive / explorer 重启误判）→ 维持 best-effort 定位不改。

### 7.3 实测纠正（本计划文本已被证伪的两处断言）

- §0「overlay 是 WorkerW 的**子**窗口」**错误**：`OverlayWindow::create` 以 `WS_POPUP` 创建，
  WorkerW/Progman 只是它的 **owner**（`GetParent` 返回 owner）——overlay **是**顶层窗口、
  出现在 `EnumWindows` 列表里（实测 `FindWindowW("WinBoskOverlay")` 可直查）。
  因此 §2.5 的 `FindWindowExW(worker, "WinBoskOverlay")` 永远找不到：`overlay_window_alive()`
  已改为顶层 `FindWindowW`（实测第二实例日志 `overlay_alive=true`）。
- §2.5「`FindWindowW`/顶层枚举必然找不到——这正是本 helper 存在的理由」同上被证伪。

### 7.4 未决根因（超出本计划范围，供后续计划）

两个滞留现场（转储：`%TEMP%\winbosk-hang-20260928-pid65708.dmp` / `-pid66588.dmp`）
指向**同一类加载器锁死锁**，非收尾代码缺陷：

- 现场 B（`pid66588`）：启动约 2.4s 即停摆、从未进入收尾——main 卡在
  `USER32!User32InitializeImmEntryTable → LdrLoadDll`（等加载器锁）；后台线程
  `winbosk-menu-prime`（外壳扩展预热）卡在 `windows.storage.dll / SHCORE!SHCreateThread` 一线。
- 现场 A（`pid65708`）：收尾期死锁，main 卡在 `nvwgf2umx → RtlFreeHeap` 等待。
- 两现场均载有 `RTSSHooks64.dll`（RivaTuner 注入）；线程创建 / 退出 / `LoadLibrary` 全线卡死。
- 含义：本计划的 T1/T2 在**收尾期**死锁下仍能强制清场（kill 线程启动期已就绪）；但
  **收尾前**的同类死锁不在其覆盖内（见 7.2-3），表现为僵尸占互斥 + 桌面图标留在隐藏态，
  需人工结束进程（看门狗 error 日志已留痕可定位）。

## 附：与 plan 11 的接口

- plan 11 §2.1 的看门狗若实现，建议把 `crate::current_phase()` 作为停摆日志的一个字段（仅诊断用，不改停摆判定口径）。
- 次序约定：T1(15s) → 看门狗 dump(30s) → T2(45s)；实现 d 时不得把 `STALL_MS` 调到 15s 以下。
- 现场取证方法（只读采样、地址解析、判据速查）见 plan 11 附录 B，本计划不重复。
