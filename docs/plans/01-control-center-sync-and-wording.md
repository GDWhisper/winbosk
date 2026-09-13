# 控制中心与桌面栅栏点击联动及文案优化实施计划

本实施计划覆盖 `docs/TODO.md` 中的 **Task #1（控制中心与桌面栅栏点击联动）** 与 **Task #2（控制中心【切换桌面】更名为【恢复桌面】）**。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals
1. **控制中心实时精准跟随**：当控制中心处于展开状态（`rt.desk.console_open == true`）时，用户在桌面上点击、拖动、框选或右键操作任意栅栏，控制中心顶部栅栏列表中对应项自动高亮显示，且列表自动平滑/对齐滚动以保证选中的栅栏行始终处于可视区域内；下方详情面板同步展示该栅栏的配置。
2. **术语心智一致性**：将控制中心标题栏上的「切换桌面」按钮文案统一更名为「恢复桌面」（处于桌面模式时继续保持为「回到栅栏」），消除 Windows 虚拟桌面误解，准确表达“暂时恢复原生桌面图标”的操作意图。

### Non-Goals
1. 不变动控制中心的单页架构与 DirectComposition 视觉树渲染模式；
2. 不在控制中心关闭时触发任何非必要的滚动或重绘计算；
3. 不增加跨线程通信与第三方 UI 状态库。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 视口滚动对齐函数（纯计算）
在 [`crates/app/src/scene.rs`](file:///g:/Codes/winbosk/crates/app/src/scene.rs) 中实现视口对齐计算：
```rust
/// 确保当前选中的栅栏在控制中心列表中完全可见
pub(crate) fn ensure_selected_fence_visible(rt: &mut Runtime) {
    let s = rt.theme.scale;
    let row_h_f = CONSOLE_FENCE_ROW_H * s;
    let fence_n = rt.desk.fences.len();
    let fence_shown = fence_n.min(CONSOLE_FENCE_MAX_ROWS);
    if fence_n <= fence_shown {
        rt.fence_scroll = 0.0;
        return;
    }
    let fence_scroll_max = (fence_n - fence_shown) as f32 * row_h_f;
    let sel = rt.selected_fence.min(fence_n.saturating_sub(1));
    let row_top = sel as f32 * row_h_f;
    let row_bottom = (sel + 1) as f32 * row_h_f;
    let view_top = rt.fence_scroll;
    let view_bottom = rt.fence_scroll + fence_shown as f32 * row_h_f;

    if row_top < view_top {
        rt.fence_scroll = row_top;
    } else if row_bottom > view_bottom {
        rt.fence_scroll = row_bottom - fence_shown as f32 * row_h_f;
    }
    rt.fence_scroll = rt.fence_scroll.clamp(0.0, fence_scroll_max);
}
```

### 2.2 控制中心文案契约
在 [`crates/render/src/draw.rs`](file:///g:/Codes/winbosk/crates/render/src/draw.rs) 中：
```rust
let toggle_label = if c.desktop_mode {
    "回到栅栏"
} else {
    "恢复桌面"
};
```

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

1. **空闲性能归零律 (Zero-Idle Invariant)**：
   - 栅栏点击联动仅在用户真实产生鼠标交互且 `rt.desk.console_open == true` 时生效；
   - 仅当 `rt.selected_fence != fence` 时才触发滚动位置计算并标记 `redraw = true`；
   - 无操作时消息循环完全阻塞在 `GetMessageW`，保持 0% CPU。
2. **旁路数据对齐律 (Sidecar Invariant)**：
   - `rt.selected_fence` 必须始终保持在 `0..desk.fences.len()` 的合法索引边界内（通过 `min(fence_n.saturating_sub(1))` 进行防御式保护）。
3. **交互无摩擦律**：
   - 用户在桌面整理栅栏时，无需手动到控制中心翻找栅栏名，随手点击栅栏即可立刻在控制中心内调参（图标尺寸、背景风格、色调等）。

---

## 4. 分层改动清单 (Implementation Steps)

### 一、渲染层（`crates/render/`）
- **[`crates/render/src/draw.rs`](file:///g:/Codes/winbosk/crates/render/src/draw.rs)**：
  - 将 `draw_console` 中按钮标签 `"切换桌面"` 改为 `"恢复桌面"`；
- **[`crates/render/src/scene.rs`](file:///g:/Codes/winbosk/crates/render/src/scene.rs)**：
  - 更新 `SceneConsole.desktop_toggle` 字段注释。

### 二、应用组装层（`crates/app/`）
- **[`crates/app/src/scene.rs`](file:///g:/Codes/winbosk/crates/app/src/scene.rs)**：
  - 新增 `ensure_selected_fence_visible` 纯函数；
  - 更新 `build_console` 中的注释。
- **[`crates/app/src/main.rs`](file:///g:/Codes/winbosk/crates/app/src/main.rs)**：
  - 在 `handle_event` 中建立统一联动拦截点：
    对于携带 `fence: usize` 的交互事件（`FenceMove`、`FenceResize`、`FenceDragEnd`、`IconClicked`、`IconDoubleClicked`、`SelectDrag`、`ContextMenu`）：
    ```rust
    if rt.desk.console_open && fence < rt.desk.fences.len() && rt.selected_fence != fence {
        rt.selected_fence = fence;
        ensure_selected_fence_visible(rt);
        redraw = true;
    }
    ```

---

## 5. 防御性自查清单 (Defensive Invariants)

- [x] **语义精准定位律**：`fence` 索引来自当前有效 `HitModel`，更新前校验 `fence < rt.desk.fences.len()`，杜绝越界 panic。
- [x] **空闲性能归零律**：联动不引入常驻定时器；只有选中索引发生真实变更时才置 `redraw = true`。
- [x] **状态全集校验律**：只有当 `rt.desk.console_open == true` 时才执行联动，控制中心关闭时桌面操作完全不触发滚动计算与多余属性更新。

---

## 6. 验证与交付门禁 (Verification Gates)

1. **编译与静态分析**：
   ```powershell
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```
2. **测试用例**：
   - 编写 `ensure_selected_fence_visible` 的单元测试：分别测试第一行、最后一行、中间行被选中时的 `fence_scroll` 计算。
3. **真实走查验证**：
   - 按 `Ctrl+Alt+T` 打开控制中心；
   - 检查标题栏按钮是否显示为「恢复桌面」；点击后是否切换为「回到栅栏」且桌面图标恢复显示；
   - 在桌面上依次单击/拖动不同栅栏（包含超出 5 行需要滚动的栅栏），验证控制中心顶部栅栏列表中对应项是否同步高亮，并自动滚动入视口；下方详情是否同步更新。
