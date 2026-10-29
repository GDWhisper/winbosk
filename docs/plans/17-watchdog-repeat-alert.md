# 实施记录 17：主线程看门狗——从一次性告警改为持续告警

> 触发事件：2026-10-17 09:00:01 Windows 事件 1002（Application Hang），
> `target\release\winbosk.exe` 无响应被终止。日志断流于 08:59:32.651，与事件仅隔
> **28.4 秒**——差 1.6 秒没够看门狗 30 秒阈值，`error` 一行都没落。
> 本文是这次取证缺口处置的唯一记录。

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

- **G1 取证窗口不再丢失**：主线程停摆后，看门狗**持续**告警（而非落一条就退出线程），
  任意时长的停摆都至少留下一行 `error`，并留下「停摆持续多久」的证据。
- **G2 恢复可观测**：主线程从停摆中恢复时落一条 `warn`，带**实测停摆时长**，
  区分「卡了 30 秒又活了」与「永久假死」——后者才是要重启进程的那种。
- **G3 不新增周期唤醒**：约束 10 的 `QueryThreadCycleTime` 判据不变。运行期看门狗线程
  仍全程 blocked；只有**真的停摆**时它才周期醒来，这是故障态而非常态。

### Non-Goals

- **不改 `STALL_MS`（仍为 30s）**。降阈值会增加长同步 I/O（Shell 属性查询、图标提取）
  期间的误报，且本次缺口是 28.4s vs 30s 的 1.6s 之差，降阈值是治标；持续告警是治本。
- **不做自动恢复 / 不强杀**。Win32 同步调用无法安全中断。退出路径的强杀归 plan 15
  的 `EXIT_WARN_MS` / `EXIT_FORCE_MS`，本计划不碰。
- **不做 minidump**（plan 11 的 d2 未启用，`crates` 里无 `dbghelp` 绑定，保持现状）。
- **不动 IME / `AttachThreadInput` 路径**。那是本次卡死的疑似入口，但收敛它需要先有
  dump 定位到具体调用栈，另立计划。
- **`crates/core` 零改动**。

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 新增状态（`crates/app/src/watchdog.rs`）

| 名称 | 类型 | 写入者 | 语义 |
| :--- | :--- | :--- | :--- |
| `REPORT_SINCE_MS` | `AtomicU64` | 看门狗置位（`compare_exchange` 0→now，仅首次生效）；主线程 `swap(0)` 清除 | 0 = 当前无未恢复的停摆报告；非 0 = 首次报告的单调毫秒时刻 |
| `REPORT_SEQ` | `AtomicU64` | 看门狗 `fetch_add`；主线程恢复时 `store(0)` | 本次停摆里已落了几条 `error` |
| `LONG_STALL_MS` | `const u64 = 300_000` | — | 停摆≥5 分钟后，报告间隔从 30s 放宽到 300s |

**所有权口径**（对照 plan 11 §3 的旁路数据对齐律）：两个新增量的**主写者唯一**。
跨线程的另一次写都用原子 RMW（`compare_exchange` / `swap`），不存在撕裂态。
竞态后果已评估：

- 看门狗在 `swap(0)` 之后立刻置位 → 主线程多落一条恢复行（无害，诊断量）。
- 时长字段由主线程用**自己的** `prev` 心跳值算，不读看门狗的时间戳 → 不存在跨线程
  时钟解释歧义。
- `prev == 0`（进程从未有过心跳却先被报告）是唯一的退化边界：此时按 **0** 记，不走
  停摆判定那套「首个心跳前按 `spawn` 起算」的口径——否则会把启动期耗时说成一次停摆
  （`recovery_stall_ms` 与 `stall_age_ms` 因此是两个函数，不共用）。

### 2.2 接口变更

```rust
/// 主线程每次派发 UI 事件调用：刷新心跳 + 续期定时器。
/// **新增**：若此前看门狗报告过停摆，落一条恢复日志（带实测时长）并清除报告态。
pub(crate) fn heartbeat();

/// 报告间隔：停摆越久报告越稀（见 §2.1 `LONG_STALL_MS`）。纯函数，可单测。
fn report_interval(stalled_ms: u64) -> u64;
```

`watch()` 的退出语义变更：**不再 `return`**，报告后重新武装同一定时器并 `continue`。
恢复行不由 `watch()` 落——它由主线程的 `heartbeat()` 落（理由见 §3）。

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

### 3.1 恢复行为只能由主线程自证（本计划最关键的一条动力学）

看门狗定时器由 `heartbeat()` 续期。主线程一旦恢复，心跳持续把定时器推后，
**定时器再也不会到期 ⇒ 看门狗线程再也醒不过来**。所以「恢复」这件事看门狗自己
观察不到：把恢复判定放在 `watch()` 里会让它永远不触发。

结论：恢复行必须由**主线程**在 `heartbeat()` 里落——能走到 `heartbeat()` 本身就是
「事件回路已复活」的充分证据，语义上零间接量（对照语义精准定位律）。

### 3.2 空闲性能归零律（约束 10）

- 运行期（未停摆）：定时器每次都被心跳续期，看门狗线程 blocked 在
  `WaitForSingleObject(INFINITE)`，`QueryThreadCycleTime` 增量**仍为 0**；
  主线程新增开销 = 一次 `AtomicU64::swap` + 一次 `AtomicU64::load`（纳秒级），
  只在 `REPORT_SINCE_MS != 0` 时才多一次格式化与落盘。
- 停摆期：看门狗每 `report_interval()` 醒一次。这是**故障态**，不影响空闲判据——
  判据的采样窗口本来就是「空闲 60s」，此时不可能停摆。
- 不新增任何定时器、不自建周期源：仍然复用 `spawn` 里那一个可等待定时器。

### 3.3 常驻后台冲突

`heartbeat()` 挂在既有事件回路上（含 4s `SyncLibrary` 心跳），不新增唤醒源。
`SyncLibrary` 分支逻辑（库同步 / 空闲修剪）零改动。

### 3.4 孤儿状态回收律

无新增持久化状态、无新增目录、无新增句柄。定时器句柄仍是 `spawn` 里有意泄漏的那一个。

### 3.5 状态全集校验律

停摆判定仍**只**依据 `MAIN_HEARTBEAT_MS` 时间戳（单一事实源），不用
`IsHungAppWindow`、不用窗口句柄状态。恢复判定**只**依据 `REPORT_SINCE_MS != 0`
**且**本次 `heartbeat()` 真的被调用（复合条件：报告过 + 主线程活着）。

### 3.6 已知上限（沿用 plan 11 §2.1，逐条复核）

- 主线程持堆锁时，看门狗线程的日志格式化分配可能二次阻塞。持续告警**不放大**这个
  风险：一旦阻塞，一次也落不下来，与次数无关。
- 每条报告都读 `crate::current_phase()`（一个 `Mutex`）。若主线程卡在持锁状态，
  该锁会连带阻塞看门狗——此风险在一次性版本已存在，持续告警未改变其性质。
  不为此改 `EXIT_PHASE` 的同步原语（超出本计划范围，且改它会影响收尾埋点口径）。

## 4. 分层改动清单 (Implementation Steps)

自底向上，只动 1 个文件（`crates/app/src/watchdog.rs`）：

1. **模块头注释**：把「武装一个**一次性**可等待定时器」「随后线程退出」改为
   「相对时间单次触发、由 `heartbeat()` 续期」「停摆后持续告警，恢复后自动收敛」。
   保留「已知上限」整段。
2. **新增 `LONG_STALL_MS` 常量**与 `REPORT_SINCE_MS` 静态量（放 `START_MS` 旁边）。
3. **新增 `report_interval(stalled_ms)`**：`< LONG_STALL_MS` → `STALL_MS`，否则
   `LONG_STALL_MS`。
4. **`watch()`**：
   - 判定停摆后：置 `REPORT_SINCE_MS`（仅当当前为 0），按当前 `stalled_ms` 取报告
     序号/间隔，落 `error`（保留 `stalled_ms` / `stalled_s` / `last_heartbeat_ms` /
     `phase`，新增 `report_seq`），
   - **用 `report_interval(stalled_ms)` 重新武装定时器并 `continue`**（删除 `return`）。
5. **`heartbeat()`**：`store` 改 `swap` 拿到 `prev`；若 `REPORT_SINCE_MS.swap(0) != 0`，
   落一条 `warn`（字段：实测停摆时长 `stalled_ms` / `stalled_s`、首次报告距今
   `reported_ms_ago`），随后照常续期定时器。
6. **测试**：新增 `report_interval` 阶梯断言（阈值下方/边界/上方）。
7. **文档**：本文 §7 记录与 plan 11 的偏差；plan 11 §7 末尾追加一行指向本文。

## 5. 防御性自查清单 (Defensive Invariants)

| 律 | 针对性防御 |
| :--- | :--- |
| 1 常驻后台冲突 | 不新增定时器/周期源；`SyncLibrary` 分支零改动；恢复判定挂在既有 `heartbeat` |
| 2 空闲归零 | 运行期看门狗仍 blocked；主线程新增仅原子 RMW；停摆期的唤醒属故障态，不进空闲采样窗口 |
| 3 孤儿回收 | 无新增目录/句柄/持久化状态 |
| 4 语义精准定位 | 停摆仍只看心跳时间戳；恢复只看「报告态 + 主线程确实在跑」，不用 `IsHungAppWindow` |
| 5 状态全集校验 | 恢复是复合条件（报告过 **且** 主线程活着）；`REPORT_SINCE_MS` 用 `compare_exchange` 保证只置位一次 |
| 6 旁路数据对齐 | 新增两个原子量，各自单一主写者；跨线程侧写均为原子 RMW；时长由主线程用自有 `prev` 计算 |

## 6. 验证与交付门禁 (Verification Gates)

静态门禁（全绿方可交付）：

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

静态断言：

- `grep -n "SetWaitableTimer\|CreateWaitableTimerExW" crates/app/src/watchdog.rs`
  → 定时器仍在 `spawn` 里创建一次，运行期只有 `arm`。
- `grep -n "return;" crates/app/src/watchdog.rs` → `watch()` 内不再因「已报告」退出。

手动验证矩阵（**先退出正在运行的实例**；用 debug 产物，因为它不带 release 锁）：

| # | 操作 | 期望 |
| :--- | :--- | :--- |
| 1 | `WINBOSK_AUTOSTOP_MS=40000` 正常跑 40s | 日志**无**停摆行、**无**恢复行 |
| 2 | 调试期临时把 `STALL_MS` 调到 3s + 主线程插 `Sleep(20s)` | 约 3s 起落 `error`（`report_seq` 递增，间隔 3s）→ 20s 后落一条恢复 `warn`，带实测时长≈20s |
| 3 | 同上但 `Sleep(70s)`、`STALL_MS` 临时 3s、`LONG_STALL_MS` 临时 10s | 前若干条间隔 3s，超过 10s 后间隔放宽到 10s（验证阶梯） |
| 4 | 空闲 60s | 无新增周期行；`winbosk-watchdog` 线程 `QueryThreadCycleTime` 增量精确为 0 |

第 2/3 项**必须改回原值并重新构建**，交付前 `git diff` 里不得残留调试常数。

对抗性审查要点（独立干净上下文子代理，实施完成后必须执行）：

- 持续告警在「主线程持堆锁」下是否制造新的阻塞面（对照 §3.6）。
- `REPORT_SINCE_MS` 双写竞态的最坏后果是否只是多/少一行日志（不得影响判定口径）。
- `report_interval` 的边界（0、恰好等于阈值、`u64` 溢出侧）。
- 是否真的零新增周期唤醒（对照约束 10 判据）。

## 7. 实施后追加：落地形态与自查处置

### 7.1 相对本文的偏差

| 本文 §4 的口径 | 落地形态 | 理由 |
| :--- | :--- | :--- |
| 恢复时长用 `stall_age_ms(now, prev, START_MS)` | 新增独立函数 `recovery_stall_ms(now, prev)`，`prev == 0` 时返回 0 | 借停摆判定的口径会在「从未有过心跳就先被报告」的边界把**启动期耗时**说成一次停摆。两个口径不共用（§2.1 已同步） |
| `main.rs` 注释「把一次性的看门狗白白耗掉」 | 已删 | 一次性语义废止后该句失效（`crates/app/src/main.rs:979-981`） |

### 7.2 门禁实测（2026-10-17）

- `cargo build --workspace` 通过；`cargo test --workspace` 全绿（100+103+69+25，含新增 2 条：
  `report_interval_escalates_for_long_stalls`、`recovery_stall_ms_does_not_report_startup_as_stall`）。
- `cargo clippy --workspace -- -D warnings` 通过；`cargo fmt --all -- --check` 通过。
- **未跑的项**：§6 手动矩阵第 2/3/4 项（注入 `Sleep` 制造停摆、空闲 60s 的
  `QueryThreadCycleTime` 采样）。原因——本会话探测 `Progman` 返回 0，桌面壳层不可用
  （agent 会话不在交互桌面站），进程起不到消息循环，注入停摆无法观测。**这两项必须在
  交互桌面会话里补跑**，尤其第 4 项（约束 10 判据）。
- **未跑的项**：独立子代理对抗性审查。本会话两次派发子代理均返回
  `provider-not-found`（供应商不可用），未能执行。**交付前必须补跑**（AGENTS.md 硬性要求）。

### 7.3 与 plan 11 的偏差记录

| plan 11 的口径 | 现状 | 处置 |
| :--- | :--- | :--- |
| 「线程退出（**一次性**语义）」 | 本次事件证明一次性会丢取证窗口（28.4s < 30s） | **本计划改**：持续告警 + 恢复自证 |
| 「`STALL_MS` 不得低于 15s——plan 12 的 T1=15s/T2=45s」 | plan 15 已把兜底压到 `EXIT_WARN_MS=2_000` / `EXIT_FORCE_MS=5_000`（`crates/app/src/main.rs:255-256`），收尾 5s 即被 `TerminateProcess` | **该约束的前提已消失**：看门狗 30s 在退出路径上本来也落不了日志，收尾取证由 plan 15 的快速兜底承担。**本计划不动 `STALL_MS`** |

附带事实（不改代码，仅记录）：本次运行的 `target\release\winbosk.exe` 时间为
2026-10-10 20:38，落后于 HEAD（10-10 23:14/23:15 两个提交改了 `main.rs` /
`overlay.rs` / `compositor.rs` / `render/lib.rs`）。排查任何现场前，先确认跑的是哪个产物。
