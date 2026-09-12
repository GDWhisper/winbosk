# 控制中心呼出/收起动画（卷帘揭示）实施计划

背景：控制中心（控制台面板）的开合动画由单一进度标量 `ConsoleAnim.panel`（0..1，0.24s，
`ease_out_back`）驱动，同时映射到**面板高度**（`scene.rs::console_geometry`）与**整体透明度**
（`draw.rs::draw_console`）。实测观感「不流畅、有点奇怪」，根因是这单一进度的四种误用：

1. **内容不随高度裁切**：`build_console` 里所有行/按钮/详情的 y 只依赖 `panel.y`，与 `panel.h`
   无关；`draw_console` 也没有面板级 clip。展开中内容按最终布局整块绘制，一半浮在边框之外
   的桌面上，边框再从上方「追」下来 —— 观感是「零件先散出来，盒子后跟上」。
2. **高度与透明度共用同一进度**：双重衰减。起步既矮又透明（前两帧实际亮度不足 20%），
   中段又猛地亮起，主观上「先没反应，然后啪一下」。
3. **过冲只作用于高度**：`a = panel.clamp(0,1)` 早早饱和，回弹阶段只有高度在缩
   （峰值 1.10 → 1.0），透明度不动，两个通道不同步，末段看着像「抖了一下」。
4. **收起被提前截断**：收起是 `from=1 → to=0`，`panel = 1 - e`，而 `ease_out_back` 峰值 1.10
   → 中途 `panel` 变负并被 clamp 成 0，面板在约 0.17s 就消失，最后 0.07s 定时器空转。

顺带发现两处既有缺陷（同属本面板几何契约，一并修复）：

- `console_full_height` 末项写成 `12.0 * s.clamp(CONSOLE_MIN_H * s, CONSOLE_MAX_H * s)`：
  `clamp` 的下限恒为 `170·s`，于是这项恒等于 `12 × 170·s ≈ 2040·s` 像素（本意只是 12·s 的
  底部留白）。面板因此被撑到几乎满屏——内容只占上部约 680·s，下方是一大片空白。这也放大了
  第 1 条问题：展开时内容很早就全部露出，剩下的行程全在长一段空白，观感上「框一直往下长」。
- `ConsoleResize` 反解「完全展开高度」仍用胶囊时代公式 `pill_h + (rect.3 - pill_h) / panel`，
  与当前几何 `h = full_h * panel` 不符（胶囊形态早已移除）。

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

- **G1 面板级裁切**：展开/收起全程，任何内容都不得绘制到面板矩形之外，形成真正的
  「卷帘揭示」（reveal）：顶边锚定、底边下移、内容自上而下露出。
- **G2 透明度与高度解耦**：淡入只在开场极短一段完成，其后只有高度在动；淡出只在收起
  末段发生。消除双重衰减，也消除「高度动、透明度卡住」的通道错位。
- **G3 收起单调收敛**：收起补间的 `panel` 严格由 1 单调收敛到 0，不越过 0、不被 clamp
  提前归零，动画时长内面板始终可见，尾段与淡出同步结束。
- **G4 过冲收敛且仅用于展开**：过冲幅度由约 10% 降到约 5%，且只有展开方向使用；收起使用
  单调缓动，杜绝负进度。
- **G5 顺带修复**：`console_full_height` 底部留白笔误、`ConsoleResize` 反解公式。

### Non-Goals

- 不改面板位置、尺寸与持久化语义（`console_pos` / `console_size` 契约不变）。
- 不改 Z 序提权逻辑（见 `05-console-temporary-topmost.md`）。
- 不改动画驱动方式：仍由 overlay 16ms `ANIM_TIMER` 发 `AnimTick` 推进，仍遵守空闲归零律，
  不引入 DirectComposition 关键帧动画、不引入常驻渲染循环。
- 不改命中模型的 `panel >= 0.5` 门控与 `hover_zone` 门控语义。
- 不把动画推进下沉到 `core`（`core::animation` 已有 `Tween`/`Ease`，app 层补间尚未迁移，
  本次不做跨层搬迁，避免扩大改动面）。
- 不引入胶囊形态、不改变 `panel <= 0.01` 时完全不渲染的既有语义。

## 2. 状态契约与接口设计 (Contracts & Data Models)

### app 层补间（`crates/app/src/anim.rs`）

```rust
/// 面板展开/折叠缓动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelEase {
    /// 收起/回位：单调收敛，绝不过冲（避免 panel 越过 0 被 clamp 提前归零）。
    CubicOut,
    /// 展开：轻微过冲回弹（峰值约 1.05）。
    BackOutSoft,
}

pub(crate) struct PanelTween {
    pub(crate) t0: Instant,
    pub(crate) dur: f32,
    pub(crate) from: f32,
    pub(crate) to: f32,
    pub(crate) ease: PanelEase,   // 新增
}

/// ease_out_back 的克制版：C1 = 1.2，峰值约 1.053（标准版 C1=1.70158 峰值约 1.10）。
pub(crate) fn ease_out_back_soft(t: f32) -> f32;
```

### 常量与派生规则（`crates/app/src/main.rs` / `scene.rs`）

| 常量 | 值 | 语义 |
|---|---|---|
| `CONSOLE_TWEEN_OPEN_S` | 0.24 | 展开时长（秒），`BackOutSoft` |
| `CONSOLE_TWEEN_CLOSE_S` | 0.20 | 收起时长（秒），`CubicOut` |
| `CONSOLE_FADE_SPAN` | 0.40 | 淡入/淡出占用的进度区间 |
| `CONSOLE_OVERSHOOT_MAX` | 1.08 | 高度过冲上限（几何钳制，防止瞬时过高） |

派生规则（**纯函数，无新状态**）：

```
几何高度   h    = full_h * panel.clamp(0.0, CONSOLE_OVERSHOOT_MAX)
不透明度   fade = (panel / CONSOLE_FADE_SPAN).clamp(0.0, 1.0)
```

- 展开：`panel` 到 0.40 之前即完成淡入（约 17ms），其后为纯高度揭示 + 末尾 5% 回弹；
  过冲段 `panel > 1` 时 `fade` 饱和为 1，不再出现「高度回弹、透明度已卡住」的错位
  （此时本就该是实心面板）。
- 收起：`panel` 由 1 单调下降，到 0.40 之前保持不透明，末段与高度一起淡出到 0；
  因为 `CubicOut` 无过冲，`panel` 恒 ≥ 0，「消失」这一事件与补间结束同帧发生。

### render 层场景结构（`crates/render/src/scene.rs`）

```rust
pub struct SceneConsole {
    // ...既有字段不变...
    /// 面板展开进度 0..1（几何用，可 >1 表示过冲回弹中）。
    pub panel: f32,
    /// 面板整体不透明度 0..1（由 `panel` 派生，与高度解耦）。
    pub fade: f32,
}
```

单位口径：全部为**物理像素**（已乘 `theme.scale`）；时长为**秒**；时间基准为 `Instant`（单调），
与既有补间一致。`fade` 为**派生量**（不参与持久化、不参与命中判定），所有权仍在 App 层。

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

- **常驻定时器**：`ANIM_TIMER` 16ms，仅 `arm_anim_timer` 在有补间时启用，`advance_anim`
  返回 false 即 `set_anim_active(false)`。本次只给 `PanelTween` 加 `ease` 字段，启停判据
  （`panel_tween.is_some()`）不变 —— 定时器生命周期不受影响。
- **空闲性能归零**：`fade` 是纯派生、不新增补间，不产生额外帧；补间结束仍立即归零 CPU。
- **重绘与状态回流**：`AnimTick` 每帧重建 `Scene`，`fade` 随 `panel` 同步写入，不存在
  「旁路缓存滞后一帧」的对齐问题（旁路数据对齐律）。
- **缩放/拖动在动画中发生**：`ConsoleResize` 反解改用 `full_h = rect.3 / panel`（`panel`
  已 `max(0.05)` 防除零）；`panel == 1.0` 时与旧公式等价，故稳定态行为不变，仅动画中
  缩放的结果变正确。
- **D2D 状态栈**：新增 `PushAxisAlignedClip` / `PopAxisAlignedClip` 必须**严格配对**，
  包括 `?` 提前返回分支 —— 故内容绘制抽成独立函数，由外层统一 Pop，防止 clip 残留
  污染后续帧（尤其 `draw_inline_edit` 与下一帧的栅栏绘制）。
- **嵌套 clip**：`draw_fences_page` 内部已有列表可视区的 push/pop，D2D 允许栈式嵌套，
  本次在其外层再加一层，配对关系不变。
- **命中模型**：`panel` 语义（0..1，可 >1）不变，`panel >= 0.5` 的悬停/命中门控不变；
  `fade` 不参与命中（半透明不构成不可点）。

## 4. 分层改动清单 (Implementation Steps)

按依赖自底向上：`render`（场景数据结构）→ `app`（状态推进与派生）→ `render`（绘制消费）。

1. **`crates/render/src/scene.rs`**：`SceneConsole` 新增 `fade: f32`；更新 `panel` 字段注释
   （几何进度，可 >1）。
2. **`crates/app/src/anim.rs`**：
   - 新增 `PanelEase` 枚举与 `ease_out_back_soft`；
   - `PanelTween` 增加 `ease` 字段；
   - `advance_anim` 按 `ease` 分支取缓动值；
   - `start_panel_tween` 按目标方向选缓动与时长（展开 `BackOutSoft`/0.24s，
     收起 `CubicOut`/0.20s）；
   - 修正顶部文档注释（`ease_out_cubic` → 实际缓动）。
3. **`crates/app/src/scene.rs`**：
   - 新增常量 `CONSOLE_FADE_SPAN`、`CONSOLE_OVERSHOOT_MAX`（与既有常量同处）；
   - `console_geometry` 的高度钳制上限由字面量 `1.12` 改为 `CONSOLE_OVERSHOOT_MAX`；
   - `build_console` 写入 `fade: (anim.panel / CONSOLE_FADE_SPAN).clamp(0.0, 1.0)`；
   - 修复 `console_full_height` 末项 `12.0 * s.clamp(...)` → `12.0 * s`（底部留白笔误）；
   - 修正「折叠胶囊」相关过时注释（胶囊形态已不存在，进度 0 = 完全不渲染）。
4. **`crates/app/src/main.rs`**：
   - 新增 `CONSOLE_TWEEN_OPEN_S` / `CONSOLE_TWEEN_CLOSE_S` 常量（或置于 `anim.rs`）；
   - `ConsoleResize` 反解公式改为 `rect.3 / panel`，并更新注释。
5. **`crates/render/src/draw.rs`**：
   - `draw_console` 拆分为「背景 + 描边」与 `draw_console_content`（标题/关闭按钮/管理页）；
   - 内容绘制整体包在面板矩形的 `PushAxisAlignedClip` 内，**错误路径也保证 Pop**；
   - 透明度改用 `c.fade`（不再用 `c.panel`）；
   - 更新函数文档注释（说明 reveal 语义与 clip 契约）。
6. **单测（`crates/app/src/anim.rs` 内 `#[cfg(test)]`）**：缓动端点、峰值上界、收起单调性、
   `fade` 派生边界。
7. **独立审查后补充**（对抗性审查发现，已一并实施）：
   - `console_geometry`：可用高度改为 `avail / CONSOLE_OVERSHOOT_MAX` —— 否则内容被夹到
     `max_h` 时，过冲高峰会把面板底边推出屏幕下沿（面板本体不参与内容裁切）。
   - `console_geometry`：`auto_full_h` 补 `.min(CONSOLE_MAX_H * s)`，恢复该常量的实际约束语义。
   - `ConsoleResize`：反解值 `clamp(CONSOLE_MIN_H * s, CONSOLE_MAX_H * s)` —— 动画中途缩放
     （`panel` 可低至 0.05）会把展开高度放大数十倍并**持久化**进 `console_size`。
   - `start_panel_tween`：已在目标态（`|to - from| <= 1e-3`）时直接落定，不建空转补间、
     不唤醒定时器（重复唤出不再白跑约 12 帧）。
   - `draw.rs`：列表可视区的内层 clip 同样抽成 `draw_fence_rows` 并由调用方统一 Pop，
     修补既有「`?` 提前返回导致 clip 残留」缺陷。
   - **未采纳**：审查提出「启动恢复 `console_open=true` 时未调用 `raise_console()`」——
     这是 `05-console-temporary-topmost.md` §3 的既定语义（提权限定为「本会话用户显式唤出」），
     不是缺陷，本次不动。
8. **第二轮审查后的收尾修正**：
   - `console_geometry`：`max_h` 补 `.max(CONSOLE_MIN_H * s)` 兜底，且手动尺寸分支改用
     `h.clamp(MIN_H * s, max_h)` —— 原 `h.max(MIN_H).min(max_h)` 在 `max_h < MIN_H`
     （小屏且 `avail` 被最小值兜底）时会把面板压到最小高以下。
   - `ConsoleResize`：补间进行中按满进度（`panel = 1.0`）反解，避免同一段拖动随 `panel`
     逐帧变化算出不同展开高度并持久化。
   - `overlay.rs` 修正仍写「折成胶囊」的过时注释（收起即完全不渲染）。

## 5. 防御性自查清单 (Defensive Invariants)

| 律 | 校验与针对性防御 |
| :---: | --- |
| 1 常驻后台冲突 | 定时器启停判据不变；`fade` 非独立补间，不会造成「补间已结束但定时器仍开」或反之。 |
| 2 空闲性能归零 | 不新增轮询、不新增补间；补间结束仍 `set_anim_active(false)`，动画后 0% CPU。 |
| 3 孤儿状态回收 | 不涉及外部实体/路径，无孤儿风险。 |
| 4 语义精准定位 | 缓动选择按「目标方向」（`to > from`）语义判定，不按下标或位置假设。 |
| 5 状态全集校验 | 收起判定用「方向 + 单调缓动」复合保证，而非单看 `panel` 数值；`panel` 与 `console_open` 仍由 `set_console_open` 单一出口同步。 |
| 6 旁路数据对齐 | `fade` 与 `height` 同帧由同一 `panel` 派生写入 `Scene`，无跨帧滞后；`SceneConsole` 新增字段在唯一构造点（`build_console`）赋值。 |

补充防御：

- clip 矩形与面板矩形同源（同一 `c.x/c.y/c.width/c.height`），不另算，杜绝几何漂移。
- `panel` 为负或 NaN 的兜底：`fade` 与高度均经 `clamp`，NaN 时 `clamp` 返回 NaN —— 故
  `advance_anim` 的收起分支改用无过冲缓动，从源头排除负值；`tween_progress` 保证
  `dur > 0`（常量固定 0.20/0.24，非运行时输入）。
- `draw_console` 早退分支（`fade <= 0.01`）发生在 push clip **之前**，不会产生未配对的栈。

## 6. 验证与交付门禁 (Verification Gates)

自动化：

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

新增用例矩阵（`crates/app/src/anim.rs`）：

1. `ease_out_back_soft`：端点 `f(0)=0`、`f(1)=1`；峰值 ≤ 1.06（克制过冲）；全程 ≥ 0。
2. 收起补间：以 `CubicOut` 逐帧推进，`panel` 单调不增且恒 ∈ [0, 1]，不出现负值（回归
   原「提前归零」缺陷）。
3. `fade` 派生：`panel <= 0 → 0`；`panel >= CONSOLE_FADE_SPAN → 1`；过冲 `panel = 1.05 → 1`。
4. `console_full_height` 回归：面板高度 = 内容底部 + `12 * scale`（断言不再多出 170·s）。

人工走查（改动观感，必做）：

- `Ctrl+Alt+T` 连续展开/收起 5 次：内容不再溢出边框；展开末段回弹幅度明显变小；
  收起不再「唰地消失」，而是高度收回 + 尾段淡出。
- 动画进行中拖动面板标题栏、拖右下角缩放：无闪烁、无高度突变。
- 展开动画期间点击面板外桌面：不穿透到桌面图标（既有 `last_press_in_console` 语义不变）。

独立上下文子代理审查要点：D2D clip 配对与错误路径、补间状态机与定时器生命周期、
`fade`/`panel` 派生一致性、`ConsoleResize` 反解在 `panel → 0` 时的数值稳定性。
