# 控制中心「文件位置」行语义重排实施计划

本计划修正控制中心栅栏详情区「文件位置」行的**列语义错位**：标签列里被塞进一个"值"（状态芯片），
而"值"的位置被放上一个"动作"（「更改位置…」按钮），导致用户读不出「标签 → 值」的配对关系。
同时把该行的两个动作收口成"长得像动作"的按钮，并补上此前完全缺失的**后果提示**。

> 前序：本行由 [plan 04](04-fence-storage-visibility.md) 引入。plan 04 完成了"可见性 + 可回退"，
> 本计划只动**排版语义与可点性**，不改 `storage_path` 的任何状态语义。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

1. **列语义归位**：标签列只放标签，值放值，动作放动作。
   - 值行（第一行）：`文件位置` 标签 + 状态标签（模式）+ 真实落地路径 + `打开` 动作；
   - 动作行（第二行）：后果提示小字（左）+ 动作按钮（右端对齐）。
2. **可点性与视觉一致**：把"看着像按钮却点不动"与"看着像灰字却能点"这对反转修掉。
   - 状态标签彻底去按钮化（灰底填充标签，无描边，**不参与命中**——保持不可点，但也不再像按钮）；
   - 路径文本**取消命中**（plan 04 曾把它整块设为 `ChangeStoragePath` 热区，本计划回退该决定）；
   - 新增 `打开` 按钮：路径旁给一个**看得见**、真正有用的动作（资源管理器打开目录）。
3. **后果提示常显**：补上"在此删除会真删磁盘文件"这一条此前界面上一个字都没有的信息
   （两种模式的判定真源见 [`crates/app/src/file_ops.rs`](file:///g:/Codes/sylva/crates/app/src/file_ops.rs) `is_managed_path`）。
4. **动作文案与风险对齐**：「更改位置…」→「更改文件位置…」（与行标签同名，动作对象无歧义），
   并在**确有文件要被搬动时**加一次模态确认（该动作是真搬文件：复制到新目录 + 删除旧副本）。
5. **零高度变更**：仍是 2 行，`detail_visible_rows` 的 `n += 2` 与 `CONSOLE_FENCE_DETAIL_H` 不动。

### Non-Goals

1. 不改 `Fence::storage_path` 字段类型、不改 `desk.json` 结构、零迁移。
2. 不改删除语义本身（`is_managed_path` / `delete_managed_file` 的判定与文案）。
3. 不改「恢复默认」的**行为**（仍为"解链 + 摘除成员，磁盘文件不动"），只改它与提示语的关系。
   plan 04 §7 遗留的"文案暗示把文件搬回来"问题，本计划用第二行的后果提示消解，
   **不做反向迁移**。
4. 不新增常驻定时器、轮询、文件监视；不引入 tooltip 基础设施（plan 04 的既定约束）。
5. 不改面板默认宽度 `CONSOLE_W` / 最小宽 `CONSOLE_MIN_W` / 行距 30。
6. 不改「应用内部」模式下路径**必须常显**这一 plan 04 的 Goal（路径降级为次要信息，但不隐藏）。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 核心层：文案与宽度预算（改 [`crates/core/src/storage.rs`](file:///g:/Codes/sylva/crates/core/src/storage.rs)）

```rust
impl StorageKind {
    /// 状态标签文案。两者都是 5 个 CJK 字，宽度天然对齐（见 `chip_width` 单测）。
    pub fn badge(self) -> &'static str {
        match self {
            StorageKind::AppLibrary => "应用内部库",     // 原「应用内部」：名词化，说明它是一个"库"
            StorageKind::ExternalFolder => "外部文件夹",
        }
    }

    /// 动作行左侧的后果提示。**两种模式都必须回答"在这里删会不会删到真文件"**——
    /// `is_managed_path` = 内部库 ∪ 任一栅栏的链接目录，两种模式都为真。
    pub fn hint(self) -> &'static str {
        match self {
            StorageKind::AppLibrary => "所有栅栏共用 · 删除会真删文件",
            StorageKind::ExternalFolder => "删除会真删磁盘文件",
        }
    }
}

/// 状态标签宽度：文案实测宽 + 两侧内边距（`font_size` 与 `pad_x` 必须同单位）。
/// 取代原先硬编码的 `56.0 * s`——硬编码与文案长度耦合，改文案就会挤字。
pub fn chip_width(badge: &str, font_size: f32, pad_x: f32) -> f32 {
    estimate_width(badge, font_size) + 2.0 * pad_x
}

/// 预算不足时返回空串（= 不绘制），保证提示语永不与动作按钮重叠。
pub fn hint_text(kind: StorageKind, budget: f32, font_size: f32) -> &'static str {
    let h = kind.hint();
    if budget > 0.0 && estimate_width(h, font_size) <= budget { h } else { "" }
}
```

单位口径：`font_size` = `theme.label.size * 0.72`（**已按 DPI 缩放**的物理像素，与 `formats.detail` 同源），
`pad_x` / `budget` 同为物理像素。本模块保持**零 OS 依赖**、纯函数、可内存单测。

### 2.2 渲染层：场景字段（改 [`crates/render/src/scene.rs`](file:///g:/Codes/sylva/crates/render/src/scene.rs)）

`SceneFenceDetail` 的存储相关字段（8 个，语义逐条写死，杜绝"字段名对不上绘制意图"）：

| 字段 | 语义 | 命中 |
| :--- | :--- | :--- |
| `storage_btn` | 动作行「更改文件位置…」 | `ChangeStoragePath` |
| `storage_reset` | 动作行「恢复默认」（`h <= 0.0` = 不出现） | `ResetStoragePath` |
| `storage_open` | 值行「打开」按钮（**新增**） | `OpenStoragePath` |
| `storage_kind` | 状态标签的模式（文案与配色） | 不参与 |
| `storage_chip` | 值行状态标签矩形（**仅绘制**，不可点） | 不参与 |
| `storage_path_text` | 中段省略后的真实路径（App 层按 DPI 预算预计算） | 不参与（**回退 plan 04 的热区**） |
| `storage_path_rect` | 路径文本区（**仅绘制**） | 不参与（**回退 plan 04 的热区**） |
| `storage_hint_text` | 动作行后果提示（空串 = 不绘制，**新增**） | 不参与 |
| `storage_hint_rect` | 后果提示文本区（`h <= 0.0` = 不绘制，**新增**） | 不参与 |

### 2.3 渲染层：命中枚举（改 [`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)）

```rust
/// 栅栏管理页：在资源管理器中打开选中栅栏的真实落地目录。
OpenStoragePath,
```

### 2.4 组装层：行几何的纯函数（改 [`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)）

把两行几何抽成纯函数，使其可在内存中单测"空间互斥"（指南 §4.1 要求）：

```rust
/// 「文件位置」两行几何（值行 + 动作行）。纯函数：只吃常量与文案，返回矩形。
pub(crate) struct StorageRow {
    pub tag: RectF,            // 值行：状态标签
    pub path: RectF,           // 值行：路径文本区
    pub path_text: String,     // 值行：中段省略后的路径
    pub open: RectF,           // 值行：「打开」
    pub change: RectF,         // 动作行：「更改文件位置…」
    pub reset: RectF,          // 动作行：「恢复默认」（h <= 0.0 = 不出现）
    pub hint: RectF,           // 动作行：后果提示（h <= 0.0 = 不绘制）
    pub hint_text: String,     // 动作行：后果提示文案（空串 = 不绘制）
}

pub(crate) fn storage_row_geometry(
    detail: &RectF,     // 详情区矩形（物理像素）
    row_y: f32,         // 值行顶边 y（动作行 = row_y + 30*s）
    kind: StorageKind,
    can_reset: bool,
    path: &str,
    theme: &Theme,      // 取 label.size（已缩放）与 scale
) -> StorageRow
```

**几何常量（DIP，实现时乘 `s`）与推导式**——所有宽度都由文案实测宽度推出，不再硬编码：

| 量 | 取值 |
| :--- | :--- |
| 行内左右内缘 | `inner_left = detail.x + 2*s`；`inner_right = detail.x + detail.w - 2*s` |
| 标签列 | `label_w = 40*s`（不动，仍是 `draw.rs` 的 `label_w` 口径） |
| 状态标签 | `x = detail.x + label_w`；`y = row_y + 3*s`；`h = 18*s`；`w = chip_width(badge, detail_font, 6*s)` |
| 路径 | `x = tag.right + 6*s`；`w = (open.x - 6*s - x).max(0)`；`y = row_y + 4*s`；`h = 18*s` |
| 「打开」 | 右端对齐：`x = inner_right - open_w`；`w = estimate_width("打开", label_font) + 16*s`（= 40*s） |
| 「更改文件位置…」 | 右端对齐：`w = estimate_width("更改文件位置…", label_font) + 16*s`（= 100*s） |
| 「恢复默认」 | `w = estimate_width("恢复默认", label_font) + 16*s`（= 64*s）；`x = change.x - 6*s - w`，仅当 `x >= inner_left + label_w + 6*s` 时出现，否则零矩形 |
| 提示语 | `budget = actions_left - 6*s - inner_left`；文案由 `hint_text(kind, budget, detail_font)` 决定；空串时 `hint` 为零矩形 |
| 动作行 y | `row_y + 30*s`（行距 30 不动） |

**实测预算（`s = 1`，`detail_font = 8.64`，提示文案宽 = 字数 × 8.64）**：

| 面板宽 | 模式 | 提示预算 | 提示文案宽 | 结论 |
| :--- | :--- | ---: | ---: | :--- |
| `CONSOLE_W = 320`（默认） | 应用内部库 | 186 | 131.7 | **常显** |
| `CONSOLE_W = 320` | 外部文件夹 | 116 | 77.8 | **常显** |
| `CONSOLE_MIN_W = 260` | 应用内部库 | 126 | 131.7 | 差 5.7 → **整条不画** |
| `CONSOLE_MIN_W = 260` | 外部文件夹 | 56 | 77.8 | **整条不画** |

即：**默认宽度下两种模式都常显；面板被拖到最小宽时提示语整体消失**（优雅降级，不截断、不重叠）。
刻意**不**为了给提示腾地方而砍掉「恢复默认」——那会让外部文件夹模式在窄面板下退回 plan 04
修掉的单向门。这条取舍由 §6.1 的不变量测试钉住（"空文案 ⟺ 零矩形"）。

### 2.5 组装层：存储位置变更的具名结果（改 [`crates/app/src/file_ops.rs`](file:///g:/Codes/sylva/crates/app/src/file_ops.rs)）

```rust
/// 目标目录不可作为存储位置的原因（具名结果，不用歧义布尔值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageReject { NotADirectory, InsideLibrary, NoSuchFence }

impl StorageReject {
    /// 面向用户的拒绝原因（告警框文案）。与变体同处一地，新增变体会被 match 穷尽性强制补文案。
    pub(crate) fn reason(self) -> &'static str;
}

/// 校验目标目录（纯判定，无副作用）——`change_fence_storage` 与点击入口共用同一真源。
pub(crate) fn validate_storage_dir(rt: &Runtime, new_dir: &str) -> Result<(), StorageReject>;

/// 需要随「更改文件位置…」搬走的库内项数量（纯查询，无副作用）。
pub(crate) fn count_movable_items(rt: &Runtime, fence_idx: usize) -> usize;

/// `change_fence_storage` 改为返回 `Result<(), StorageReject>`（原先静默 `return`）。
pub(crate) fn change_fence_storage(rt: &mut Runtime, fence_idx: usize, new_dir: &str)
    -> Result<(), StorageReject>;
```

### 2.6 壳层：打开目录（改 [`crates/shell/src/items.rs`](file:///g:/Codes/sylva/crates/shell/src/items.rs)）

```rust
/// 用系统默认动作打开一个目录（资源管理器）。与 `DesktopItem::launch` 同一套 ShellExecuteW 手法。
pub fn open_folder(path: &str);
```

### 2.7 确认弹窗（改 [`crates/app/src/context_menu.rs`](file:///g:/Codes/sylva/crates/app/src/context_menu.rs)）

```rust
/// 「更改文件位置…」前的二次确认。**仅在确有文件要被搬动时调用**（count > 0），
/// 无文件可搬时保持无模态（遵守「基础操作无模态阻断」）。
pub(crate) fn confirm_move_storage(rt: &Runtime, count: usize, dir: &str) -> bool;
```

复用 `confirm_delete_fence` 已验证的模态手法：先借 `menu_owner()` 焦点代理把本线程提到前台，
owner 用 overlay 本体（**不是**离屏 1×1 的代理），默认焦点落在「否」（`MB_DEFBUTTON2`），
结束后把前台还给弹出前的窗口。`MessageBoxW` 的再入消息由外层 `ReentryGuard` 丢弃。

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

1. **常驻后台与同步引擎（`SyncLibrary`，4s 周期）**：本改动**不触碰** `storage_path` 的写入路径，
   只重排几何与文案。`打开` 与 `更改文件位置…` 都不新增写入者，故不与后台差集同步产生写竞争。
   确认弹窗是模态的，期间 `SyncLibrary` 仍在独立节奏上跑——它只读写 `desk`，不依赖控制中心几何，
   无交互面。
2. **空闲性能归零律**：零新增定时器 / 轮询 / 文件监视。`storage_row_geometry` 是纯算术；
   `estimate_width` 是纯字符累加。**关键约束**：不得为了判断目录是否存在而在每帧 `is_dir()`
   —— `打开` 的存在性判定放在**点击时**，不在 `build_console`（绘制路径）里。
3. **孤儿与物理变更回流**：用户可能已把链接目录删掉。`打开` 在点击时做一次 `is_dir()` 判定，
   不存在则 `tracing::warn!` 后不动作（**已知取舍**：静默不动作优于弹系统错误框；
   此时栅栏本身已被 `SyncLibrary` 同步为空，用户能看到状态）。本计划不引入新的孤儿项。
4. **模态与重入**：`confirm_move_storage` 与 `confirm_delete_fence` 同构——嵌套消息由外层
   `ReentryGuard` 丢弃，不会造成 `Runtime` 借用冲突（`main.rs` 的 `handle_event` 外层已保护）。
5. **DPI 动力学**：所有宽度由 `estimate_width(文案, 已缩放字号) + 常数 × s` 推出，
   `WM_DPICHANGED` 重新 `apply_theme_scale` 后几何自动跟随，不会出现"字号放大但按钮宽度不变"的挤字。
6. **绘制时序**：`storage_open` 是幽灵按钮（透明底 + 描边），悬停走既有 `hover_zone` 单值机制，
   不引入新的 hover 状态位；路径与提示语**不再**与任何按钮共用 hover 高亮（修掉 plan 04 的耦合）。
7. **不新增任何后台重绘触发**：本改动不改变"无状态变更不重绘"的门控（`AnimTick` 只在补间期间存在）。

---

## 4. 分层改动清单 (Implementation Steps)

按 `core → shell → render → app` 自底向上实施，禁止越级。

### 4.1 核心层（`crates/core/`）

- [`crates/core/src/storage.rs`](file:///g:/Codes/sylva/crates/core/src/storage.rs)：
  改 `badge()` 文案；新增 `hint()` / `chip_width()` / `hint_text()`；补单测（§6.1）。

### 4.2 壳层（`crates/shell/`）

- [`crates/shell/src/items.rs`](file:///g:/Codes/sylva/crates/shell/src/items.rs)：新增 `pub fn open_folder(path: &str)`。

### 4.3 渲染层（`crates/render/`）

- [`crates/render/src/scene.rs`](file:///g:/Codes/sylva/crates/render/src/scene.rs)：
  `SceneFenceDetail` 新增 `storage_value_row` / `storage_open` / `storage_hint_text` / `storage_hint_rect`
  （`storage_value_row` 是「文件位置」标签的锚点，保证"标签与它命名的值在同一行"），
  并把 `storage_chip` / `storage_path_rect` 的注释改为「仅绘制，不参与命中」。
- [`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)：`ConsoleZone` 新增 `OpenStoragePath`。
- [`crates/render/src/draw.rs`](file:///g:/Codes/sylva/crates/render/src/draw.rs)：
  - `draw_storage_chip` 去按钮化：灰底填充标签（`AppLibrary` = 白 0.12 填充；`ExternalFolder` = accent 0.25 填充），
    圆角 `5*s`（不再是 `h/2` 胶囊），**无描边**，文字 `0.85`；**不要**再画成与分段按钮同族的描边胶囊。
  - 值行：标签 `文件位置` + 状态标签 + 路径（`0.55` 白，**与 hover 解耦**）+ 新增 `draw_ghost_button`（`打开`）。
  - 动作行：左侧 `storage_hint_text`（`formats.detail`，白 0.40；空串跳过）+ 右端
    `draw_segmented_button`（`更改文件位置…` / `恢复默认`）。
  - 新增私有 `draw_ghost_button`：透明底 + `1*s` 白 0.22 描边 + 白 0.75 文字，hover 时填充白 0.12。

### 4.4 组装层（`crates/app/`）

- [`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)：
  - 新增纯函数 `storage_row_geometry` + `StorageRow`（§2.4），`build_console` 改为调用它；
  - 命中表：**移除** `(ChangeStoragePath, d.storage_path_rect)`（路径不再可点），
    **新增** `(OpenStoragePath, d.storage_open)`；零矩形仍不入表（`storage_reset` / `storage_open` 都判 `h > 0.0`）。
  - `detail_visible_rows` 的 `n += 2` 与 `CONSOLE_FENCE_DETAIL_H` **不动**（仍是 2 行）。
- [`crates/app/src/file_ops.rs`](file:///g:/Codes/sylva/crates/app/src/file_ops.rs)：
  抽出 `validate_storage_dir` / `count_movable_items`（`change_fence_storage` 复用同一判定，消除重复守卫），
  `change_fence_storage` 改返回 `Result<(), StorageReject>`。
- [`crates/app/src/context_menu.rs`](file:///g:/Codes/sylva/crates/app/src/context_menu.rs)：
  新增 `confirm_move_storage` / `warn_storage_reject`，并把 `confirm_delete_fence` 与它们共用的
  模态外壳抽成 `modal_box`（此前是同一段 unsafe 前台代理逻辑的第三份拷贝）；
  `pick_folder` 的对话框标题由「选择栅栏链接的文件夹」改为「选择栅栏的存储文件夹」
  并去掉 `#[allow(dead_code)]`（本次起被 `ChangeStoragePath` 正式使用）。
  **顺带删除已成孤儿项的 `pick_paths`**（多选文件夹 + 标题「添加到栅栏」）：它唯一剩下的调用者
  就是 `ChangeStoragePath`——也就是说"改存储位置"此前复用的是"添加到栅栏"的选择器，
  连对话框标题都是错的。`FenceMenuAction` 里并没有「添加」入口，故该函数删除后无功能损失；
  随之清理 `IShellItemArray` / `FOS_ALLOWMULTISELECT` 两个随之失效的导入。
- [`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs)：
  - `ChangeStoragePath` 分派改为：`pick_folder` → `validate_storage_dir`（失败弹告警，**修掉静默失败**）
    → `count_movable_items`（> 0 才 `confirm_move_storage`）→ `change_fence_storage`；
    由 `pick_paths`（多选、标题「添加到栅栏」）改为 `pick_folder`（单选、标题正确）。
  - 新增 `OpenStoragePath` 分派：取选中栅栏的 `describe(...).path`（**与绘制同源**），
    `is_dir()` 通过则 `winbosk_shell::items::open_folder`，否则 `tracing::warn!`。
  - 新增 `warn_storage_reject`（`MessageBoxW`，仅 OK + 警告图标），复用 `confirm_delete_fence` 的前台代理手法。

### 4.5 文档

- [`AGENTS.md`](file:///g:/Codes/sylva/AGENTS.md) 领域术语表：在 `Storage Mode` 条目补一句
  状态标签文案为「应用内部库」/「外部文件夹」，并注明路径行**不可点**、动作只有
  「更改文件位置…」「恢复默认」「打开」三个，避免后续 Agent 按旧注释把路径重新接回热区。

---

## 5. 防御性自查清单 (Defensive Invariants)

- [x] **常驻后台冲突律**：不新增写入 `storage_path` 的路径；确认弹窗期间后台同步照常，无共享可变状态。
- [x] **空闲性能归零律**：零新增定时器；纯算术几何；`is_dir()` 只在点击时调用一次，
      **不在 `build_console`（每帧/每重绘）里调用**。
- [x] **孤儿状态回收律**：不改镜像成员的生命周期；链接目录被外部删除时 `打开` 只记日志不动作，
      栅栏内容由既有 `SyncLibrary` 归零。
- [x] **语义精准定位律**：所有取栅栏一律 `selected_fence.min(len.saturating_sub(1))`；
      `打开` 的路径来自 `storage::describe(...)`（与绘制同源），**不得**在分派处另写一份 `match storage_path`。
- [x] **状态全集校验律**：`恢复默认` 的显隐仍由 `StorageInfo::can_reset` 复合判定
      （`ExternalFolder && !desktop_source`），几何为零矩形时绘制与命中同时忽略；
      `confirm_move_storage` 仅在 `count_movable_items > 0` 时调用。
- [x] **旁路数据对齐律**：`detail_visible_rows` 的 `n += 2` 与 `build_console` 的 `row += 2` 同步
      （本次**不增行**，两处均不动）；命中表与 `SceneFenceDetail` 字段在同一提交内同步增删。
- [x] **空间互斥（指南 §4.1）**：`tag / path / open / change / reset / hint` 六矩形两两不重叠，
      由 §6.1 的几何单测在全宽与最小宽两个极端下断言。
- [x] **越界防御**：所有宽度用 `.max(0.0)` 收敛；`reset` 出现前校验左端不与标签列重叠；
      提示语预算不足时整条不绘制（不截断半个字）。
- [x] **零迁移**：不改 `desk.json`；旧配置直接可用。
- [x] **可点性唯一性**：路径矩形**不在**命中表内（`zones` 里不得再出现 `ChangeStoragePath` + `storage_path_rect`），
      避免"看得见的值仍是隐形按钮"的回归。

---

## 6. 验证与交付门禁 (Verification Gates)

### 6.1 单元测试

`cargo test -p winbosk-core` —— `storage::tests` 新增：

- `badge_texts_are_equal_width`：两个 `badge()` 在 `chip_width` 下等宽（同 5 个 CJK 字），
  防止将来只改一侧文案导致两模式标签宽窄不一。
- `chip_width_grows_with_text`：`chip_width("外部文件夹", f, p) > chip_width("外部", f, p)`，
  且空文案 = `2 * pad_x`。
- `hint_text_drops_when_budget_short`：预算 0 / 负 → 空串；预算恰好等于文案宽 → 原样返回；
  预算略小 → 空串（**不得**返回被截断的半个字）。
- `hints_mention_real_delete`：两个 `hint()` 都非空且都含「删」字
  （把"两种模式都必须回答删除后果"这条契约钉进测试）。

`cargo test -p winbosk-app` —— `scene::tests` 新增：

- `storage_row_geometry_is_pairwise_disjoint`：在 `CONSOLE_W` / `CONSOLE_MIN_W` ×
  `s ∈ {1.0, 1.5, 2.0}` × 两种模式 × `can_reset` 两种取值下，断言
  `tag / path / open / change / reset / hint` 中所有非零矩形两两不重叠，且全部落在
  `[inner_left, inner_right]` 内；并断言 `tag` / `open` / `change` 永不为零
  （值行必须能显示"存在哪"，两个动作按钮必须常驻）。
  *（实测抓到的坑：`CONSOLE_W` / `CONSOLE_MIN_W` 是 DIP 常量，测试里必须乘 `s` 才是真实面板宽——
  漏乘会让高 DPI 用一条假想的窄面板去判重叠。）*
- `storage_row_geometry_hint_visibility`：默认宽度下两种模式的提示都必须非空；
  并在两种宽度下断言**不变量**「空文案 ⟺ 零矩形」（不允许画空框、也不允许有文字没矩形）。
- `storage_row_geometry_reset_absent_is_zero_rect`：`can_reset == false` 时 `reset` 为零矩形
  （`h <= 0.0`）；`can_reset == true` 时它排在「更改文件位置…」左侧且不重叠；
  少了它提示预算应变宽。

### 6.2 幂等与零输入

- 空栅栏（无图标）→ `count_movable_items == 0` → **不弹确认**，直接改路径；
- 连续两次「打开」→ 只是再开一次资源管理器窗口，不改任何状态；
- 连续两次「更改文件位置…」选同一目录 → 第二次 `count_movable_items == 0`（项已在目标目录内），无搬移。

### 6.3 静态质量门禁（CI 阻断项）

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

**实测（2026-09-13，含 §7.2 全部审查修复之后）：**

| 门禁 | 结果 |
| :--- | :--- |
| `cargo build --workspace` | `Finished` dev profile，19.41s，**零警告** |
| `cargo test --workspace` | **41 + 66 + 34 + 18 passed, 0 failed**（另 3 个 0 测试的 crate） |
| `cargo clippy --workspace -- -D warnings` | `Finished`，6.36s，**零警告** |
| `cargo fmt --all -- --check` | 干净（无 diff） |

构建环境按 `.workbuddy-ai/skills/sylva-repo-ops/SKILL.md` §1 注入 MSVC（Git Bash 下
`link.exe` 会被 coreutils 遮蔽），并设 `WINBOSK_WINSDK_ROOT` 绕开被沙箱屏蔽的 `reg.exe`——
`crates/app/build.rs` 已有免注册表回退，**无需再打临时补丁**（SKILL.md §2 的 `SYLVA_TMP_RC_TOOLKIT`
手法已过时，见该文件更新后的说明）。

### 6.4 真实走查

`$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe`（干净退出，**勿硬杀**）。逐项确认：

1. 新建空白栅栏：值行 = `文件位置 | [应用内部库] | G:\…\data\library | 打开`；
   动作行 = `所有栅栏共用 · 删除会真删文件` + 右端 `更改文件位置…`，**无**「恢复默认」；
2. 点「打开」→ 资源管理器打开 `…\data\library`；鼠标悬停路径**不再**点亮任何按钮；
3. 点「更改文件位置…」→ 对话框标题为「选择栅栏的存储文件夹」；
   - 选一个空目录 → 无确认弹窗（无文件可搬）→ 标签变「外部文件夹」+ 出现「恢复默认」+ 路径更新；
   - 栅栏内有库内项时再改一次 → 出现确认弹窗（默认焦点在「否」），选「否」→ **零变动**；
4. 选应用内部库本身或其父/子目录 → 弹告警框（不再是静默无反应）；
5. 点「恢复默认」→ 回到「应用内部库」、按钮消失、磁盘文件未被动过；
6. 把面板拖到最小宽（`CONSOLE_MIN_W`）→ 无重叠、无越界；`外部文件夹` 模式下提示语整体消失；
7. 125% / 150% DPI 下无挤字、无重叠。

> **本次未做 GUI 实跑**：本机有用户既有实例在跑（旧名 `sylva.exe`，PID 35700），它持有单实例
> 互斥 `WinBosk.Desktop.Fences`，新进程会静默自退；按 SKILL.md §4 不杀用户实例。且 2s 的
> autostop 跑**覆盖不到控制中心面板**（默认不展开，SKILL.md §4c）。故上述 1~7 项**仍待人工
> 按 `Ctrl+Alt+T` 走查**；几何正确性由纯函数单测（§6.1）覆盖，其余靠 §7.2 的静态审查。
> 另：控制中心的热键与面板渲染路径本次未改动，风险面集中在存储行的几何与文案。

### 6.5 独立上下文子代理审查

实施完成后派发**无对话历史**的子代理，重点对抗性走查：

1. 命中表与视觉一致性（是否存在"可见不可点/可点不可见"）；
2. `can_reset` / `hint_text` 的复合状态分支在两种模式 × 全宽/最小宽下的取值矩阵；
3. `storage_row_geometry` 的边界（预算为 0/负、`detail.w` 极小、`s` 非整数）；
4. `open_folder` 的 ShellExecuteW 失败路径与后台进程前台化；
5. 是否引入空闲期工作（`is_dir()` 是否漏进绘制路径）；
6. `change_fence_storage` 改 `Result` 后所有调用点是否都处理了错误分支。

---

## 7. 对抗性审查结论（实施后填写）

派发**无对话历史**的子代理（只读、不跑 cargo）走查上述 6 项。结论如下。

### 7.1 审查确认无误的部分

| 走查项 | 结论 | 证据 |
| :--- | :--- | :--- |
| 命中表与视觉一致 | 路径热区**已彻底移除**：`hit_model_from` 只 push `OpenStoragePath` / `ChangeStoragePath` / `ResetStoragePath` 三个；`storage_path_rect` 全仓仅出现在绘制与赋值处；`pick_paths` / `IShellItemArray` / `FOS_ALLOWMULTISELECT` 源码零残留（仅存于未跟踪的 `target/full_backup/`） | `app/src/scene.rs:1840-1847` |
| 零矩形不被误命中 | 命中判定额外要求 `w > 0 && h > 0`，故 `can_reset == false` 时的 `RectF::default()` 天然不可命中 | `render/src/overlay.rs:1449`、`app/src/scene.rs:652-667` |
| 几何与本文 §2.4 一致 | 逐项吻合，绘制侧全部取自同一批字段 | `app/src/scene.rs:614-676`、`render/src/draw.rs:747-835` |
| 空闲期零新增工作 | `is_dir()` 只在点击路径；`build_console` / `storage_row_geometry` / 绘制全是纯算术；`describe` 绘制与点击**同源** | `app/src/main.rs:1312`、`app/src/scene.rs:865` |
| `change_fence_storage` 改 `Result` | 唯一调用点处理了 `Err`；`validate_storage_dir` 纯判定，`?` 前置于任何写入 | `app/src/main.rs:1289`、`app/src/file_ops.rs:197-209` |
| 几何边界不 panic | `storage_row_geometry` 无除法、无 `clamp`；预算 ≤ 0 时 `elide_middle` / `hint_text` 返回空串，无「零宽有字」 | 静态推演 + 单测 |

### 7.2 审查发现的问题与处置

**P1 — `modal_box` 的文档在撒谎（已修）**

`modal_box` 的首行文档宣称「借焦点代理把本线程提到前台」，但函数体**只读取** `proxy` 用于事后恢复，从不 raise；唯一的 raise 在 `main.rs` 的 `ChangeStoragePath` 分支。于是
`RemoveFence → confirm_delete_fence` 路径上没有任何前台化，而 overlay 是 `WS_EX_NOACTIVATE`
（点击它不会让本进程成为前台进程），删除确认框可能不获焦、被前台窗口盖住。

本次把 `modal_box` 提升为「所有模态框都必须走这里」的公共外壳，等于**把一条错误断言权威化**，故必须修：
把 `raise_to_foreground()` 移入 `modal_box`，位置在**捕获 `prev` 之后、`MessageBoxW` 之前**
（顺序不可换：先 raise 会把 `prev` 记成我们自己的隐藏代理，弹完就还原不回去）。
`main.rs` 里那次 raise 保留，但注释改为「是给 `pick_folder` 的 `IFileDialog` 用的」——
它不走 `modal_box`，不会自己提权。

**P2 — `open_folder` 丢弃 `ShellExecuteW` 返回值（已修）**

`let _ = ShellExecuteW(...)` 把失败吞掉了，UI 上表现为「点了没反应」。改为返回 `bool`
（判据是返回值 **> 32**，`HINSTANCE` 口径而非 `GetLastError`），调用方失败时留一行 `tracing::warn!`。
同时 `OpenStoragePath` 分支补上 `raise_to_foreground()`：本进程常驻后台，不提权拉起资源管理器
可能不获焦。

**P2 — 值行内三段同字号文字落在三条基线上（已修，本次唯一观感修正）**

行标签写死 `vr.y + 2·s`、状态标签就地按 `label.size × 1.6` 估行高居中（实际字号是 `detail`，
比标签框还高，文字被顶到框外）、路径写死 `row_y + 4·s`。三段同字号却各有各的偏移，
**整行看起来是歪的**——与本次要修的「位置关系读不懂」是同一类病。

修法：在 `storage_row_geometry` 里算**唯一顶线** `text_dy = (24·s − detail_font × 1.6) / 2`，
经新增字段 `storage_text_top` 下发；行标签、状态标签、路径、动作行提示语一律用它。
`draw_storage_chip` 改为接收 `text_top` 而不是自己估算。

**P2 — 两处注释与实现不符（已修）**

- `app/src/scene.rs` 行预算注释仍写「第一行标签+操作按钮，第二行模式芯片+真实路径」，与实现的
  值行/动作行结构不符；
- `render/src/overlay.rs` 的连击白名单排除清单未列入新控件 `OpenStoragePath` / `ResetStoragePath`
  （行为上默认 `false` 是正确的 fail-safe，但「与真正会 `zones.push` 的控件一一对应」的注释已不成立）。

### 7.3 审查未能验证的事项（须实机走查）

1. **125% / 150% DPI 下的真实字形宽度**与上述 1.6·s 基线差的观感；
2. **最小面板宽**下提示语被丢弃、`ResetStoragePath` 按钮不出现时的降级观感；
3. **从后台进程拉起资源管理器是否真的获焦**（P2 的修复效果）；
4. `MessageBoxW` 在**未提权**时是否确实失焦（P1 的影响面）；
5. 四道门禁的静态判断无法替代实跑（子代理按要求未跑 cargo）。

上述 1、2 需人工按 `Ctrl+Alt+T` 展开面板走查；3、4 需实机触发删除确认与「打开」。
本次因单实例互斥被用户既有实例占用，未做 GUI 实跑（详见 §6.4）。
