//! 主线程看门狗：主线程停摆时由独立线程落一条 `error` 日志（停摆自证）。
//!
//! 触发面（2026-09-28 实测）：主线程点击栅栏图标后卡进 Windows 输入法/TSF
//! （imm32 ↔ MSCTF）的同步等待——事件循环、4s 心跳日志、托盘/热键一并失效，
//! 日志直接断流，假死在日志里没有任何痕迹。本模块让停摆可自证：
//!
//! - 进消息循环前武装一个**一次性**可等待定时器（相对时间，`STALL_MS` 后到期）；
//! - 主线程每派发一个 UI 事件就调 [`heartbeat`] 刷新时间戳并续期定时器——**不新增
//!   周期唤醒**：空闲期唯一的事件源就是既有的 4s `SyncLibrary` 心跳，约束 10 的
//!   `QueryThreadCycleTime` 判据不变；
//! - 主线程停摆 ⇒ 定时器到期 ⇒ 看门狗线程核对心跳时间戳后写 `error` 日志
//!   （tracing 的 appender 是独立线程，主线程停摆不影响落盘），随后线程退出。
//!   日志带 `phase` 字段（收尾阶段名，见 `main.rs` 的 `ExitPhase`）——退出收尾滞留
//!   时直接点名卡点。
//!
//! **已知上限**：若主线程停摆时恰好持有进程堆锁，看门狗线程的日志格式化分配也可能
//! 阻塞——本机制覆盖「等事件/等系统调用」型停摆，覆盖不了「持堆锁死锁」型。
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
/// 停摆判定——否则冷启动偶发的慢初始化会误报，并把一次性的看门狗白白耗掉。
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
pub(crate) fn heartbeat() {
    MAIN_HEARTBEAT_MS.store(now_mono_ms(), Ordering::Relaxed);
    let timer = HANDLE(WATCHDOG_TIMER.load(Ordering::Relaxed) as *mut core::ffi::c_void);
    if !timer.is_invalid() {
        let _ = rearm(timer);
    }
}

/// 看门狗线程：定时器到期后核对心跳——两次采样都确认停摆才落 `error` 日志并退出
/// （一次性；一次误判会白烧掉本次进程的取证窗口，故宁可多复核一次）。
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
        let stalled_ms = stall_age_ms(now_mono_ms(), last, START_MS.load(Ordering::Relaxed));
        tracing::error!(
            stalled_ms,
            stalled_s = stalled_ms / 1000,
            last_heartbeat_ms = last,
            // 收尾阶段名（`ExitPhase` 写入；空串 = 不在收尾期）。仅诊断用，
            // 不参与停摆判定口径——退出收尾滞留时它直接点名卡点。
            phase = crate::current_phase(),
            "主线程停摆：≥{}s 未回到事件循环（疑似卡死在同步调用，或正在执行长同步 I/O；进程仍存活，可用任务管理器右键该进程「创建转储文件」取证）",
            stalled_ms / 1000
        );
        return;
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
