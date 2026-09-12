# Sylva 桌面栅栏待办事项与重构实施计划

本待办事项文档针对近期收集的 4 项交互与功能缺陷进行系统性建模、架构推演与分层方案设计。

---

## 1. 待办事项总览 (Task Overview)

| 序号 | 任务名称 | 所属领域 | 严重程度 | 现状与核心诉求 |
| :---: | :--- | :--- | :---: | :--- |
| **#1** | **控制中心与桌面栅栏点击联动** | `app` / `render` | 交互体验 | 控制中心开启时，点击桌面任意栅栏未联动切换控制中心顶部列表与详情设置。 |
| **#2** | **控制中心【切换桌面】更名为【恢复桌面】** | `render` / `app` | 文案对齐 | 按钮文案与用户心智模型不一致；原操作实质是暂时隐去栅栏并恢复原生桌面图标。 |
| **#3** | **栅栏内置收起/展开（Collapse/Expand）** | `core` / `render` / `app` | 核心功能 | 栅栏支持一键折叠；收起时仅保留标题栏并显示栅栏名称，非标题区域完全穿透。 |
| **#4** | **删除分类栅栏图标自动回归【桌面】栅栏** | `core` / `app` | 核心缺陷 | 删除分类栅栏后图标退入不可见的未分组区（`free_icons`）导致丢失；期望删除分类栅栏=取消分类，图标自动回归【桌面】。 |

---

## 2. 目标与非目标 (Goals & Non-Goals)

### Goals
1. **控制中心精准实时跟随**：当控制中心处于展开状态时，用户在桌面点击、拖动或右键操作任意栅栏，控制中心顶部列表自动高亮该栅栏，并自动滚动到视口内，下方属性面板同步呈现对应配置。
2. **术语心智一致性**：将控制中心单页内的操作按钮统一更名为「恢复桌面」（处于原始桌面状态时继续保持为「回到栅栏」）。
3. **栅栏极简折叠**：
   - 数据模型增加向后兼容的折叠状态 `collapsed: bool`；
   - 折叠后栅栏高度自动降为标题栏高度，内容区图标隐藏且不占用布局；
   - 镂空出的屏幕区域通过 `SetWindowRgn` 实时剪裁，实现物理级点击穿透至桌面；
   - 提供直观的双向切换入口（标题栏折叠按钮、双击标题栏空白处、右键菜单项）。
4. **图标生命周期闭环**：
   - 彻底消灭“不可见的黑洞未分组区”导致的图标失踪问题；
   - 无论是控制中心的「删除栅栏」还是右键菜单的「删除栅栏」，均依据语义查找【桌面】栅栏；
   - 转移图标归属而非丢弃，删除栅栏即等价于撤销分类。

### Non-Goals
1. 不变动系统原生 Explorer 图标的实际物理存储拓扑；
2. 不破坏 `bounds.h <= 0.0` 标记的自适应高度计算机制；
3. 不引入跨线程消息循环与第三方 UI 框架（严格遵守单线程无锁架构）。

---

## 3. 详细任务分解与技术方案

---

### Task #1: 控制中心与桌面栅栏点击联动

#### 现状根因分析
- 控制中心由 `rt.selected_fence` 驱动顶部栅栏列表行（`fence_rows`）的高亮以及详情区（`fence_detail`）的显示。
- 当前 `rt.selected_fence` 仅在用户直接点击控制中心列表行（`ConsoleZone::FenceSelect(i)`）或新增/删除栅栏时被修改。
- 当用户在桌面上点击栅栏主体、标题栏、右键菜单、框选或拖动栅栏时，`overlay` 派发事件并未通知主状态机更新 `rt.selected_fence`，导致控制中心面板处于“脱节假死”状态。

#### 方案设计
1. **统一事件归集点**：
   在 [`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs) 的主事件循环中，针对所有带有 `fence: usize` 的交互事件：
   - `OverlayEvent::IconClicked { fence, .. }`
   - `OverlayEvent::SelectDrag { fence, .. }`
   - `OverlayEvent::FenceMove { fence, .. }`
   - `OverlayEvent::FenceDragEnd { fence }`
   - `OverlayEvent::ContextMenu { fence, .. }`
   增加状态同步钩子：若 `rt.desk.console_open` 为 `true`，且 `fence < rt.desk.fences.len()`，则：
   ```rust
   if rt.desk.console_open && rt.selected_fence != fence {
       rt.selected_fence = fence;
       // 自动滚动控制中心列表，确保选中的栅栏行处于可视范围内
       ensure_selected_fence_visible(rt);
       redraw = true;
   }
   ```
2. **纯点击栅栏标题与空白时的通知**：
   在 [`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs) 中，当 `DragKind::Move` 未超过阈值（单纯点击标题栏或栅栏主体空白处）时，同样派发明确包含 `fence` 索引的事件（或 `FenceDragEnd`），使主状态机感知用户选中了该栅栏。

---

### Task #2: 控制中心【切换桌面】更名为【恢复桌面】

#### 现状根因分析
- 在 [`crates/render/src/draw.rs:385-389`](file:///g:/Codes/sylva/crates/render/src/draw.rs#L385-L389)，按钮文本被硬编码为：
  ```rust
  let toggle_label = if c.desktop_mode {
      "回到栅栏"
  } else {
      "切换桌面"
  };
  ```
- 对于普通用户，“切换桌面”容易被误解为 Windows 虚拟桌面（Win+Tab），且无法准确表达“暂时把真实桌面图标还原出来”的功能意图。

#### 方案设计
1. **渲染层文本变更**：
   将 `draw.rs` 中的标签修改为：
   ```rust
   let toggle_label = if c.desktop_mode {
       "回到栅栏"
   } else {
       "恢复桌面"
   };
   ```
2. **全局文档与注释对齐**：
   同步检查 `scene.rs`、`overlay.rs`、`main.rs` 及用户手册中对该按钮的文本描述与注释。

---

### Task #3: 栅栏内添加收展功能（收起时仅显示栅栏名称）

#### 契约与数据模型设计
1. **核心领域模型（`crates/core/src/model.rs`）**：
   在 `Fence` 结构体中扩展折叠状态字段，强制增加 `#[serde(default)]`：
   ```rust
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   pub struct Fence {
       pub id: u64,
       pub title: Option<String>,
       // ...
       #[serde(default)]
       pub collapsed: bool,
   }
   ```
2. **自适应高度与折叠高度的解耦约定**：
   - 遵循《Agent 操作规范》第 8 条：`bounds.h` 必须保存展开时的原高（或 `<=` 0.0 表示自适应高度）。
   - 折叠时**严禁修改 `bounds.h`**，而是由渲染与场景构建阶段按 `title_h` 进行临时裁切；
   - 展开时立即无缝恢复原有的 `bounds.h` 与 `last_layout_h`，杜绝高度丢失或反复抖动。

#### 几何布局与交互设计
1. **场景构建（`crates/app/src/scene.rs`）**：
   - 当 `f.collapsed` 为 `true` 时：
     - `SceneFence.height` 强制计算为 `title_h = (theme.title.size * 1.6 + theme.title_padding_bottom + 2.0 * theme.fence_padding)`；
     - 清空 `SceneFence.icons`，列表列头及滚动条不生成；
     - 标题栏右侧生成收展切换指示按钮（展开态显示 `▾`，收起态显示 `▸`）。
2. **穿透裁剪（`crates/render/src/overlay.rs`）**：
   - 依赖 `apply_hit_model` 中基于 `f.body` 的 `CreateRectRgn`；
   - 由于折叠状态下 `f.body.h == title_h`，剪裁出的 RGN 几何体仅有标题栏大小，栅栏原内容区域自动穿透，点击完全落至桌面底层壁纸/窗口。
   - **唯一的例外（见 plan 07）**：拖动栅栏期间，收起的栅栏会按「原大小」画一个虚线占位框（并把该矩形并入窗口区域，否则区域外画不出来）。它只解释"为什么这里就推不动了"，拖动结束当帧即撤除、穿透恢复；清理走 App 层白名单规则（`Runtime::drag_hint`），保证任何异常路径都不会让它滞留吞掉点击。
3. **触发入口（三通道交互）**：
   - **标题栏收展按钮**：点击标题栏右侧的折叠指示器即刻切换；
   - **双击标题栏**：双击标题栏空白区域直接切换收起/展开状态；
   - **右键上下文菜单**：在栅栏右键菜单增加「收起栅栏」/「展开栅栏」菜单项。

---

### Task #4: 删除分类栅栏图标自动回归【桌面】栅栏

#### 现状根因分析
1. 当前在 [`crates/app/src/main.rs:1124-1126`](file:///g:/Codes/sylva/crates/app/src/main.rs#L1124-L1126) 及 [`crates/app/src/context_menu.rs:190-192`](file:///g:/Codes/sylva/crates/app/src/context_menu.rs#L190-L192) 中，删除栅栏逻辑为：
   ```rust
   for id in ids {
       rt.desk.move_icon(&id, None);
   }
   if fence < rt.desk.fences.len() {
       rt.desk.fences.remove(fence);
   }
   ```
2. `move_icon(&id, None)` 将图标移到了 `desk.free_icons`。
3. Sylva 采用全盘桌面接管机制，底层的 Windows `SysListView32` 真实图标被隐藏（`IconGuard`），且 Sylva 只渲染 `desk.fences` 内的图标。
4. `free_icons` 没有对应的界面容器，图标一旦进入该集合便从桌面上彻底不可见，导致用户产生“文件被删除了”的极大恐慌；只有重新点击「一键整理」时，`auto_organize_all` 将 `free_icons` 重新收纳为新栅栏，图标才再度出现。

#### 期望方案与算法推演
1. **语义精准定位【桌面】栅栏（Semantic Invariant）**：
   定义函数 `resolve_desktop_fence(desk: &Desk, exclude_id: Option<u64>) -> Option<u64>`：
   - 优先寻找 `title == "桌面"` 或 `storage_path == desktop_dir` 且 `id != exclude_id` 的栅栏；
   - 次优寻找未配置规则的第一个普通栅栏（`rule.is_none()`）；
   - 若桌面上完全没有其他栅栏（所有栅栏均被删除），则自动在原位创建标准默认「桌面」栅栏。
2. **原子化转移与删除**：
   ```rust
   pub(crate) fn delete_fence_and_reclaim_icons(rt: &mut Runtime, fence_idx: usize) {
       let Some(fence) = rt.desk.fences.get(fence_idx) else { return; };
       let deleting_id = fence.id;
       let icon_ids = fence.icon_ids.clone();

       // 1. 定位受纳归流的「桌面」栅栏
       let target_fid = match resolve_desktop_fence(&rt.desk, Some(deleting_id)) {
           Some(fid) => fid,
           None => {
               // 桌面上已无其他栅栏，自动重置出一个空白桌面栅栏
               create_fallback_desktop_fence(rt)
           }
       };

       // 2. 将被删除栅栏内的图标逐一归流到目标桌面栅栏
       for id in icon_ids {
           rt.desk.move_icon(&id, Some(target_fid));
       }

       // 3. 安全删除栅栏
       rt.desk.fences.remove(fence_idx);

       // 4. 同步旁路数据与选中状态
       if fence_idx < rt.last_layout_h.len() {
           rt.last_layout_h.remove(fence_idx);
       }
       rt.selected_fence = rt.selected_fence.saturating_sub(1).min(rt.desk.fences.len().saturating_sub(1));

       // 5. 立即持久化
       let _ = rt.store.save(&rt.desk);
   }
   ```

---

## 4. 隐性动力学自查推演（六律自检）

| 校验律 | 涉及任务 | 潜在隐患推演 | 针对性防御方案 |
| :--- | :--- | :--- | :--- |
| **常驻后台冲突律** | Task #4 | 图标由分类栅栏移至【桌面】栅栏时，后台库文件同步引擎（`sync_library`）是否会错误触发物理删除？ | 图标移动仅改变逻辑归属 `IconLocation::Fence(desktop_fid)`，不调用 `remove_icon_entirely`，底层磁盘物理文件（`Icon.path`）分毫未动。 |
| **空闲性能归零律** | Task #1, #3 | 点击联动与收展切换是否会引发持续帧刷新？ | 仅在状态改变帧触发重绘并提交新的 `HitModel`。无补间动画时立即归零 CPU。 |
| **孤儿状态回收律** | Task #4 | 若目标【桌面】栅栏不存在且未自动生成，图标是否会遗留在 `free_icons` 形成死数据？ | `resolve_desktop_fence` 必须兜底创建默认「桌面」栅栏，禁止任何操作回流到不可见的 `free_icons`。 |
| **语义精准定位律** | Task #4 | 是否直接盲取 `rt.desk.fences[0]` 作为归流目标？ | 禁止硬编码下标。必须按 `title == "桌面"` 或 `storage_path == desktop_dir` 语义匹配，并显式排除 `deleting_id`。 |
| **状态全集校验律** | Task #3 | 折叠状态下如果触发了「一键整理」，分类文件是否会挤入折叠栅栏？ | 图标分拣归位只更新 `icon_ids`，折叠栅栏保持折叠；待用户展开时自动完整排布。 |
| **旁路数据对齐律** | Task #4 | 删除栅栏后，`rt.last_layout_h` 是否同步清除？ | 在删除 `rt.desk.fences[fence_idx]` 的同一代码块中，必须同步执行 `rt.last_layout_h.remove(fence_idx)`，杜绝下标偏移越界。 |

---

## 5. 分层改动清单 (Implementation Steps)

### 一、领域模型层（`crates/core/`）
- **[`crates/core/src/model.rs`](file:///g:/Codes/sylva/crates/core/src/model.rs)**：
  - `Fence` 添加 `#[serde(default)] pub collapsed: bool`；
  - 扩展单元测试：验证折叠状态字段在 JSON 序列化/反序列化中与旧配置的双向兼容性；
  - 增加纯函数测试：验证图标在栅栏删除后归流到指定栅栏的完整性。

### 二、渲染与穿透层（`crates/render/`）
- **[`crates/render/src/draw.rs`](file:///g:/Codes/sylva/crates/render/src/draw.rs)**：
  - 将控制中心内的 `"切换桌面"` 文案替换为 `"恢复桌面"`；
  - 在 `draw_fence` 中适配折叠绘制：当 `f.collapsed` 时仅绘制标题栏与收展符号（`▸`/`▾`），跳过主体图标与滚动条；
- **[`crates/render/src/scene.rs`](file:///g:/Codes/sylva/crates/render/src/scene.rs)**：
  - `SceneFence` 增加收展指示器几何矩形 `collapse_toggle: Option<RectF>` 与 `collapsed: bool` 标记。
- **[`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)**：
  - 增加标题栏收展按钮命中判定；
  - 增加标题栏双击切换收展判定。

### 三、应用组装与交互层（`crates/app/`）
- **[`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs)**：
  - 在事件分发逻辑中为桌面栅栏交互注入 `rt.selected_fence` 联动更新逻辑；
  - 提炼公共的 `delete_fence_and_reclaim_icons` 函数，统一控制中心 `ConsoleZone::RemoveFence` 的删除归流行为；
  - 确保删除栅栏时同步移除 `rt.last_layout_h` 对应项。
- **[`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)**：
  - 在 `build_scene` 中为折叠栅栏计算高度 `title_h`，隐藏折叠栅栏内的所有图标项；
  - 在 `hit_model_from` 中确保折叠栅栏的 `body` 仅包含标题高度，使外部区域完全穿透。
- **[`crates/app/src/context_menu.rs`](file:///g:/Codes/sylva/crates/app/src/context_menu.rs)**：
  - 栅栏右键菜单增加「收起栅栏」/「展开栅栏」菜单项；
  - 栅栏右键菜单的「删除栅栏」迁移至 `delete_fence_and_reclaim_icons`。

---

## 6. 验证与交付门禁 (Verification Gates)

### 自动化验证
```powershell
# 1. 核心模型测试（必须瞬时通过）
cargo test -p sylva-core

# 2. 全工作区测试
cargo test --workspace

# 3. 静态代码分析（零警告）
cargo clippy --workspace -- -D warnings

# 4. 代码格式检查
cargo fmt --all -- --check
```

### 真实交互走查用例
1. **联动验证**：打开控制中心（`Ctrl+Alt+T`），在桌面上依次点击不同的栅栏，验证控制中心顶部栅栏列表中对应项是否同步高亮，下方配置是否实时切换为该栅栏的配置。
2. **文案验证**：检查控制中心界面中的按钮是否已更名为「恢复桌面」，点击后是否正常恢复 Windows 原生桌面图标且按钮变为「回到栅栏」。
3. **收展验证**：
   - 点击栅栏标题栏收展按钮或双击标题栏，验证栅栏是否折叠至只保留标题栏；
   - 在收起的栅栏下方空白处点击鼠标，验证点击是否穿透到系统桌面；
   - 再次点击或双击展开，验证图标完整恢复，高度未发生任何异常变形。
4. **归流验证**：
   - 在桌面放置若干不同类型的文件并点击「⚡ 一键整理桌面」，生成分类栅栏；
   - 在控制中心或右键菜单中删除其中一个分类栅栏（例如「常用应用」）；
   - 验证被删除栅栏内的图标全部自动回归至「桌面」基础栅栏中，绝不发生图标失踪现象。

---

## 7. 已知缺口 (Known Gaps)

### #5 桌面「虚拟壳项」未镜像（含回收站）

- **现状**：桌面镜像栅栏按 plan 08 修复后，内容 = 用户桌面文件 + 公共桌面文件；实测真实桌面
  （`SysListView32` 只读读取，140 项）= 用户桌面 105 + 公共桌面 34 + **虚拟项 1（回收站）**，
  即**只差回收站**这 1 项。
- **为何没一并做**：
  1. 虚拟项的 shell 属性**不置** `SFGAO_HIDDEN`/`GHOSTED`（实测 此电脑/网络/控制面板/用户文件夹/
     库/图库/Linux… 全是 `attrs=0x20000000`），无法用属性区分「Windows 是否把它显示在桌面上」；
     判定需要读 `HKCU\...\HideDesktopIcons\{NewStartPanel,ClassicStartMenu}` 的 per-CLSID 开关，
     并处理「值缺失时按 Windows 内建默认」的语义（实测本机仅 `{5C899A03-…}=1`，而 DefView 只显示了
     回收站 → 说明缺失即默认，且默认并非「一律显示」）。
  2. 更要紧的是：**无路径项在栅栏里是死图标**——`DesktopItem::launch()` 对 `path == None` 直接返回
     （`crates/shell/src/items.rs:50-53`），双击毫无反应；右键也只落到简版菜单。放进来会让用户
     以为回收站坏了。
- **补齐所需**：① 虚拟项可见性判定（CLSID + 注册表开关 + 内建默认表）；② 无路径项的打开与右键
  （PIDL 级 `ShellExecuteEx` / `IContextMenu`）；③ 虚拟项没有文件系统路径 → 孤儿回收分支不能按其
  处理（它们恒存在，需单独白名单）。
