# 控制中心「文件位置」行语义重排实施计划

本计划修正控制中心栅栏详情区「文件位置」行的**列语义错位**：标签列里被塞进一个"值"（状态芯片），
而"值"的位置被放上一个"动作"（「更改位置…」按钮），导致用户读不出「标签 → 值」的配对关系。
同时把该行的两个动作收口成"长得像动作"的按钮，并补上此前完全缺失的**后果提示**。

> 前序：本行由 [plan 04](04-fence-storage-visibility.md) 引入。plan 04 完成了"可见性 + 可回退"，
> 本计划只动**排版语义与可点性**，不改 `storage_path` 的任何状态语义。

---

## 1. 目标与非目标 (Goals & Non-Goals)

> **注（2026-09-13）**：本章是 **v1** 的规格。§8 做了一次设计修订——「打开」按钮被删除，
> 改由**路径自身**承担 `OpenStoragePath`。下面凡提到 `打开` 按钮、或"路径取消命中"之处，
> **都只对 v1 成立**；**现役（v2）契约见 §8**。原文保留以留下当初的目标与判据。

### Goals

1. **列语义归位**：标签列只放标签，值放值，动作放动作。
   - 值行（第一行）：`文件位置` 标签 + 状态标签（模式）+ 真实落地路径 + `打开` 动作；
   - 动作行（第二行）：后果提示小字（左）+ 动作按钮（右端对齐）。
2. **可点性与视觉一致**：把"看着像按钮却点不动"与"看着像灰字却能点"这对反转修掉。
   - 状态标签彻底去按钮化（灰底填充标签，无描边，**不参与命中**——保持不可点，但也不再像按钮）；
   - 路径文本**取消命中**（plan 04 曾把它整块设为 `ChangeStoragePath` 热区，本计划回退该决定）；
   - 新增 `打开` 按钮：路径旁给一个**看得见**、真正有用的动作（资源管理器打开目录）。
   - *（v2 修订：按钮删除，路径本身成为 `OpenStoragePath` 热区；见 §8。）*
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

`SceneFenceDetail` 的存储相关字段（语义逐条写死，杜绝"字段名对不上绘制意图"）：

| 字段 | 语义 | 命中 |
| :--- | :--- | :--- |
| `storage_btn` | 动作行「更改文件位置…」 | `ChangeStoragePath` |
| `storage_reset` | 动作行「恢复默认」（`h <= 0.0` = 不出现） | `ResetStoragePath` |
| `storage_path_hit` | **路径文本的命中区**（贴实际字形，宽度 ≤ `storage_path_rect.w`） | `OpenStoragePath` |
| `storage_kind` | 状态标签的模式（文案与配色） | 不参与 |
| `storage_chip` | 值行状态标签矩形（**仅绘制**，不可点） | 不参与 |
| `storage_path_text` | 中段省略后的真实路径（App 层按 DPI 预算预计算） | 不参与（热区见 `storage_path_hit`） |
| `storage_path_rect` | 路径文本区（**仅绘制**：决定省略预算与裁剪框） | 不参与 |
| `storage_text_top` | 值行内 detail 字号文字的**共用顶线** | 不参与 |
| `storage_hint_text` | 动作行后果提示（空串 = 不绘制） | 不参与 |
| `storage_hint_rect` | 后果提示文本区（`h <= 0.0` = 不绘制） | 不参与 |

> **设计修订（2026-09-13，见 §8）**：初版在值行右端放了一个「打开」幽灵按钮。宽度实测证明
> 它要吃掉 40·s，占路径预算的 1/3，把默认面板宽下的库路径从"完整显示"压到 23/40 字。
> 已**移除该按钮**，改由路径自身承担"打开目录"这个动作（`storage_path_hit` 接
> `OpenStoragePath`，hover 提亮 + 下划线把"可点"画出来）。值行因此是**零按钮的纯信息行**。

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
    pub value_row: RectF,      // 值行整行（仅作标签锚点与行带文档，不参与命中）
    pub tag: RectF,            // 值行：状态标签
    pub path: RectF,           // 值行：路径文本区（决定省略预算与裁剪框）
    pub path_text: String,     // 值行：中段省略后的路径
    pub path_hit: RectF,       // 值行：路径命中区（贴字形，点它 = 打开目录）
    pub change: RectF,         // 动作行：「更改文件位置…」
    pub reset: RectF,          // 动作行：「恢复默认」（h <= 0.0 = 不出现）
    pub hint: RectF,           // 动作行：后果提示（h <= 0.0 = 不绘制）
    pub hint_text: String,     // 动作行：后果提示文案（空串 = 不绘制）
    pub text_top: f32,         // 值行内 detail 字号文字的共用顶线
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
| 共用顶线 | `text_top = row_y + (24*s - detail_font × 1.6) / 2`（`detail_font = label.size × 0.72`） |
| 状态标签 | `x = detail.x + label_w`；`y = row_y + 3*s`；`h = 18*s`；`w = chip_width(badge, detail_font, 6*s)` |
| 路径 | `x = tag.right + 6*s`；`w = (inner_right - x).max(0)`（**吃满剩余宽度**）；`y = text_top`；`h = 18*s` |
| 路径命中区 | `x/y` 同路径；`w = min(estimate_width(path_text, detail_font), path.w)`（**贴字形**） |
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

**路径预算（`s = 1`，`d.w = CONSOLE_W − 2·CONSOLE_PAD = 296`）** —— 初版漏算了这一项，见 §8：

| 版本 | 路径省略预算 | 默认宽 · 35 字中文路径 | 最小宽 |
| :--- | ---: | :--- | :--- |
| 改动前（plan 09 之前） | `d.w − 70·s` = 226 | **完整 35 字** | 22 字 |
| 初版（值行带「打开」按钮） | `d.w − 151.2·s` = 144.8 | 15 字 | 8 字 |
| **现版（路径自己承担打开）** | `d.w − 105.2·s` = 190.8 | **28 字** | **15 字** |

路径比"改动前"窄 35.2·s，全部来自**状态标签搬进值列**（+37.2·s）——那是本次要修的病，不可退。
**新增任何控件前，必须先把"这块宽度从谁身上抢"写进这张表**，否则会在别处悄悄降级。

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
6. **绘制时序**：路径 hover 走既有 `hover_zone` 单值机制（`OpenStoragePath`），不引入新的 hover 状态位；
   提示语**不再**与任何按钮共用 hover 高亮（修掉 plan 04 的耦合）。
   **路径的 hover 提亮 + 下划线是本行唯一的"可点"提示**——它是 `OpenStoragePath` 的可发现性来源，
   改弱它等于把"看着是灰字、其实能点"的老毛病放回来。下划线只铺在**命中区**上（贴字形），
   保证"画出来的范围 = 能点的范围"。
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
  `SceneFenceDetail` 新增 `storage_value_row` / `storage_path_hit` / `storage_text_top` /
  `storage_hint_text` / `storage_hint_rect`
  （`storage_value_row` 是「文件位置」标签的锚点，保证"标签与它命名的值在同一行"），
  并把 `storage_chip` / `storage_path_rect` 的注释改为「仅绘制，不参与命中」，
  `storage_path_hit` 注释写明"点它 = 打开目录，宽度贴字形 ≤ `storage_path_rect.w`"。
- [`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)：`ConsoleZone` 新增 `OpenStoragePath`。
- [`crates/render/src/draw.rs`](file:///g:/Codes/sylva/crates/render/src/draw.rs)：
  - `draw_storage_chip` 去按钮化：灰底填充标签（`AppLibrary` = 白 0.12 填充；`ExternalFolder` = accent 0.25 填充），
    圆角 `5*s`（不再是 `h/2` 胶囊），**无描边**，文字 `0.85`；**不要**再画成与分段按钮同族的描边胶囊。
  - 值行（**零按钮**）：标签 `文件位置` + 状态标签 + 路径。路径常态白 0.55，hover（`OpenStoragePath`）
    提亮到 0.92 并在命中区下缘画 `1*s` 白 0.75 下划线。
  - 动作行：左侧 `storage_hint_text`（`formats.detail`，白 0.40；空串跳过）+ 右端
    `draw_segmented_button`（`更改文件位置…` / `恢复默认`）。
  - 值行内所有 detail 字号文字（标签 / 状态标签 / 路径）的顶线一律取 `storage_text_top`，
    动作行提示语取 `hint.y`（同一条口径）——**任何一处自带偏移都会让整行看起来是歪的**。

### 4.4 组装层（`crates/app/`）

- [`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)：
  - 新增纯函数 `storage_row_geometry` + `StorageRow`（§2.4），`build_console` 改为调用它；
  - 命中表：**移除** `(ChangeStoragePath, d.storage_path_rect)`，
    **新增** `(OpenStoragePath, d.storage_path_hit)`（路径接的是无害可逆的"打开目录"，
    破坏性的搬文件动作只留在动作行的具名按钮上）；零矩形仍不入表（`storage_reset` / `storage_path_hit` 都判 `> 0.0`）。
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
  状态标签文案为「应用内部库」/「外部文件夹」，并写明**值行是零按钮的纯信息行**、
  路径本身是「打开目录」的热区、**路径必须吃满值行剩余宽度**（塞任何占位控件都会从路径身上抢宽度），
  以及"命中表里不得再出现路径 → `ChangeStoragePath`"。

---

## 5. 防御性自查清单 (Defensive Invariants)

- [x] **常驻后台冲突律**：不新增写入 `storage_path` 的路径；确认弹窗期间后台同步照常，无共享可变状态。
- [x] **空闲性能归零律**：零新增定时器；纯算术几何；`is_dir()` 只在点击时调用一次，
      **不在 `build_console`（每帧/每重绘）里调用**。
- [x] **孤儿状态回收律**：不改镜像成员的生命周期；链接目录被外部删除时点路径（打开）只记日志不动作，
      栅栏内容由既有 `SyncLibrary` 归零。
- [x] **语义精准定位律**：所有取栅栏一律 `selected_fence.min(len.saturating_sub(1))`；
      打开的路径来自 `storage::describe(...)`（与绘制同源），**不得**在分派处另写一份 `match storage_path`。
- [x] **状态全集校验律**：`恢复默认` 的显隐仍由 `StorageInfo::can_reset` 复合判定
      （`ExternalFolder && !desktop_source`），几何为零矩形时绘制与命中同时忽略；
      `confirm_move_storage` 仅在 `count_movable_items > 0` 时调用。
- [x] **旁路数据对齐律**：`detail_visible_rows` 的 `n += 2` 与 `build_console` 的 `row += 2` 同步
      （本次**不增行**，两处均不动）；命中表与 `SceneFenceDetail` 字段在同一提交内同步增删。
- [x] **空间互斥（指南 §4.1）**：`tag / path / change / reset / hint` 五矩形两两不重叠，
      由 §6.1 的几何单测在全宽与最小宽两个极端下断言。
      （`path_hit` 不在互斥集合内：它是 `path` 的子矩形，天然重叠，由专门测试覆盖"贴字形且不越界"。）
- [x] **越界防御**：所有宽度用 `.max(0.0)` 收敛；`reset` 出现前校验左端不与标签列重叠；
      提示语预算不足时整条不绘制（不截断半个字）。
- [x] **零迁移**：不改 `desk.json`；旧配置直接可用。
- [x] **可点性唯一性**：`zones` 里不得再出现 `ChangeStoragePath` + `storage_path_rect` 这种
      "看得见的值是隐形按钮"的接法；路径只接 `OpenStoragePath`（无害可逆）。
- [x] **宽度归属唯一性**：路径必须吃满值行剩余宽度（右缘精确落在详情区内缘）——
      值行里不得再塞任何占位控件，否则会从路径身上抢宽度（实测一个 40·s 按钮让库路径从
      完整显示退化成 23/40 字）。由 §6.1 的 `path_owns_rest_of_value_row` 断言。

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

- `storage_row_geometry_is_pairwise_disjoint`：在四种（面板宽, DPI）组合
  `(CONSOLE_W,1.0) / (CONSOLE_MIN_W,1.0) / (CONSOLE_W,1.5) / (CONSOLE_MIN_W,2.0)`
  （`s ∈ {1.0, 1.5, 2.0}`，但**不是** 2×3 全交叉）× 两种模式 × `can_reset` 两种取值下，断言
  `tag / path / change / reset / hint` 五个**排布**矩形中所有非零矩形两两不重叠，且全部落在
  `[inner_left, inner_right]` 内；并断言 `tag` / `change` 永不为零、`path_hit` 永不为零
  （值行必须能显示"存在哪"且路径必须可点，动作按钮必须常驻）。
  *（`path_hit` 不参与两两比对：它是 `path` 的**子矩形**（同 x/y、宽度 ≤ `path.w`），天然重叠；
  它的契约由 `..._path_hit_hugs_text` 与 `storage_zones_value_row_has_exactly_the_path` 覆盖。）*
  *（实测抓到的坑：`CONSOLE_W` / `CONSOLE_MIN_W` 是 DIP 常量，测试里必须乘 `s` 才是真实面板宽——
  漏乘会让高 DPI 用一条假想的窄面板去判重叠。）*
- `storage_row_geometry_hint_visibility`：默认宽度下两种模式的提示都必须非空；
  并在两种宽度下断言**不变量**「空文案 ⟺ 零矩形」（不允许画空框、也不允许有文字没矩形）。
- `storage_row_geometry_reset_absent_is_zero_rect`：`can_reset == false` 时 `reset` 为零矩形
  （`h <= 0.0`）；`can_reset == true` 时它排在「更改文件位置…」左侧且不重叠；
  少了它提示预算应变宽。
- `storage_row_geometry_path_owns_rest_of_value_row`：路径右缘**精确落在详情区内缘**
  （吃满值行剩余宽度），路径左端**紧贴状态标签、只隔一个固定 `STORAGE_TAG_GAP`**
  （标签与路径之间被塞任何东西都会立刻失败），且状态标签仍不被压住。
  覆盖 `s ∈ {1.0, 1.25, 1.5, 2.0}` × `CONSOLE_W` / `CONSOLE_MIN_W` × 两种模式 × `can_reset` 两种取值。
  *（这条是**宽度回退的回归保险**：初版值行右端有个 40·s 的「打开」按钮，实测把默认宽下的
  库路径从"完整显示（40 字）"压到 23 字。见 §8。）*
- `storage_row_geometry_path_hit_hugs_text`：覆盖 `s ∈ {1.0, 1.25, 1.5, 2.0}` × 两种面板宽 ×
  两种模式 × `can_reset` 两种取值 × **空路径**，断言命中区是绘制框的**子矩形**（同 x / 同 y /
  同高 / 不宽于）、**永不宽于字形**，且「严丝合缝贴字形」与「被绘制框钳住（文字已截断、
  铺满整框）」**只允许二选一**——文字没截断却铺满整框 = "看不见却能点"复发，
  文字截断了却没铺满 = 白丢可点区域；空路径必须退化为零命中区且**不入命中表**。
  *（上一版该测试只喂 `D:\归档\资料库` 一个输入，永远非空 → "空路径 → 零命中区"那条断言恒真、
  从未执行，是第二轮审查抓到的 P2-1。）*
- `storage_zones_value_row_has_exactly_the_path`：**直接断言命中表**——值行**有且只有**
  `OpenStoragePath` 一个热区，且矩形逐字段等于 `path_hit`；`ChangeStoragePath` 必须常驻；
  `ResetStoragePath` 入表 ⟺ 非零矩形；命中表长度恰为 `1 + 1 + usize::from(has_reset)`
  （多一个热区就失败）。
  *（几何不重叠管不住**映射**：值行冒出第二个热区、或把整段预算接成 `ChangeStoragePath`，
  只有在这里才会暴露。为此把命中区构造抽成纯函数 `storage_zones(path_hit, change, reset)`，见 §2.4。）*
- `storage_row_geometry_shares_one_text_top`：值行内三段 detail 字号文字**共用唯一顶线**——
  断言 `path.y == text_top`、动作行提示语与值行文字用**同一偏移**、且整行文字
  （行高 = `1.6 × detail 字号`）不溢出值行带。覆盖 `s ∈ {1.0, 1.25, 1.5, 2.0}` × 两种模式
  × `can_reset` 两种取值。
  *（这条是**审查后补的**：`storage_text_top` 字段是修 P2 时新增的，补测试前它没有任何断言保护。
  已用 A/B 证明非空转——把 `path_rect.y` / `hint.y` 临时改回写死偏移，该测试立刻
  `FAILED: 路径未落在共用顶线上`；改回即通过。）*

### 6.2 幂等与零输入

- 空栅栏（无图标）→ `count_movable_items == 0` → **不弹确认**，直接改路径；
- 连续两次点路径（打开）→ 只是再开一次资源管理器窗口，不改任何状态；
- 连续两次「更改文件位置…」选同一目录 → 第二次 `count_movable_items == 0`（项已在目标目录内），无搬移。

### 6.3 静态质量门禁（CI 阻断项）

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

**实测（2026-09-13）** —— 第一轮 = §7.2 修复之后；第二轮 = §8 设计修订 + §7.4 复审修复之后；
第三轮 = §7.5 复审修复之后（含用真 DWrite 实测的新单测）：

| 门禁 | 第一轮 | 第二轮 | 第三轮（现役） |
| :--- | :--- | :--- | :--- |
| `cargo build --workspace` | `Finished` 19.41s，零警告 | `Finished` 8.09s，零警告 | `Finished` 8.55s，**零警告** |
| `cargo test --workspace` | 42 + 66 + 34 + 18 | 45 + 66 + 34 + 18 | **45 + 66 + 35 + 18 passed, 0 failed**（另 3 个 0 测试的 crate；render +1 = `measure_detail_is_tighter_than_estimate_for_ascii`） |
| `cargo clippy --workspace -- -D warnings` | `Finished` 6.36s，零警告 | `Finished` 4.22s，零警告 | `Finished` 3.89s，**零警告** |
| `cargo fmt --all -- --check` | 干净 | 干净 | 干净（无 diff） |

（第二、三轮各因新代码里几处超长行被 rustfmt 判 diff，`cargo fmt --all` 后复检干净。）

构建环境按 `.workbuddy-ai/skills/sylva-repo-ops/SKILL.md` §1 注入 MSVC（Git Bash 下
`link.exe` 会被 coreutils 遮蔽），并设 `WINBOSK_WINSDK_ROOT` 绕开被沙箱屏蔽的 `reg.exe`——
`crates/app/build.rs` 已有免注册表回退，**无需再打临时补丁**（SKILL.md §2 的 `SYLVA_TMP_RC_TOOLKIT`
手法已过时，见该文件更新后的说明）。

### 6.4 真实走查

`$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe`（干净退出，**勿硬杀**）。逐项确认：

1. 新建空白栅栏：值行 = `文件位置 | [应用内部库] | G:\…\data\library`（**零按钮**，路径吃满到内缘）；
   动作行 = `所有栅栏共用 · 删除会真删文件` + 右端 `更改文件位置…`，**无**「恢复默认」；
2. 悬停路径 → 提亮（0.55 → 0.92）+ 出现下划线；点它 → 资源管理器打开 `…\data\library`；
   悬停状态标签 → **无任何反应**（它是状态不是按钮）；
   光标移出窗口 → 提亮与下划线**立刻消失**（§7.4 P2-5 的修复）；
3. 点「更改文件位置…」→ 对话框标题为「选择栅栏的存储文件夹」；
   - 选一个空目录 → 无确认弹窗（无文件可搬）→ 标签变「外部文件夹」+ 出现「恢复默认」+ 路径更新；
   - 栅栏内有库内项时再改一次 → 出现确认弹窗（默认焦点在「否」），选「否」→ **零变动**；
4. 选应用内部库本身或其父/子目录 → 弹告警框（不再是静默无反应）；
5. 点「恢复默认」→ 回到「应用内部库」、按钮消失、磁盘文件未被动过；
6. 把面板拖到最小宽（`CONSOLE_MIN_W`）→ 无重叠、无越界；`外部文件夹` 模式下提示语整体消失；
7. 125% / 150% DPI 下无挤字、无重叠。

> **本次未做 GUI 实跑**，理由如下（**注意：不是「互斥被占用」**）：
> 本机在跑的是**更名前的旧二进制** `sylva.exe`（PID 35700），它持有的是**旧互斥名
> `Sylva.Desktop.Fences`**；而 `main.rs` 现在用的是 `WinBosk.Desktop.Fences`。
> 已用 `OpenMutexW` 实测：`Sylva.Desktop.Fences` 存在、`WinBosk.Desktop.Fences` **不存在**——
> 也就是说新构建**不会**因单实例而自退，旧实例并不能挡住新实例。
>
> 之所以仍然不跑，是另外两条理由：
> 1. **数据目录会撞车**：`<exe_parent>/data` 是硬约定（`AGENTS.md` 第 7 条），而旧实例也在
>    `target\debug\` 下，两个进程会**同时读写同一份 `data\`（配置与内部库）**，
>    4s 心跳落盘交错有损坏用户栅栏配置的实际风险；
> 2. **不该打扰用户的活桌面**：起第二个 overlay 会在用户桌面上叠出第二套栅栏、并二次
>    `SW_HIDE` 真实桌面图标，退出时又无条件 `SW_SHOW`（与旧实例的隐藏状态打架）。
>    抓屏还会拍到用户桌面的私人内容。
>
> 结论：**留待用户关闭旧实例后人工走查**。若要在不碰用户配置的前提下自行验证，可行做法是把
> `winbosk.exe` 复制到临时目录（`<exe_parent>/data` 随之落在临时目录、配置全新）再
> `WINBOSK_AUTOSTOP_MS=60000` + `run_in_background` 启动、用 `PostMessageW` 投
> `WM_HOTKEY` 唤出面板抓屏——本次未采用，因为仍会在用户桌面上叠第二套栅栏。
>
> 几何正确性由纯函数单测（§6.1）覆盖，其余靠 §7.2 的静态审查。
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

> **注（2026-09-13，第二轮之后补）**：本节是**第一轮**审查的存档记录，描述的是 **v1** 设计
> （值行 = 标签 + 状态标签 + 灰路径，动作全靠按钮；「打开」是**独立幽灵按钮**，路径本身不可点）。
> §8 随后做了一次修订——「打开」按钮删除，改由**路径自身**承担 `OpenStoragePath`。
> 因此下文凡出现「路径热区已彻底移除」「值行零热区」之处，**都只对 v1 成立**；
> **v2 的现役契约见 §8**。原文保留，是为了留下"第一轮为什么这么判"的推理链。

### 7.1 审查确认无误的部分

| 走查项 | 结论 | 证据 |
| :--- | :--- | :--- |
| 命中表与视觉一致（**v1 口径**） | v1：路径**不可点**，值行唯一热区是新增的「打开」幽灵按钮；`hit_model_from` 只 push `OpenStoragePath` / `ChangeStoragePath` / `ResetStoragePath` 三个；`pick_paths` / `IShellItemArray` / `FOS_ALLOWMULTISELECT` 源码零残留（仅存于未跟踪的 `target/full_backup/`）。**v2 修订见 §8**：按钮删除，`OpenStoragePath` 改接 `storage_path_hit` | `app/src/scene.rs:1884-1888`（v2） |
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

上述 1、2 需人工按 `Ctrl+Alt+T` 展开面板走查；3、4 需实机触发删除确认与点路径（打开）。
本次未做 GUI 实跑——**原因不是单实例互斥被占用**（旧实例持有的是旧互斥名
`Sylva.Desktop.Fences`，挡不住新构建），而是「两个进程会同时读写同一份
`<exe_parent>/data`」与「不该在用户的活桌面上叠第二套栅栏」，详见 §6.4。

### 7.4 第二轮对抗性审查（v2 修订后，2026-09-13）

§8 的设计修订落地后，又派发了一个**无对话历史**的子代理复审 v2 的 diff（同样只读、不跑 cargo）。
结论：**1 个 P1 + 5 个 P2**，全部已处置。

**P1 — 注释与实现完全相反（已修）**

`render/src/draw.rs` 的值行注释写着「状态标签与路径都**不是**按钮…动作只有三个按钮」——
v2 恰恰相反：路径**就是**唯一的打开热区。这类"注释反着写"比没有注释更坏：它会主动误导下一个
改这一行的人。已按 v2 契约重写。

**P2-1 — 两个新测试名不副实（已修）**

- `..._path_hit_hugs_text` 的唯一输入是 `D:\归档\资料库`（永远非空），于是它注释里宣称覆盖的
  "空路径 → 零命中区"那条断言**恒真、从未执行**；且 `path_hit.w == text_w.min(path.w)` 是
  源码 `scene.rs:649` 的逐字镜像。→ 已改为全矩阵（`s ∈ {1.0,1.25,1.5,2.0}` × 两档面板宽 ×
  两种模式 × `can_reset` × **空路径**），并把"严丝合缝贴字形 / 被绘制框钳住"写成**二选一**的
  独立性质（文字没截断却铺满整框 = "看不见却能点"复发；截断了却没铺满 = 白丢可点区域）。
- **新增** `storage_zones_value_row_has_exactly_the_path`：**直接断言命中表**——值行有且只有
  `OpenStoragePath`，矩形逐字段等于 `path_hit`，`ChangeStoragePath` 常驻，
  `ResetStoragePath` 入表 ⟺ 非零矩形，表长恰为 `1 + 1 + usize::from(has_reset)`。
  为此把命中区构造抽成纯函数 `storage_zones(path_hit, change, reset)`——几何不重叠管不住**映射**。
- 另给 `..._path_owns_rest_of_value_row` 补了"路径左端紧贴标签、只隔一个 `STORAGE_TAG_GAP`"。

**四条断言均已用 A/B 证明非空转**（逐个临时重引入缺陷 → 全部如期失败 → 还原；基线 45 passed 不变）：

| 临时重引入的缺陷 | 期望失败的断言 | 实测 |
| :--- | :--- | :--- |
| `path_hit.w = path_w`（命中区铺满整段预算） | 命中区永不宽于字形 | ✅ `宽于字形` |
| 空路径时给 `path_hit.w = 8` | 空路径必须零命中区 | ✅ `没有文字却留着` |
| 值行路径接成 `ChangeStoragePath`（**原始 bug**） | 值行必须有且只有一个 `OpenStoragePath` | ✅ `值行必须恰好有一个打开热区` |
| 标签与路径之间插 4·s | 路径必须紧贴标签 | ✅ `标签与路径之间被塞了东西` |

**P2-2 — §7.1 与 §8 自相矛盾（已修）**

§7.1 仍写着"路径热区**已彻底移除**"，而 §8 把路径热区加了回来。已给 §7 加 **v1 / v2 口径说明**
并改写该行——保留原文是为了留下第一轮的推理链，但不能让读者以为它描述的是现役代码。

**P2-3 — 5 处「打开」残留注释（已修）**

`app/src/scene.rs` 四处 + `shell/src/items.rs` 一处，仍在描述已被删除的「打开」按钮。

**P2-4 — 下划线锚在行带底而非文字行盒（已修）**

`path_rect.y + path_rect.h − 1·s` 落在字形下方约 3·s（也低于状态标签底缘），看着像"路径下面画了条
横线"而不是"下划线"。改为锚在文字行盒底：`storage_text_top + 1.6 × detail 字号`。

**P2-5 — 控制台悬停会「粘」住（已修，顺带修复的既有缺陷）**

`render/src/overlay.rs` 的 `WM_MOUSELEAVE` 只把本地镜像 `state.console_hovered` 置空，
**从不 emit `ConsoleHover { zone: None }`**；而 App 层 `rt.console_hover` 才是绘制高亮的唯一来源，
光标离开窗口后又不会再有 `WM_MOUSEMOVE` 来纠正它 → 离开前那个控件的高亮 / 下划线一直亮着。
（`HoverLeave` / `CursorLeave` 都只管图标悬停与 Dock 放大，不清控制台悬停。）
修法：在 `WM_MOUSELEAVE` 里，若镜像为 `Some` 则**先 emit 清除事件再置空**。
这是 v2 之前就存在的缺陷，只是 v2 让路径热区的宽度随面板变化，更容易被看见。

### 7.5 第三轮对抗性审查（2026-09-13）

再派发一个**无对话历史**子代理复审上面三个提交。**无 P0**；1 个 P1 + 4 个 P2，全部已处置。

**P1 — 命中区/下划线并没有"贴实际字形"（已修）**

本轮自定的核心不变量是"画出来的范围 = 能点的范围"，但命中区与下划线宽度都取自
`estimate_width`——而它**对 ASCII 偏大 24.5%**（见 §8.1 补记）。默认库路径上实测：
估算 214.27 / 真实 172.05，下划线因此戳出文字 **42 DIP**，默认面板宽下被省略的路径更会
留出约 **49 DIP** 的空白被划线——正是本轮要消灭的那个毛病，只是缩小了。

修法不是去调 `estimate_width`（它是全 App 共用口径，动它会影响所有布局，须单独立 plan）：
- **新增** `TextFormats::measure_detail(text) -> Option<(w, h)>`，用真 DWrite
  `CreateTextLayout` + `GetMetrics` 实测；`TextFormats` 自持一份 `IDWriteFactory`
  （布局层拿不到工厂，所以"画的人"自己留一份）；
- **下划线改用实测宽与实测行高**，命中区**保持**用估算宽——命中区偏宽是**故意**的：
  路径右侧留白也能点，无害且更宽容（值行右端没有别的控件可抢，动作又是可逆的"打开"）。
  即"能点的可以比画出来的大，画出来的必须等于字形"；
- 实测失败时退回估算值照常画，**不因为量不出来就不给 hover 反馈**；
- 用单测钉住（含 CJK 应≈估算、空串应为 `(0,0)` 而非"实测失败"）。

**P2-1 — 悬停只在 `WM_MOUSELEAVE` 清了，拖拽开始时没清（已修）**

`WM_MOUSEMOVE` 的悬停更新被 `if state.drag.is_none()` 门禁，`ConsoleMove` / `ConsoleResize` /
拖图标期间 `rt.console_hover` 不更新 → 拖面板时进入拖拽那一刻鼠标下的控件（多半是路径）
全程亮着高亮与下划线。已在 `else` 分支补"镜像为 `Some` 则 emit 清除"。
另在 App 侧唯一的 `set_console_open` 写入点无条件清 `rt.console_hover`——面板凭空出现/消失
时（热键、托盘）光标位置未知，overlay 的两条通路覆盖不到。

**P2-2 — 新测试里有一条恒假分支（已修）**

`truncated && clamped` 恒假：`truncated` 为真意味着 `elide_middle` 已把字宽压到预算内
（预算 = 框宽 − 2·s），于是永远不钳位。已改成 `min(字形宽, 框宽)`，并把**长路径**加进输入矩阵
（上一版只有短路径与空路径，"被省略"这条分支从未执行）。

**P2-3 — 下划线仍悬空（已随 P1 一并修）**

锚点用的是估算行高 1.6 em，而实测行高只有 **1.27 em** → 下划线落在字形下方约 0.33 em。
改用实测 `m.height` + 0.06 em 间隙。

**P2-4 — 两处小假（已修）**

`render/src/scene.rs` 称"绘制层不再二次截断"，但 `draw_text` 仍走 `truncate_to_fit`
（App 预留 2·s 所以平时不触发，但那是兜底不是保证）；§6.1 把 4 组（面板宽, DPI）组合
写成了"2 宽 × 3 DPI"全交叉。

---

## 8. 设计修订：移除「打开」按钮，改由路径承担（2026-09-13）

### 8.1 怎么发现的

实施与审查都通过、门禁全绿之后，我做了一次**宽度实测验证**——方法是不跑 GUI、直接量字体：
布局宽度**全部**由 `winbosk_core::text::estimate_width` 推出，所以"会不会挤字/裁字"可以直接测量。
用 Pillow 按同一字体族（`Microsoft YaHei UI`，见 `theme.rs`）实测真实 advance width：

| 字符串 | 估算 | 实测 | 误差 |
| :--- | ---: | ---: | ---: |
| 纯 CJK（`应用内部库` / `恢复默认` / `文件位置` …） | — | — | **0.00%**（YaHei UI 的 CJK 正好 1.0 em） |
| `所有栅栏共用 · 删除会真删文件` | 131.67 | 119.52 | +10.2%（空格按 0.62 em，实际 ≈0.31） |
| `G://…//data//library` | 110.42 | 82.07 | **+34.5%**（ASCII 按 0.62 em，实际 ≈0.5） |

**结论：估算器全线偏保守（偏大），没有一处偏小** → 不会挤字、不会裁字。

**补记（第三轮审查后，用 DirectWrite 复核）**：Pillow 的数是外部工具量的，为排除"量错"，
后来在 `TextFormats::measure_detail` 里用**真 DWrite `CreateTextLayout` + `GetMetrics`**
复核了同一条路径，并用单测钉住（`crates/render/src/draw.rs`：
`measure_detail_is_tighter_than_estimate_for_ascii`）：

| 口径 | `G:\Codes\sylva\target\debug\data\library` @ 8.64 DIP |
| :--- | ---: |
| `estimate_width`（估算） | 214.27 |
| DWrite `GetMetrics`（实测） | **172.05** |
| Pillow `getlength`（实测，交叉验证） | 172.67 |

两种独立实测吻合到 **0.4%**，估算则偏大 **24.5%**。**实测行高 10.97 = 1.27 em**（不是 1.6 em）。

于是立下一条口径规则：**预算用估算（宁大勿小，保证永不裁字），贴字形的绘制用实测。**
顺带一个可选的后续项：ASCII 系数 0.62 比实测宽 ~24%，会让 ASCII 为主的路径被**过度**省略；
但它是全 App 共用的口径，改动影响所有布局，须单独立 plan + 审查，本次不动。

### 8.2 发现的问题

`estimate_width` 偏保守**不会**导致裁字，却会放大"预算不够"的后果。量下来路径的省略预算：

| 版本 | 路径省略预算（`s=1`） | 默认宽 · 35 字中文路径 | 最小宽 |
| :--- | ---: | :--- | :--- |
| 改动前（plan 09 之前） | `d.w − 70·s` = 226 | **完整 35 字** | 22 字 |
| **初版（值行带「打开」按钮）** | `d.w − 151.2·s` = 144.8 | 15 字 | **8 字** |

也就是说：这一行的存在意义就是"告诉用户文件到底在哪"，而初版让它**比改动前少显示一半以上**。
成因拆解（`d.w = 296`）：

- **+37.2·s**：状态标签从 `d.x + 2·s` 搬到值列 `d.x + label_w(40·s)`，路径起点随之右移
  ——**这是本次要修的病，不能退**；
- **+46·s**：初版新加的「打开」幽灵按钮（`estimate_width("打开",12) + 16·s = 40·s`，加 6·s 间距）。

### 8.3 为什么不能简单把「打开」挪到动作行

提示预算实测 AppLibrary **186** / ExternalFolder **116**（`can_reset` 只对外部文件夹为真）。
挪走「打开」要吃掉 46·s → 变 140/70，而外部文件夹的提示 `删除会真删磁盘文件` 需要 **77.8**
→ **会被整条丢掉**。而那句提示正是本行存在的理由（回答"删除是删副本还是删真身"）。
把提示文案砍短来腾地方也能凑出来，但那是拿核心信息换一个次要按钮的位置。

### 8.4 修订内容

**删除「打开」按钮，让路径本身承担这个动作**：

- 值行成为**零按钮的纯信息行**：`文件位置` 标签 + 状态标签 + 路径；
- `storage_path_hit` 接 `OpenStoragePath`（点击 = 在资源管理器里打开该落地目录）；
- **命中区贴实际字形**（`min(estimate_width(path_text), path.w)`），不铺满整段预算——
  否则文字右侧的空白也会吞点击，又变成"看不见却能点"；
- 路径 hover 时**提亮（0.55 → 0.92）+ 在命中区下缘画下划线**：这是本行唯一的"可点"提示，
  必须够明显。它接的动作是**无害且可逆**的"打开目录"，与破坏性的「更改文件位置…」彻底分开
  ——后者仍是动作行上的具名按钮（原始病因正是"点路径会静默搬文件"）；
- `draw_ghost_button` 随之删除（无其它调用点）。

**修订后实测**：路径省略预算回到 `d.w − 105.2·s` = **190.8**，默认宽 35 字路径显示 **28 字**、
最小宽 **15 字**。未回到改动前的 35 字，差额全部来自"状态标签搬进值列"那 37.2·s——那是本次的
核心修复，不可退。

**新增回归断言**：`storage_row_geometry_path_owns_rest_of_value_row`（路径右缘精确落在内缘）
+ `storage_row_geometry_path_hit_hugs_text`（命中区贴字形且不越界）。

### 8.5 教训（写给下一个改这一行的人）

**plan 09 初版的宽度表只算了提示语预算，漏算了路径预算。** 新增任何控件前，必须先把
"这块宽度从谁身上抢"写进宽度表——否则它会在别处悄悄降级，而门禁（编译 / 单测 / clippy / fmt）
**一条都拦不住**：几何自洽、空间互斥、文字不重叠全都成立，只是信息变少了。

---

## 9. 修订记录：CONSOLE_W 320→368 与控制中心字号解耦（源自 plan 05 迭代二）

### 9.1 背景与 Non-Goal 5 的取代

plan 05 第二迭代（控制中心临时置顶，见该文档 §8–§9）把控制中心文字与桌面栅栏解耦并
整体加大两号：`console_label` 12→16，detail 级 8.64→11.52（= 16 × 0.72，比例常量
`DETAIL_SIZE_RATIO` 为全链路唯一真源）。字号变大后 §2.1 的宽度预算全面变化：
「更改文件位置…」100·s→128·s、「恢复默认」64·s→80·s，320 默认宽下后果提示放不下
（两模式分别缺口约 18 / 32px），**违反本计划「默认宽度下提示常显」的主契约**。

Non-Goal 5（不改 `CONSOLE_W` / `CONSOLE_MIN_W` / 行距 30）是**本计划自身**的范围声明
——它禁止的是本计划的改动顺带动宽度，不是对面板宽度的永久冻结。plan 05 迭代二因
字号解耦触发该前提，将 `CONSOLE_W` 320→368（+48）并留下复算记录如下；
`CONSOLE_MIN_W` 与行距 30 不变。§8.5「新增任何控件前先更新宽度表」的教训继续有效。

### 9.2 提示预算复算（`s = 1`，`detail_font = 11.52`，`label_font = 16`）

公式不变（`storage_row_geometry`）：`hint_w = inner_w − change_w − reset_w − 2·gap`
（`can_reset` 为假时 `reset_w = 0`），`inner_w = d.w − 4`，`d.w = 面板宽 − 2·CONSOLE_PAD`。
提示文案宽 = `estimate_width(hint, 11.52)`：「所有栅栏共用 · 删除会真删文件」15.24 单位
= **175.6**；「删除会真删磁盘文件」9 单位 = **103.7**。

| 面板宽 | 模式 | 提示预算 | 提示文案宽 | 结论 |
| :--- | :--- | ---: | ---: | :--- |
| `CONSOLE_W = 368`（默认） | 应用内部库 | 206 | 175.6 | **常显**（余量 30） |
| `CONSOLE_W = 368` | 外部文件夹 | 120 | 103.7 | **常显**（余量 16） |
| `CONSOLE_MIN_W = 260` | 应用内部库 | 98 | 175.6 | 差 77.6 → **整条不画** |
| `CONSOLE_MIN_W = 260` | 外部文件夹 | 98\* | 103.7 | 差 5.7 → **整条不画** |

\* 最小宽下有一处**行为变化**：新字号使「恢复默认」的出现条件
（`reset.x ≥ inner_left + label_w + gap`）不再满足——按钮在 260 宽下退场（零矩形，
`actions_left` 退回 `change.x`），与代码既有注释「面板被拖到极限时宁可不出这个次要
按钮」的取舍一致，提示预算因此比「按钮在场」口径多出 86。降级方向（次要控件先消失）
与 §2.1 的设计原则相同，不构成回归。

勘误：plan 05 迭代二实施中 main.rs `CONSOLE_W` 注释初稿曾写「余量 22 / 16px」，其中
「22」系算误——实算 206 − 175.6 = **30.4**，已随本修订改正为 30；缺口 18 / 32 与余量
16 复核无误。

### 9.3 路径预算复算（§2.1 路径表追加一行，历史行保留）

状态标签宽 `tag_w = 5 × 11.52 + 12 = 69.6`（原 55.2），路径起点右移 14.4；面板加宽
48。路径省略预算 = `d.w − 119.6·s`：

| 版本 | 路径省略预算 | 默认宽 · 35 字路径 | 最小宽 |
| :--- | ---: | :--- | :--- |
| **现版（plan 05 迭代二后）** | `d.w − 119.6·s` = **224.4** | 约 24 字（等宽换算） | 约 9 字 |
| 上一版（plan 09 收尾口径，历史保留） | `d.w − 105.2·s` = 190.8 | 28 字 | 15 字 |

像素预算比上版宽 33.6，但单字成本涨 33%（8.64→11.52），CJK 等效字容量 22.1→19.5 略降。
**「路径吃满值行剩余宽度」的契约不变**（`path_w = inner_right − path_x` 恒成立，回归断言
`storage_row_geometry_path_owns_rest_of_value_row` 继续钉住）。等宽换算说明：elide_middle
按「叶子完整 > 层级数 > 盘符」贪心，可显示字数与总字数非严格线性，表中字数为同一路径
按新旧字号的等宽换算估算。

### 9.4 契约复核结论

- **默认宽度下提示常显** ✓：368 宽两模式余量 30 / 16（§9.2 表）。
- **路径吃满值行剩余宽度** ✓：§9.3，回归断言锚定。
- **值行零按钮 / 路径热区只接 `OpenStoragePath` / 动作行具名按钮**：本修订未触碰。
- **验证机制**：`scene.rs` 的宽度预算不变量测试以 `CONSOLE_W` / `CONSOLE_MIN_W`
  参数化——常量一改即被重验；本修订后 `cargo test --workspace` 全绿。
- 顺带记录：plan 09 历史章节引用的 `TextFormats::measure_detail` 及测试名
  `measure_detail_is_tighter_than_estimate_for_ascii` 已随 plan 05 迭代二更名为
  `measure_console_detail` /
  `measure_console_detail_is_tighter_than_estimate_for_ascii`（语义不变，实测口径现在
  明确对准 `console_detail` 字号）；历史章节按原貌保留，不回改。
