//! 主线程看门狗：主线程停摆时由独立线程落一条 `error` 日志（停摆自证）。
//!
//! 触发面（2026-09-28 实测）：主线程点击栅栏图标后卡进 Windows 输入法/TSF
//! （imm32 ↔ MSCTF）的同步等待——事件循环、4s 心跳日志、托盘/热键一并失效，
//! 日志直接断流，假死在日志里没有任何痕迹。本模块让停摆可自证：
//!
//! - 进消息循环前武装一个**单次触发**的可等待定时器（相对时间，`STALL_MS` 后到期）。
//!   定时器全程只有一个，由 [`heartbeat`] 反复续期——**不是**周期定时器；
//! - 主线程每派发一个 UI 事件就调 [`heartbeat`] 刷新时间戳并续期定时器——**不新增
//!   周期唤醒**：空闲期唯一的事件源就是既有的 4s `SyncLibrary` 心跳，约束 10 的
//!   `QueryThreadCycleTime` 判据不变；
//! - 主线程停摆 ⇒ 定时器到期 ⇒ 看门狗线程核对心跳时间戳后写 `error` 日志
//!   （tracing 的 appender 是独立线程，主线程停摆不影响落盘），随后**重新武装同一个
//!   定时器继续观察**——停摆不结束就持续告警，且停摆越久报告越稀（见
//!   [`report_interval`]）。日志带 `phase` 字段（收尾阶段名，见 `main.rs` 的
//!   `ExitPhase`）——退出收尾滞留时直接点名卡点；
//! - 主线程恢复 ⇒ 由 [`heartbeat`] 落一条 `warn`（带实测停摆时长）并清除报告态。
//!
//! **为什么告警必须是持续的**（2026-10-17 事件教训）：当天进程无响应被终止，日志断流
//! 到事件记录只有 **28.4 秒**，差 1.6 秒没够 30 秒阈值——原「落一条就退出线程」的一次性
//! 语义下，这行 `error` 永远不会落，取证窗口整个丢失。持续告警把「停摆多久」变成可
//! 观测量：任意时长的停摆都至少留下一行。
//!
//! **为什么「恢复」只能由主线程自证**：定时器由 [`heartbeat`] 续期，主线程一旦恢复，
//! 心跳会把到期时刻不断推后——定时器再也不会到期，看门狗线程再也醒不过来。所以恢复
//! 判定只能挂在 [`heartbeat`]：能走到那里本身就是「事件回路已复活」的充分证据。
//!
//! **已知上限**：若主线程停摆时恰好持有进程堆锁，看门狗线程的日志格式化分配也可能
//! 阻塞——本机制覆盖「等事件/等系统调用」型停摆，覆盖不了「持堆锁死锁」型。持续告警
//! **不放大**该风险：一旦阻塞，一次也落不下来，与报告次数无关。
//! 不做自动恢复：Win32 同步调用无法安全中断，要恢复只能重启进程。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
    TIMER_MODIFY_STATE,
};

/// 主线程心跳（进程内单调毫秒）。**只由主线程写**、看门狗只读。
/// 0 = 尚未收到第一个心跳，此时停摆时长按 `START_MS`（spawn 时刻）起算。
static MAIN_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

/// 看门狗定时器句柄的原始值（`HANDLE` 非 `Sync`，跨线程只传数值）。
/// 只在 `spawn` 创建一次，进程生命周期内一直持有（有意不回收）。
static WATCHDOG_TIMER: AtomicU64 = AtomicU64::new(0);

/// `spawn` 时刻（单调毫秒）：首个心跳到来前的停摆起算点。
static START_MS: AtomicU64 = AtomicU64::new(0);

/// 单调时钟基准：`Instant` 不受 NTP 授时/手改系统时间的跳变影响（墙钟会误判停摆）。
static MONO_BASE: OnceLock<Instant> = OnceLock::new();

/// 续期失败告警只发一次。
static ARM_WARNED: AtomicBool = AtomicBool::new(false);

/// 停摆阈值：心跳 4s ⇒ 30s = 连续缺 7 个心跳（正常负载下不可能）。
///
/// **不得低于 15s**：plan 12 的退出收尾兜底（T1=15s 落 warn、T2=45s 强制结束）把本
/// 阈值当作取证窗口的起点（见 `docs/plans/11-hang-resilience-and-ime-scope.md` §7）。
const STALL_MS: u64 = 30_000;

/// 长停摆门槛：停摆超过 5 分钟后，报告间隔从 [`STALL_MS`] 放宽到本值（见
/// [`report_interval`]）。永久假死不需要每 30 秒刷一行——「卡死了多久」由日志首末
/// 行的时间戳与 `stalled_ms` 字段读出，密集重复不增加信息量。
const LONG_STALL_MS: u64 = 300_000;

/// 首次报告时刻（进程内单调毫秒）；0 = 当前没有未恢复的停摆报告。
///
/// 写入者两端、语义不对称（对照旁路数据对齐律）：
/// - 看门狗：`compare_exchange(0 → now)` —— **只有首次生效**，后续报告沿用同一时刻，
///   于是「报告了几次」与「首次报告距今多久」是两个独立观测量；
/// - 主线程：[`heartbeat`] 用 `swap(0)` 清除（恢复自证）。
///
/// 竞态最坏后果：看门狗在清除之后立刻置位 ⇒ 主线程多落一条恢复行。诊断量，无害。
static REPORT_SINCE_MS: AtomicU64 = AtomicU64::new(0);

/// **本次停摆**里已落了几条 `error`（第 1 条 = 刚越过阈值）。
///
/// 看门狗单调递增；主线程在恢复自证时清零（[`heartbeat`]）。跨线程竞态的最坏后果是
/// 序号重复或跳号——纯诊断量，不参与任何判定。
static REPORT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 报告间隔（毫秒）：停摆越久报告越稀。纯函数，可单测。
fn report_interval(stalled_ms: u64) -> u64 {
    if stalled_ms < LONG_STALL_MS {
        STALL_MS
    } else {
        LONG_STALL_MS
    }
}

fn now_mono_ms() -> u64 {
    MONO_BASE.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// 停摆时长（毫秒）：距最后一次心跳的间隔；从未有过心跳时按 `start` 起算。
fn stall_age_ms(now: u64, last_heartbeat: u64, start: u64) -> u64 {
    now.saturating_sub(if last_heartbeat == 0 {
        start
    } else {
        last_heartbeat
    })
}

/// 恢复日志用的停摆时长（毫秒）：**从不停摆判定里那套「首个心跳前按 spawn 起算」的
/// 口径退化**——[`heartbeat`] 拿到的 `prev` 为 0 意味着本进程从未有过心跳，此时
/// 「距上次心跳的间隔」无从谈起，报 0 而不是把启动期耗时说成一次停摆。
fn recovery_stall_ms(now: u64, prev_heartbeat: u64) -> u64 {
    if prev_heartbeat == 0 {
        0
    } else {
        now.saturating_sub(prev_heartbeat)
    }
}

/// 创建可等待定时器（优先 `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`，失败退化为普通
/// 可等待定时器；两者都不成才返回 `None`）。与 `winbosk-render` 动画时钟同款 API。
fn create_timer() -> Option<HANDLE> {
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
    (!h.is_invalid()).then_some(h)
}

/// 以相对时间武装/续期定时器：`due` 为负 = 相对时间（单位 100ns），`lPeriod = 0`
/// ⇒ 单次触发、不自动重排。重新武装同时把已触发的信号复位（手动复位定时器语义）。
fn arm(timer: HANDLE, ms: u64) -> bool {
    let due: i64 = -((ms as i64) * 10_000);
    unsafe { SetWaitableTimer(timer, &due, 0, None, None, false).is_ok() }
}

/// 续期/重新武装；失败会持续发生（逐次告警会刷屏），只告警一次。
fn rearm(timer: HANDLE) -> bool {
    if arm(timer, STALL_MS) {
        return true;
    }
    if !ARM_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!("看门狗定时器续期失败：主线程停摆将无日志自证");
    }
    false
}

/// 启动看门狗：创建定时器并起一个线程等它到期。返回后调用方无需持有任何句柄。
///
/// 在**进消息循环前**调用：启动期的重活（图标提取 / Shell 右键菜单预热）不计入
/// 停摆判定——否则冷启动偶发的慢初始化会误报。
pub(crate) fn spawn() {
    START_MS.store(now_mono_ms(), Ordering::Relaxed);
    let Some(timer) = create_timer() else {
        tracing::warn!("看门狗定时器创建失败：主线程停摆将无日志自证");
        return;
    };
    if !arm(timer, STALL_MS) {
        tracing::warn!("看门狗定时器武装失败：主线程停摆将无日志自证");
        return;
    }
    // `HANDLE` 内部是裸指针、非 `Send`：跨线程只传原始数值，在闭包内重建。
    let raw = timer.0 as u64;
    WATCHDOG_TIMER.store(raw, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("winbosk-watchdog".into())
        .spawn(move || watch(HANDLE(raw as *mut core::ffi::c_void)));
    if spawned.is_err() {
        tracing::warn!("看门狗线程创建失败：主线程停摆将无日志自证");
    }
}

/// 主线程每派发一个 UI 事件调用（含模态菜单期间的嵌套派发）：刷新心跳时间戳并把
/// 定时器续期到 `STALL_MS` 之后。挂在既有事件回路上，不建自己的周期源。
///
/// 若此前看门狗报告过停摆（[`REPORT_SINCE_MS`] 非 0），这里同时落一条**恢复**日志并
/// 清除报告态——能走到本函数即证明事件回路已复活。停摆时长由 `prev`（本次刷新前的
/// 心跳时刻）实测得出，不读看门狗写的时间戳，避免跨线程时钟解释歧义。
pub(crate) fn heartbeat() {
    let now = now_mono_ms();
    let prev = MAIN_HEARTBEAT_MS.swap(now, Ordering::Relaxed);
    let reported_at = REPORT_SINCE_MS.swap(0, Ordering::Relaxed);
    if reported_at != 0 {
        // 恢复自证：报告态非空 ⇒ 主线程刚刚从一次已报告的停摆里回来。
        // `prev == 0` 只可能出现在「从未有过心跳就先被报告」的边界，此时按 0 记
        // （启动期耗时不是停摆，见 [`recovery_stall_ms`]）。
        let stalled_ms = recovery_stall_ms(now, prev);
        REPORT_SEQ.store(0, Ordering::Relaxed); // 本次停摆结束，序号归零。
        tracing::warn!(
            stalled_ms,
            stalled_s = stalled_ms / 1000,
            reported_ms_ago = now.saturating_sub(reported_at),
            "主线程已从停摆中恢复：事件回路重新可用（停摆时长为实测值；若反复发生请按日志抓取转储）"
        );
    }
    let timer = HANDLE(WATCHDOG_TIMER.load(Ordering::Relaxed) as *mut core::ffi::c_void);
    if !timer.is_invalid() {
        let _ = rearm(timer);
    }
}

/// 看门狗线程：定时器到期后核对心跳——两次采样都确认停摆才落 `error` 日志，随后
/// **重新武装同一定时器继续观察**（持续告警；停摆不结束就一直报，间隔见
/// [`report_interval`]）。
///
/// 两次采样复核的理由（沿用原一次性版本）：系统休眠/唤醒的瞬间、或长同步操作刚收尾
/// 时，心跳可能还陈旧而主线程已经复活——隔 1.5s 再采样，两次都停摆才判定（不把
/// 「睡醒」误报成卡死）。误报的代价已从「白烧掉取证窗口」降为「多几行日志」，但
/// 宁可多复核一次仍然成立。
fn watch(timer: HANDLE) {
    loop {
        let rc = unsafe { WaitForSingleObject(timer, INFINITE) };
        if rc.0 != WAIT_OBJECT_0.0 {
            // 等待失败（句柄异常等）：静默退出，绝不能带着坏句柄空转。
            return;
        }
        if !stalled() {
            // 到期与续期竞态（心跳刚落地、定时器未及重置）：重新武装继续观察。
            if !rearm(timer) {
                return;
            }
            continue;
        }
        // 二次采样复核：系统休眠/唤醒的瞬间、或长同步操作刚收尾时，心跳可能还陈旧而
        // 主线程已经复活——隔 1.5s 再采样，两次都停摆才判定（不把「睡醒」误报成卡死）。
        std::thread::sleep(std::time::Duration::from_millis(1_500));
        if !stalled() {
            if !rearm(timer) {
                return;
            }
            continue;
        }
        let last = MAIN_HEARTBEAT_MS.load(Ordering::Relaxed);
        let now = now_mono_ms();
        let stalled_ms = stall_age_ms(now, last, START_MS.load(Ordering::Relaxed));
        // 只置位一次：后续报告沿用同一时刻，「报告了几次」与「首次报告距今多久」
        // 于是成为两个独立观测量。
        let _ = REPORT_SINCE_MS.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        let seq = REPORT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::error!(
            stalled_ms,
            stalled_s = stalled_ms / 1000,
            last_heartbeat_ms = last,
            report_seq = seq,
            // 收尾阶段名（`ExitPhase` 写入；空串 = 不在收尾期）。仅诊断用，
            // 不参与停摆判定口径——退出收尾滞留时它直接点名卡点。
            phase = crate::current_phase(),
            "主线程停摆：≥{}s 未回到事件循环（疑似卡死在同步调用，或正在执行长同步 I/O；进程仍存活，可用任务管理器右键该进程「创建转储文件」取证）",
            stalled_ms / 1000
        );
        // 持续观察：重新武装同一定时器。间隔随停摆时长放宽——永久假死不需要每 30s
        // 刷一行，首末行时间戳已给出时长。
        if !arm(timer, report_interval(stalled_ms)) {
            // 武装失败：退化为默认间隔再试一次（`rearm` 内只告警一次，不刷屏）；
            // 仍失败说明句柄已失效，退出而非带着坏句柄空转。
            if !rearm(timer) {
                return;
            }
        }
    }
}

/// 采样判定：距最后一次心跳的间隔是否已达 `STALL_MS`。
fn stalled() -> bool {
    let last = MAIN_HEARTBEAT_MS.load(Ordering::Relaxed);
    stall_age_ms(now_mono_ms(), last, START_MS.load(Ordering::Relaxed)) >= STALL_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::WAIT_TIMEOUT;

    /// 停摆时长口径：首个心跳前按 spawn 起算；有过心跳后按心跳起算；时钟回退不为负。
    #[test]
    fn stall_age_uses_spawn_until_first_heartbeat() {
        assert_eq!(stall_age_ms(31_000, 0, 1_000), 30_000);
        assert_eq!(stall_age_ms(5_000, 4_000, 0), 1_000);
        assert_eq!(stall_age_ms(3_000, 4_000, 0), 0);
    }

    /// 恢复时长口径：`prev == 0`（从未有过心跳）时报 0，而不是把启动期耗时说成停摆。
    #[test]
    fn recovery_stall_ms_does_not_report_startup_as_stall() {
        assert_eq!(recovery_stall_ms(10_000, 4_000), 6_000);
        assert_eq!(recovery_stall_ms(10_000, 0), 0);
        assert_eq!(recovery_stall_ms(3_000, 4_000), 0); // 时钟回退不为负
    }

    /// 报告间隔阶梯：短停摆按 `STALL_MS`，长停摆放宽到 `LONG_STALL_MS`。
    #[test]
    fn report_interval_escalates_for_long_stalls() {
        assert_eq!(report_interval(0), STALL_MS);
        assert_eq!(report_interval(STALL_MS), STALL_MS);
        assert_eq!(report_interval(LONG_STALL_MS - 1), STALL_MS);
        assert_eq!(report_interval(LONG_STALL_MS), LONG_STALL_MS);
        assert_eq!(report_interval(u64::MAX), LONG_STALL_MS);
    }

    /// 相对时间武装 → 到期触发；再次武装把已触发的信号复位（手动复位语义）。
    /// 顺带验证「相对时间单次触发」这条零周期唤醒的地基。
    #[test]
    fn timer_fires_after_relative_due_and_rearms() {
        let Some(t) = create_timer() else {
            return; // 环境不支持可等待定时器：跳过（正式运行会 warn）
        };
        assert!(arm(t, 30));
        let rc = unsafe { WaitForSingleObject(t, 5_000) };
        assert_eq!(rc.0, WAIT_OBJECT_0.0);
        assert!(arm(t, 60_000));
        let rc = unsafe { WaitForSingleObject(t, 0) };
        assert_eq!(rc.0, WAIT_TIMEOUT.0);
    }
}
