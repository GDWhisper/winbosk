//! 动画补间：面板展开/栅栏拖动/图标悬停的补间类型与推进（纯状态，无窗口访问）。

use crate::*;

/// 面板展开/折叠缓动方向。
///
/// 展开与收起**不是**同一条曲线的正反向：展开用带轻微过冲的回弹（有「顶出来」的手感），
/// 收起必须单调收敛——过冲会让进度越过 0 被钳成 0，面板在补间结束前就已消失，
/// 尾段时间变成空转（观感「唰地没了」，而非收回）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelEase {
    /// 单调收敛（`ease_out_cubic`）：收起/回位，绝不过冲。
    CubicOut,
    /// 轻微过冲回弹（峰值约 1.05）：仅用于展开。
    BackOutSoft,
}

/// 面板展开/折叠补间（`from→to`，`dur` 秒，缓动由 `PanelEase` 指定）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PanelTween {
    pub(crate) t0: Instant,
    pub(crate) dur: f32,
    pub(crate) from: f32,
    pub(crate) to: f32,
    pub(crate) ease: PanelEase,
}

/// 控制台面板动画状态（App 层驱动，overlay `AnimTick` 定时推进）。
pub(crate) struct ConsoleAnim {
    /// 面板展开进度 0..1（0=完全隐藏，1=完整面板）。已 ease 插值。
    pub(crate) panel: f32,
    pub(crate) panel_tween: Option<PanelTween>,
}

impl ConsoleAnim {
    pub(crate) fn new(open: bool) -> Self {
        Self {
            panel: if open { 1.0 } else { 0.0 },
            panel_tween: None,
        }
    }

    /// 是否有动画在推进（决定动画定时器启停）。
    fn active(&self) -> bool {
        self.panel_tween.is_some()
    }
}

/// 栅栏拖动/缩放补间：视觉矩形从 `from` 追赶到 `to`（模型已落到 `to`，
/// 场景渲染用补间值，形成丝滑跟随；补间结束视觉 = 模型）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FenceTween {
    pub(crate) fence: usize,
    pub(crate) from: Rect,
    pub(crate) to: Rect,
    pub(crate) t0: Instant,
    pub(crate) dur: f32,
}

/// 图标悬停缩放补间（0..1：1 = 完全放大）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct IconHoverAnim {
    pub(crate) fence: usize,
    pub(crate) icon: usize,
    pub(crate) t0: Instant,
    pub(crate) dur: f32,
    /// 补间起点进度（0/1，或上一次动画的当前值——连续进出不跳变）。
    pub(crate) from: f32,
    /// 补间终点进度（1 = 放大，0 = 收回）。
    pub(crate) to: f32,
}

impl IconHoverAnim {
    pub(crate) fn progress(&self, now: Instant) -> f32 {
        match tween_progress(self.t0, self.dur, now) {
            Some(p) => self.from + (self.to - self.from) * ease_out_cubic(p),
            None => self.to,
        }
    }
}

pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0);
    1.0 - (1.0 - u).powi(3)
}

/// ease_out_back 的克制版：过冲约 5%（标准 `C1 = 1.70158` 过冲约 10%，
/// 作用在面板高度上会「长高一大截再缩回」，观感是抖一下而不是回弹）。
pub(crate) fn ease_out_back_soft(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0);
    const C1: f32 = 1.2;
    const C3: f32 = C1 + 1.0;
    1.0 + C3 * (u - 1.0).powi(3) + C1 * (u - 1.0).powi(2)
}

/// 面板不透明度：由展开进度派生，与高度**解耦**（见 `CONSOLE_FADE_SPAN`）。
///
/// 进度到 `CONSOLE_FADE_SPAN` 即完全不透明，之后只有高度在动；收起时反之——
/// 进度降到该区间内才淡出。避免「既矮又透明」的双重衰减，也避免过冲段
/// 「高度在回弹、透明度已饱和」的通道错位。
pub(crate) fn console_fade(panel: f32) -> f32 {
    (panel / CONSOLE_FADE_SPAN).clamp(0.0, 1.0)
}

/// 补间进度：`t0` 起 `dur` 秒内返回 0..1，超时返回 None（补间结束）。
pub(crate) fn tween_progress(t0: Instant, dur: f32, now: Instant) -> Option<f32> {
    let el = now.duration_since(t0).as_secs_f32();
    if el >= dur {
        None
    } else {
        Some(el / dur)
    }
}

/// 栅栏平滑滚动阻尼补间。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollTween {
    pub(crate) fence: usize,
    pub(crate) from: f32,
    pub(crate) to: f32,
    pub(crate) t0: Instant,
    pub(crate) dur: f32,
}

/// 启用动画定时器（overlay 每 16ms 触发一次 `AnimTick`）。
pub(crate) fn arm_anim_timer(rt: &mut Runtime) {
    if !rt.console_anim.active()
        && rt.desktop_fade.is_none()
        && rt.fence_tweens.is_empty()
        && !icon_hover_active(rt)
        && rt.scroll_tweens.is_empty()
    {
        return;
    }
    unsafe { (*rt.overlay_ptr).set_anim_active(true) };
}

/// 推进一帧动画：面板补间 + 栅栏补间 + 图标悬停 + 滚轮平滑滚动。
/// 返回是否仍有动画在推进（否则调用方停用定时器）。
pub(crate) fn advance_anim(rt: &mut Runtime) -> bool {
    let now = Instant::now();
    let anim = &mut rt.console_anim;
    // 桌面切换：栅栏整体淡入/淡出补间（结束即清除）
    if let Some(df) = rt.desktop_fade {
        if tween_progress(df.t0, df.dur, now).is_none() {
            rt.desktop_fade = None;
        }
    }
    // 栅栏拖动/缩放补间：结束即从表里摘除（视觉 = 模型）
    rt.fence_tweens
        .retain(|t| tween_progress(t.t0, t.dur, now).is_some());
    // 面板展开/折叠补间
    if let Some(pt) = anim.panel_tween {
        match tween_progress(pt.t0, pt.dur, now) {
            Some(p) => {
                let e = match pt.ease {
                    PanelEase::CubicOut => ease_out_cubic(p),
                    PanelEase::BackOutSoft => ease_out_back_soft(p),
                };
                anim.panel = pt.from + (pt.to - pt.from) * e;
            }
            None => {
                anim.panel = pt.to;
                anim.panel_tween = None;
            }
        }
    }
    // 栅栏平滑滚动阻尼推进
    let mut scroll_finished = false;
    rt.scroll_tweens.retain_mut(|st| {
        if let Some(p) = tween_progress(st.t0, st.dur, now) {
            let cur = st.from + (st.to - st.from) * ease_out_cubic(p);
            if let Some(f) = rt.desk.fences.get_mut(st.fence) {
                f.scroll = cur;
            }
            true
        } else {
            if let Some(f) = rt.desk.fences.get_mut(st.fence) {
                f.scroll = st.to;
            }
            scroll_finished = true;
            false
        }
    });
    // 滚动动画结束时一次性防抖持久化
    if scroll_finished {
        let _ = rt.store.save(&rt.desk);
    }

    anim.active()
        || rt.desktop_fade.is_some()
        || !rt.fence_tweens.is_empty()
        || icon_hover_active(rt)
        || !rt.scroll_tweens.is_empty()
}

/// 开始面板展开/折叠补间（`to` 目标进度）。目标立即生效于命中模型，
/// 视觉高度由补间在 `AnimTick` 逐帧推进。
pub(crate) fn start_panel_tween(rt: &mut Runtime, to: f32) {
    let from = rt.console_anim.panel;
    // 已在目标态（如重复唤出/重复收起）：直接落定，不建空转补间、不唤醒定时器。
    if (to - from).abs() <= 1e-3 {
        rt.console_anim.panel = to;
        rt.console_anim.panel_tween = None;
        return;
    }
    // 方向语义（非位置假设）：目标进度大于当前进度即展开，否则收起。
    let opening = to > from;
    let ease = if opening {
        PanelEase::BackOutSoft
    } else {
        PanelEase::CubicOut
    };
    let dur = if opening {
        CONSOLE_TWEEN_OPEN_S
    } else {
        CONSOLE_TWEEN_CLOSE_S
    };
    rt.console_anim.panel_tween = Some(PanelTween {
        t0: Instant::now(),
        dur,
        from,
        to,
        ease,
    });
    arm_anim_timer(rt);
}

/// 栅栏管理页列表最大可滚动量（物理像素；0 = 行数不超出可视区）。
pub(crate) fn fence_alpha(rt: &Runtime, now: Instant) -> f32 {
    if let Some(t) = rt.desktop_fade {
        match tween_progress(t.t0, t.dur, now) {
            Some(p) => t.from + (t.to - t.from) * ease_out_cubic(p),
            None => t.to,
        }
    } else if rt.desk.desktop_mode {
        0.0
    } else {
        1.0
    }
}

/// 栅栏当前视觉矩形：有补间时取插值（拖拽跟随），否则 = 模型矩形。
pub(crate) fn fence_visual_rect(rt: &Runtime, fence: usize) -> Rect {
    let now = Instant::now();
    if let Some(t) = rt.fence_tweens.iter().find(|t| t.fence == fence) {
        match tween_progress(t.t0, t.dur, now) {
            Some(p) => {
                let e = ease_out_cubic(p);
                Rect::new(
                    t.from.x + (t.to.x - t.from.x) * e,
                    t.from.y + (t.to.y - t.from.y) * e,
                    t.from.w + (t.to.w - t.from.w) * e,
                    t.from.h + (t.to.h - t.from.h) * e,
                )
            }
            None => t.to,
        }
    } else {
        rt.desk
            .fences
            .get(fence)
            .map(|f| f.bounds)
            .unwrap_or_default()
    }
}

/// 图标悬停当前缩放（1.0 = 常态，1.3 = 完全放大，约 1.3 倍原图标大小）。
pub(crate) fn icon_hover_scale(rt: &Runtime, fence: usize, icon: usize, now: Instant) -> f32 {
    match rt.icon_hover {
        Some(h) if h.fence == fence && h.icon == icon => 1.0 + 0.3 * h.progress(now),
        _ => 1.0,
    }
}

/// 图标悬停补间是否仍在推进（决定动画定时器是否需要跑）。
pub(crate) fn icon_hover_active(rt: &Runtime) -> bool {
    rt.icon_hover
        .map(|h| tween_progress(h.t0, h.dur, Instant::now()).is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 展开缓动：端点精确、全程非负、过冲存在且不超过几何钳制上限
    /// （一旦超过 `CONSOLE_OVERSHOOT_MAX`，高度会被截断成「停顿一下再回弹」）。
    #[test]
    fn back_out_soft_is_bounded_by_overshoot_clamp() {
        assert!(ease_out_back_soft(0.0).abs() < 1e-5);
        assert!((ease_out_back_soft(1.0) - 1.0).abs() < 1e-5);
        let samples = (0..=1000).map(|i| i as f32 / 1000.0);
        let peak = samples
            .clone()
            .map(ease_out_back_soft)
            .fold(0.0_f32, f32::max);
        assert!(peak > 1.0, "展开应保留过冲，实测峰值 {peak}");
        assert!(
            peak <= CONSOLE_OVERSHOOT_MAX,
            "过冲峰值 {peak} 超过几何钳制上限 {CONSOLE_OVERSHOOT_MAX}"
        );
        assert!(samples.map(ease_out_back_soft).all(|v| v >= 0.0));
    }

    /// 收起必须单调收敛且恒落在 [0, 1]：过冲会让进度越过 0 被钳成 0，面板在补间
    /// 结束前就消失、尾段定时器空转（观感「唰地没了」）——本用例是该缺陷的回归守卫。
    #[test]
    fn close_tween_never_undershoots_zero() {
        let (from, to) = (1.0_f32, 0.0_f32);
        let mut prev = from;
        for i in 1..=100 {
            let p = i as f32 / 100.0;
            let panel = from + (to - from) * ease_out_cubic(p);
            assert!(
                panel <= prev + 1e-6,
                "收起进度必须单调不增: {panel} > {prev}"
            );
            assert!((0.0..=1.0).contains(&panel), "收起进度越界: {panel}");
            prev = panel;
        }
        assert!(prev.abs() < 1e-6, "收起终值应精确为 0，实测 {prev}");
    }

    /// 不透明度与高度解耦：进度到 `CONSOLE_FADE_SPAN` 即完全不透明，过冲段饱和为 1。
    #[test]
    fn console_fade_saturates_before_full_height() {
        assert_eq!(console_fade(0.0), 0.0);
        assert!((console_fade(CONSOLE_FADE_SPAN * 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(console_fade(CONSOLE_FADE_SPAN), 1.0);
        assert_eq!(console_fade(1.0), 1.0);
        assert_eq!(console_fade(1.05), 1.0);
    }
}
