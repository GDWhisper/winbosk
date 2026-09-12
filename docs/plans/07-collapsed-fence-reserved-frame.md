# 收起栅栏「原大小占位框」实施计划

**缘起**：折叠只是视觉收缩（`build_scene` 把 `SceneFence.height` 改成 `title_h`），模型矩形 `bounds` 与碰撞口径完全不变。于是拖动收起后的窄条时，它仍按展开时的原矩形被屏幕下沿和邻居挡住，观感是「撞空气墙」；而那块区域当前既不渲染也不接收点击（`docs/TODO.md:129` 记录的"原内容区域自动穿透"），用户看不到任何解释。

本计划把**已有的预留约束可视化**，而不是解除约束。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

1. **拖动期间可视化原尺寸占位**：
   - 被拖动的栅栏若处于收起态 → 始终显示它自己的占位框；
   - 拖动中**拒绝了本次请求位置**的其它收起栅栏 → 显示其占位框（回答"是谁挡住了我"）。
2. **瞬态且可逆**：松手当帧撤除占位框，折叠态 100% 点击穿透语义立即恢复，不留任何残留。
3. **几何同源**：占位框必须与碰撞算法取自同一矩形（`fence_collision_rect`，`main.rs:639`）；并顺带修正拖动自身的口径（`bounds.h` → `fence_height`），使「画出来的框」与「实际阻挡」严格一致。
4. **视觉规约**：1px 虚线圆角描边 + 极淡填充，颜色从该栅栏自身配色派生；无动画、无新增常驻定时器。

### Non-Goals

1. **不解除原尺寸预留语义**，不引入"展开瞬间把邻居推开"的另一套动力学（那是独立议题，本计划不做）。
2. **不做动画**（不做淡入淡出）——避免为拖动结束后的余韵新开一条 `AnimTick` 生命周期。
3. **不为展开态栅栏绘制占位框**（展开态视觉 = 模型，不存在理解成本）。
4. **不改变折叠态的穿透语义**：除"拖动期间的必要并入"（§3.1）外，其余时间保持完全穿透。
5. **不新增配置项、不写盘**：占位框是纯瞬态显示，不进入 `desk.json`，不参与任何求解。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 纯算法层（`crates/core/src/magnet.rs`）

```rust
/// 候选矩形是否被 `other` 拒绝（两者间距不足 `gap`）——即该位置不会被采纳。
/// 判据与 `settle_move` 内部的 `gap_separated` 完全一致，保证「提示」与「实际阻挡」不脱节。
pub fn blocks_move(cand: &Rect, other: &Rect, gap: f32) -> bool;
```

只加这一个公开判据；`settle_move` / `settle_resize` 的行为与签名不变。

### 2.2 应用层瞬态状态（`crates/app/src/main.rs`）

```rust
/// 拖动中的占位提示状态（瞬态，不持久化，`FenceDragEnd` 或任何非拖动事件即清空）。
pub(crate) struct DragHint {
    /// 被拖动的栅栏下标（`desk.fences`）。
    pub fence: usize,
    /// 本帧**未经 settle** 的请求矩形（鼠标原始目标）：用于判定哪些收起栅栏拒绝了它。
    pub requested: Rect,
}

// Runtime 新增字段：
pub(crate) drag_hint: Option<DragHint>,
```

### 2.3 渲染场景契约（`crates/render/src/scene.rs`）

```rust
/// 收起栅栏的「原大小占位框」（瞬态，仅拖动期间存在）。
#[derive(Debug, Clone)]
pub struct SceneReserved {
    /// 占位矩形（物理像素，虚拟屏幕坐标）= 该栅栏的碰撞矩形。
    pub rect: RectF,
    /// 来源栅栏下标（`desk.fences`），用于日志与断言。
    pub fence: usize,
    /// 虚线描边色（直通 alpha，已含占位透明度与桌面切换淡出）。
    pub stroke_color: [f32; 4],
    /// 极淡填充色（直通 alpha）；None = 只描边不填充。
    pub fill_color: Option<[f32; 4]>,
}

// Scene 新增字段：
pub reserved: Vec<SceneReserved>,
```

### 2.4 命中模型契约（`crates/render/src/overlay.rs`）

```rust
pub struct HitModel {
    pub fences: Vec<FenceHit>,
    pub icons: Vec<IconHit>,
    pub console: Option<ConsoleHit>,
    pub edit_rect: Option<RectF>,
    /// 占位框矩形：**只并入窗口区域（`SetWindowRgn`），绝不参与命中判定**。
    pub reserved: Vec<RectF>,
}
```

- `reserved` 不进入 `fences`，故 `fence_at` / `hover_key` / `collapse_target_at` / `resize_zone_at` 一律看不见它，不会产生"看不见却能点"的幽灵热区。
- 单位口径：全部为**虚拟屏幕物理像素**，与 `bounds` / `last_layout_h` 同源；DIP 缩放已由 `Theme.scale` 在几何计算内消化，占位框**不再二次缩放**。

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

### 3.1 窗口区域（RGN）——本方案成立的前提，不是可选优化

`build_region`（`overlay.rs:962`）只并入 `fences[].body` / 工具提示 / 编辑框 / 控制台面板；**区域外既不渲染也不接收事件**（`overlay.rs:933-935` 注释）。占位框天然大于收起后的标题栏 → 必须并入 RGN，否则根本画不出来。

副作用与论证：并入期间该区域不再点击穿透。**可接受**，因为拖动期间鼠标已被 `SetCapture` 捕获，不会产生桌面点击；`FenceDragEnd` 当帧（`redraw = true`）重建区域即恢复穿透。

**泄漏风险（必须设防）**：若拖动异常终止（capture 被系统抢占、模态弹窗打断、DPI 变化重建窗口）导致 `FenceDragEnd` 未送达，占位框会滞留并持续吞掉其覆盖区域的桌面点击 → 由 §4.3 的**白名单式清理**兜底。

### 3.2 合成表面（`content_rect`）——易漏的第二道裁剪

合成器按内容包围盒裁剪合成表面（`compositor.rs:171` → `Scene::content_rect`）。占位框若未并入该包围盒，会被表面切掉，表现为"框只有部分方向可见"或整体消失（且与 RGN 问题症状相似、极易误判）。故 `content_rect` 必须同步并入 `reserved`。

### 3.3 空闲性能归零律

占位框只在 `drag_hint.is_some()` 的帧存在；无定时器、无补间。拖动结束回到纯阻塞 `GetMessageW`，空闲 0% CPU 不受影响。

### 3.4 区域抖动

`apply_region` 已有 `EqualRgn` 缓存（`overlay.rs:943-957`）。拖动本身每帧都在改变被拖栅栏矩形（区域已在变化），新增占位框**不增加** `SetWindowRgn` 调用次数，不会引入新的重绘抖动。

### 3.5 旁路数据对齐律 & 语义精准定位律

- `drag_hint` 持有栅栏**下标**：取用前一律 `fences.get(i)` 校验，越界即视为"无占位框"（控制台删除栅栏 / 外部布局变更都会改变长度），**禁止** `[]` 索引直取。
- 几何一律走 `fence_collision_rect(rt, i)` 单一真源；绘制层禁止自行推算占位矩形。

### 3.6 状态全集校验律

绘制条件必须是复合状态：`collapsed && layout != Sidebar && rect.w > 0 && rect.h > 可见高度 + 阈值`。
只看 `collapsed` 会在侧边栏（不支持折叠，`collapsed` 恒 false 但几何语义不同）与"占位 ≈ 可见"时画出退化或毫无信息量的框。

### 3.7 与既有后台机制的交界

- **启动重叠消解**（`resolve_overlaps`）与 **DPI/显示变化重排**（`reanchor_fences`）复用同一碰撞口径，本方案只做显示，不参与求解 → 无新副作用。
- **`SyncLibrary` 定时器**：占位框不消费它；定时器触发时若正处于拖动中，占位框按当帧 `drag_hint` 重算，不产生额外状态。
- **控制中心**：控制台打开时不改变本逻辑；拖动栅栏会联动选中栅栏（既有行为），占位框随之重算。

---

## 4. 分层改动清单 (Implementation Steps)

按依赖拓扑自底向上。

### 4.1 纯算法层（`crates/core/src/magnet.rs`）

- 新增 `pub fn blocks_move(cand: &Rect, other: &Rect, gap: f32) -> bool`（内部即 `!gap_separated`）。
- 补单测：间距不足 → `true`；任一轴净空隙 ≥ `gap` → `false`；零面积/退化矩形；贴屏边界时与 `settle_move` 结论一致性（"判为阻挡的位置，settle 后位置确实被改变"）。

### 4.2 渲染层（`crates/render/`）

- **`scene.rs`**
  - 新增 `SceneReserved`；`Scene` 增加 `pub reserved: Vec<SceneReserved>`；`Scene::new` 初始化为空；
  - `Scene::content_rect()`（`scene.rs:284`）并入全部 `reserved` 矩形（与 `tooltip_rect` 同样处理）；
  - 新增测试 `content_rect_covers_reserved`。
- **`overlay.rs`**
  - `HitModel` 增加 `reserved: Vec<RectF>`；
  - `build_region`（`overlay.rs:962`）逐项 `add_rect`（零尺寸由 `add_rect` 自身跳过）；
  - 测试内的 `HitModel { .. }` 构造点补 `reserved: Vec::new()`；
  - 新增测试 `reserved_rect_joins_region_but_not_hit`：占位矩形并入区域，且 `fence_at`/`hover_key` 在其内返回 `None`。
- **`draw.rs`**
  - 新增 `fn draw_reserved(target, theme, r: &SceneReserved) -> Result<()>`：
    - 填充：`D2D1_ROUNDED_RECT`（半径 `theme.fence_corner_radius`）先以极淡 `fill_color` 填充；
    - 描边：同形状走虚线 `ID2D1StrokeStyle`；
    - 虚线实现优先级：① 在合成器初始化时用 `device.rs` 的 `ID2D1Factory` 创建一次 `ID2D1StrokeStyle`（`D2D1_STROKE_STYLE_PROPERTIES { dashStyle: D2D1_DASH_STYLE_DASH, dashCap: D2D1_DASH_CAP_FLAT, .. }`）并随 `TextFormats` 一起持有（设备无关资源）；② 绑定受限时退化为每帧 `CreateStrokeStyle`（仅拖动帧发生，开销可忽略）；③ 再不行则手绘短线段（零新 API）。
  - `draw_scene`（`draw.rs:138`）在**栅栏循环之前**绘制全部占位框，保证其位于所有栅栏之下。
  - 配色（实现定稿）：描边 = 与控制台 UI 同一强调色 `[0.23, 0.51, 0.96]`，alpha `0.55 × 场景 alpha`；填充 = 同色 alpha `0.07 × 场景 alpha`。
    **不采用**栅栏自身的 `border_color`：它随系统明暗主题在黑白之间切换（深色=白 42% / 浅色=黑 45%），在深色壁纸上会整个消失；蓝色虚线在明暗壁纸上都读得出"这是 UI 提示"而非真实栅栏边。

### 4.3 应用层（`crates/app/`）

- **`main.rs`**
  1. `Runtime` 新增 `drag_hint: Option<DragHint>`；
  2. `OverlayEvent::FenceMove` 分支（`main.rs:826-848`）：
     - **统一拖动口径**：`let h = fence_height(rt, fence);` 取代 `f.bounds.h`（`main.rs:831`）。自动高度栅栏（`bounds.h <= 0`）从此按真实展开高度参与夹屏与避让——当前用 0 会让自身矩形退化为零高（漏检邻居/可拖出屏幕），属口径修正；
     - `rt.drag_hint = Some(DragHint { fence, requested: cand })`，`cand` 用**未 settle 的请求矩形**（settle 前的鼠标原始目标），这样"被拒绝"的判定才对应真实的空气墙；
  3. **白名单式清理**（放在 `handle_event` 开头统一处理，而非散落在各分支）：

     ```rust
     let mut reserved_changed = false;
     if rt.drag_hint.is_some() && !(keeps_drag_hint(&ev) && left_button_down()) {
         rt.drag_hint = None;
         reserved_changed = true;   // 末尾据此强制重绘，让窗口区域收缩
     }
     ```

     - `keeps_drag_hint` 是独立纯函数（便于单测），白名单 = `FenceMove` / `FenceResize` / `FenceScroll` / `KeyDown` / `AnimTick` / `SyncLibrary` / `HoverEnter` / `HoverLeave` / `CursorMove` / `CursorLeave` / `ConsoleHover`。拖动进行中 overlay 只会上报这些事件（见 `overlay.rs:1797-1943`）；任何异常路径（capture 丢失、右键菜单、托盘、控制台操作、DPI/显示变化）都会立刻清空，杜绝 §3.1 的"占位框滞留吞点击"。`FenceDragEnd` 落在"其余事件"里被自然清理。
     - **`FenceMove` 分支写入提示时也必须带 `left_button_down()` 守卫**（对抗性审查发现的 P0）：capture 被抢占后 overlay 的拖动状态可能残留，鼠标在窗口上移动仍会上报 `FenceMove`（此时左键早已松开）；若该分支无条件重建提示，就会"入口处刚清、分支里又建"，占位框与扩张的窗口区域双双滞留。
     - 新增事件变体默认落入"清理"一侧（安全默认值），并由 `drag_hint_whitelist_covers_only_drag_events` 单测固定住这份预期。

- **`scene.rs`**
  - 新增纯函数（不触碰 `Runtime`/Win32，便于单测）：

    ```rust
    pub(crate) fn reserved_frames(
        desk: &Desk,
        last_layout_h: &[f32],
        hint: Option<&DragHint>,
        theme: &Theme,
        alpha: f32,
    ) -> Vec<SceneReserved>
    ```

    逻辑：
    1. `hint` 的 `fence` 存在且 `collapsed` → 加入其自身占位框；
    2. 遍历其它栅栏 `j`：`collapsed` 且 `magnet::blocks_move(&hint.requested, &collision_rect(j), FENCE_GAP)` → 加入（谁挡住了我）；
    3. 全部经 §3.6 的复合状态过滤，空集合返回空 `Vec`。
  - `build_scene` 末尾写入 `scene.reserved`（`alpha` 与栅栏共用桌面切换淡出值）；
  - `hit_model_from`（`app/src/scene.rs:1486`）把 `scene.reserved[].rect` 映射进 `HitModel.reserved`。

### 4.4 文档同步

- `AGENTS.md` 架构约束补一条（编号顺延）：**折叠态占位框只在拖动期间并入窗口区域，且必须白名单式清理**——记录"区域外不可见"与"滞留会吞桌面点击"这两个反常点。
- `docs/TODO.md` 的收展功能条目补注：折叠态原内容区域默认穿透，**仅当拖动收展栅栏或拖动中被其阻挡时**临时并入窗口区域显示占位框。

---

## 5. 防御性自查清单 (Defensive Invariants)

| 序号 | 校验律 | 本方案的针对性设计 |
| :---: | :--- | :--- |
| 1 | 常驻后台冲突律 | 占位框不消费 `SyncLibrary`/`AnimTick` 的任何状态，纯由 `drag_hint` 派生；无新增后台循环。 |
| 2 | 空闲性能归零律 | 仅拖动帧存在；无定时器、无补间；拖动结束即回到纯阻塞 `GetMessageW`。 |
| 3 | 孤儿状态回收律 | `drag_hint` 是纯瞬态且**白名单式清理**：任何非拖动事件、capture 丢失、窗口重建都会清空，不留"吞点击"的孤儿区域。 |
| 4 | 语义精准定位律 | 几何一律 `fence_collision_rect` 单一真源；`drag_hint.fence` 取用前 `get()` 校验，越界视为无占位框。 |
| 5 | 状态全集校验律 | 绘制条件为 `collapsed && !Sidebar && 尺寸有效 && 占位显著大于可见高度` 的复合判定，禁止只看 `collapsed`。 |
| 6 | 旁路数据对齐律 | `Scene.reserved` 与 `HitModel.reserved` 均由 `scene` 单帧同步派生；`content_rect` 同帧并入，杜绝"区域/表面/命中"三处口径错帧。 |

附加防线：

- **RGN 与表面双裁剪**：RGN（§3.1）与 `content_rect`（§3.2）必须**同时**并入，遗漏任一处都会表现为"框看不见"，且症状相似——审查时须分别验证。
- **零尺寸与退化矩形**：`w/h <= 0` 一律跳过（`add_rect` 亦自带跳过），避免幽灵热区。
- **桌面切换 alpha**：占位框乘 `alpha`，与栅栏同帧淡出，不出现"栅栏没了框还在"。
- **侧边栏**：Sidebar 不支持折叠，不进入占位框逻辑。
- **不写盘**：占位框不得触发 `store.save`。

---

## 6. 验证与交付门禁 (Verification Gates)

### 6.1 自动化

```powershell
cargo test -p sylva-core                     # blocks_move 判定表
cargo test -p sylva-render                   # content_rect / region / 命中不受影响
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

用例矩阵（新增）：

| 场景 | 期望 |
| :--- | :--- |
| 空集合 / 无拖动 | `reserved_frames` 返回空 `Vec`，不 panic、不产生任何 I/O |
| 拖动收起栅栏，无遮挡 | 仅 1 个占位框（自身），矩形 = 该栅栏碰撞矩形 |
| 拖动展开栅栏，压到某收起栅栏 | 出现对方占位框；离开该区域后消失 |
| 连续两次相同拖动（幂等） | 第二次拖动结束后的场景与第一次完全一致（无累积状态） |
| 占位矩形并入后 | `fence_at`/`hover_key` 在其内返回 `None`（不产生命中） |
| `drag_hint` 下标越界 | 返回空，不 panic |
| 白名单漂移（新增事件变体） | `drag_hint_whitelist_covers_only_drag_events` 失败，强制作者显式决策 |

本地跑 `sylva-app` 相关门禁时，若 `build.rs` 的 winres 被环境拦截，按项目既有做法临时门控跳过（跑完立即还原）。

### 6.2 真实走查

1. 拖动一个收起栅栏 → 占位框跟随，位置/大小与该栅栏展开时的几何一致；
2. 拖动某栅栏去压一个收起栅栏 → 在"推不过去"时对方占位框出现，且出现位置正好解释卡位处；
3. 松手 → 占位框立刻消失；**立即点击原占位框覆盖区域的桌面图标，必须能正常选中**（验证穿透已恢复）；
4. 拖动中途按 Esc / 切换窗口 / 触发托盘菜单 → 占位框不得残留（白名单清理生效）；
5. 桌面切换（栅栏淡出）期间拖动 → 占位框与栅栏同步淡出；
6. 空闲 30 秒 → 任务管理器 CPU 0%，无 `SetWindowRgn` 抖动。

### 6.3 独立子代理对抗性审查要点

- 占位框是否存在"滞留吞点击"的路径（capture 丢失、模态、`WM_KILLFOCUS`、DPI 重建）？
- RGN 与 `content_rect` 是否**都在同一帧**并入/移除，存在错帧窗口吗？
- 拖动自身口径改为 `fence_height` 后，自动高度栅栏的夹屏/避让是否引入越界或抖动？
- `add_rect` 的 `i32` 截断在负坐标/超大虚拟屏下是否安全？
- 是否出现写盘、定时器武装、或空闲重绘？
