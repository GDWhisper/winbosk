# 栅栏内置收起/展开（Collapse/Expand）机制实施计划

本实施计划覆盖 `docs/TODO.md` 中的 **Task #3（栅栏内置收起/展开功能）**。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals
1. **纯净折叠视觉**：栅栏支持收起/展开双态；收起时仅保留标题栏高度并显示栅栏名称，图标列表及内部边距全部隐藏；
2. **物理级点击穿透**：折叠后的栅栏下部原内容区域通过 Win32 `SetWindowRgn` 实时镂空裁剪，鼠标点击完全穿透至底层桌面/其他窗口，无隐形拦截死区；
3. **自适应高度与折叠高度严格解耦**：折叠时严禁修改 `f.bounds.h`（保持展开时的原始高或 `<=` 0.0 的自适应标记），展开时能够无缝还原原始几何；
4. **多通道便捷交互**：
   - **标题栏按钮**：标题栏右侧生成折叠切换按钮（收起态显示 `▸`，展开态显示 `▾`）；
   - **双击标题栏**：双击栅栏标题栏空白区域一键切换收起/展开；
   - **上下文右键菜单**：栅栏右键菜单增加「收起栅栏」/「展开栅栏」菜单项。

### Non-Goals
1. 不对折叠过程施加拖泥带水的缓动动画，折叠与穿透区域更新保证在同一帧内瞬时完成（手感利落）；
2. 折叠状态不影响后台库文件变更对图标集合的维护。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 核心领域模型（`crates/core/src/model.rs`）
在 `Fence` 结构体中新增向后兼容字段：
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fence {
    pub id: u64,
    pub title: Option<String>,
    pub monitor_id: u32,
    pub bounds: Rect,
    pub state: FenceState,
    pub icon_ids: Vec<ItemId>,
    pub appearance: FenceAppearance,
    #[serde(default)]
    pub scroll: f32,
    #[serde(default)]
    pub storage_path: Option<String>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub rule: Option<FenceRule>,
    /// 栅栏是否收起（折叠仅留标题栏）。
    #[serde(default)]
    pub collapsed: bool,
}
```

### 2.2 渲染场景模型（`crates/render/src/scene.rs`）
在 `SceneFence` 中新增折叠指示信息：
```rust
pub struct SceneFence {
    // ...
    /// 栅栏是否折叠。
    pub collapsed: bool,
    /// 标题栏折叠/展开切换按钮矩形（物理像素）。
    pub collapse_btn: Option<RectF>,
}
```

### 2.3 交互事件契约（`crates/render/src/overlay.rs`）
新增折叠交互事件：
```rust
pub enum OverlayEvent {
    // ...
    /// 点击栅栏折叠切换按钮。
    FenceCollapseToggle { fence: usize },
    /// 双击栅栏标题栏空白区。
    FenceTitleDoubleClicked { fence: usize },
}
```

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

1. **自适应高度与《规范》第 8 条约束**：
   - `bounds.h <= 0.0` 栅栏的高度由每帧布局结果 `last_layout_h` 回写；
   - 折叠时**严禁覆盖 `f.bounds.h`**！折叠时的视觉高度 `title_h` 仅在 `build_scene` 中计算：
     ```rust
     let title_h = (theme.title.size * 1.6 + theme.title_padding_bottom + 2.0 * theme.fence_padding).round();
     let fence_h = if f.collapsed { title_h } else { expanded_h };
     ```
   - 展开时栅栏自然读回原 `f.bounds.h`（若为自适应，则按内容重新计算 `last_layout_h`），杜绝高度丢失与几何抖动。
2. **窗口区域剪裁（SetWindowRgn）动力学**：
   - Overlay 窗口采用 `SetWindowRgn` 将窗口边界约束在所有可见栅栏的几何并集；
   - 当某栅栏折叠时，场景中的 `f.body` 高度变为 `title_h`；
   - `build_region` 合并的矩形仅包含标题高度，下部区域即刻脱离窗口 RGN；
   - Win32 消息子系统在下部区域不会派发任何鼠标消息至 overlay，天然穿透到底层桌面。
3. **空闲性能归零律 (Zero-Idle Invariant)**：
   - 折叠与展开在触发帧提交新场景与新 RGN 后，重绘立即结束，进程继续阻塞在 `GetMessageW`，保持 0% CPU。

---

## 4. 分层改动清单 (Implementation Steps)

### 一、领域模型层（`crates/core/`）
- **[`crates/core/src/model.rs`](file:///g:/Codes/winbosk/crates/core/src/model.rs)**：
  - `Fence` 增加 `#[serde(default)] pub collapsed: bool`；
  - 在所有测试与辅助构造函数中补充 `collapsed: false`；
  - 编写单测验证含/不含 `collapsed` 字段的 JSON 反序列化向后兼容性。

### 二、渲染与穿透层（`crates/render/`）
- **[`crates/render/src/scene.rs`](file:///g:/Codes/winbosk/crates/render/src/scene.rs)**：
  - `SceneFence` 结构体增加 `pub collapsed: bool` 与 `pub collapse_btn: Option<RectF>`；
- **[`crates/render/src/draw.rs`](file:///g:/Codes/winbosk/crates/render/src/draw.rs)**：
  - 在 `draw_fence_inner` 中支持折叠渲染：
    - 绘制标题栏圆角矩形底色与描边（高度为 `fence.height == title_h`）；
    - 绘制栅栏标题；
    - 绘制折叠指示器（`▸` 表示收起态，`▾` 表示展开态）；
    - 若 `fence.collapsed` 为 `true`，跳过后续所有图标、列表表头、滚动条的绘制与图层裁剪。
- **[`crates/render/src/overlay.rs`](file:///g:/Codes/winbosk/crates/render/src/overlay.rs)**：
  - `FenceHit` 中记录折叠按钮矩形 `collapse_btn: Option<RectF>`；
  - 在 `on_button_down` 中优先判定点击是否命中 `collapse_btn`，若是则分发 `OverlayEvent::FenceCollapseToggle { fence }` 并返回；
  - 在 `on_double_click` 中，若未命中图标但命中了栅栏标题栏 `f.title`，分发 `OverlayEvent::FenceTitleDoubleClicked { fence }`。

### 三、应用组装与交互层（`crates/app/`）
- **[`crates/app/src/scene.rs`](file:///g:/Codes/winbosk/crates/app/src/scene.rs)**：
  - 在 `build_scene` 中：
    - 计算标题栏高度 `title_h = (theme.title.size * 1.6 + theme.title_padding_bottom + 2.0 * theme.fence_padding).round()`；
    - 若 `f.collapsed`：
      - `SceneFence.height = title_h;`
      - `SceneFence.icons.clear();`
      - `SceneFence.collapsed = true;`
      - `SceneFence.scroll_max = 0.0;`
      - `SceneFence.scroll_view = 0.0;`
      - 计算并填充 `collapse_btn` 坐标矩形；
  - 在 `hit_model_from` 中：
    - 折叠栅栏的 `f.body` 尺寸以 `SceneFence.height`（即 `title_h`）为准；
- **[`crates/app/src/main.rs`](file:///g:/Codes/winbosk/crates/app/src/main.rs)**：
  - 在 `handle_event` 中处理 `OverlayEvent::FenceCollapseToggle { fence }` 和 `OverlayEvent::FenceTitleDoubleClicked { fence }`：
    ```rust
    if let Some(f) = rt.desk.fences.get_mut(fence) {
        f.collapsed = !f.collapsed;
        let _ = rt.store.save(&rt.desk);
    }
    ```
- **[`crates/app/src/context_menu.rs`](file:///g:/Codes/winbosk/crates/app/src/context_menu.rs)**：
  - 在 `fence_context_menu` 中增加收折菜单项：
    - 若 `f.collapsed` 则为「展开栅栏」；
    - 否则为「收起栅栏」；
  - 在 `handle_fence_menu_action` 中绑定折叠状态切换并保存配置。

---

## 5. 防御性自查清单 (Defensive Invariants)

- [x] **状态全集校验律**：`bounds.h` 与 `collapsed` 彻底解耦，折叠态不改变 `bounds.h`，保证展开后恢复准确原貌。
- [x] **空闲性能归零律**：折叠操作为单帧状态切换与 RGN 更新，无轮询定时器，无动画挂起，保持 0% CPU。
- [x] **旁路数据对齐律**：折叠时清空 `SceneFence.icons`，确保 `HitModel` 不生成属于折叠栅栏的图标命中矩形，杜绝不可见图标被误点或框选。

---

## 6. 验证与交付门禁 (Verification Gates)

1. **核心测试**：
   ```powershell
   cargo test -p winbosk-core
   cargo test --workspace
   ```
2. **代码质量**：
   ```powershell
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```
3. **真实走查验证**：
   - 在桌面上分别针对网格（Grid）和列表（List）栅栏点击标题栏右侧的折叠按钮，验证栅栏瞬时收起至仅剩标题；
   - 点击折叠栅栏原下方区域，验证是否可以正常点击到桌面壁纸或底层文件；
   - 双击标题栏空白处，验证栅栏顺利展开，内部图标完整无缺，尺寸恢复至收起前的高度；
   - 右键点击栅栏空白处，选择「收起栅栏」，验证是否正常折叠；右键选择「展开栅栏」，验证是否正常展开；
   - 重启程序，验证折叠状态正常从 `desk.json` 恢复。
