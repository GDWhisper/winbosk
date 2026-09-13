# 控制中心「文件位置」可见性与可回退实施计划

本实施计划覆盖栅栏存储语义的**知情化改造**：把此前完全不可见的 `Fence::storage_path` 状态
（当前值 / 当前模式 / 是否可回退）显式呈现于控制中心栅栏详情区，并提供「恢复默认」动作，
解除「设置过路径即永久回不到内部库」的单向门。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

1. **当前值可见**：详情区「存储」行更名为「文件位置」，并新增第二行显示**真实落地目录**的
   中段省略文本（如 `…\debug\data\library`），而非自造术语。
2. **当前模式可见**：以状态芯片区分两种**机制不同**的落地模式——「应用内部」（栅栏索引内部库
   副本）与「外部文件夹」（栅栏与磁盘目录双向镜像）。二者不是程度差别，必须可辨。
3. **可回退**：新增「恢复默认」动作，把外部文件夹模式还原为应用内部模式，消除单向门。
4. **一致性修正（承重）**：详情区高度预算改为按**当前选中栅栏**计算，而非按"第一个非空栅栏"
   猜测。本改造给存储行增加一行高度，若高度预算仍取另一栅栏，会出现按钮压住详情内容。

### Non-Goals

1. 不改动 `Fence::storage_path` 的字段类型与 `desk.json` 序列化格式（零迁移，旧配置直接可用）。
2. 不改动删除语义本身（`is_managed_path` 的判定规则、删除确认弹窗文案）——留作后续独立计划。
3. 不改动面板默认宽度 `CONSOLE_W`，不新增任何常驻定时器或后台轮询。
4. 不引入工具提示（tooltip）基础设施：路径直接以第二行常显，避免 hover 才可见。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

### 2.1 核心层：纯展示模型（新增 `crates/core/src/storage.rs`）

零 OS 依赖、纯函数，可在内存中瞬时单测。

```rust
/// 栅栏文件落地模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// `storage_path == None`：应用数据目录下的共享 library（栅栏索引副本）。
    AppLibrary,
    /// `storage_path == Some(dir)`：与磁盘目录双向镜像（栅栏即文件夹）。
    ExternalFolder,
}

impl StorageKind {
    /// 状态芯片文案。
    pub fn badge(self) -> &'static str;
}

/// 控制中心「文件位置」行的渲染数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageInfo {
    pub kind: StorageKind,
    /// 真实落地目录（绝对路径；内部模式 = 共享库目录）。
    pub path: String,
    /// 是否允许「恢复默认」。
    pub can_reset: bool,
}

/// 由 `storage_path` 与两个上下文目录推导展示信息。
pub fn describe(
    storage_path: Option<&str>,
    library_dir: &str,
    desktop_dir: Option<&str>,
) -> StorageInfo;

/// 中段省略：保留尾部（叶子名优先）与可选的盘符前缀，宽度口径 = `text::estimate_width`。
pub fn elide_middle(path: &str, budget: f32, font_size: f32) -> String;
```

**契约要点**：

- `can_reset == true` 当且仅当 `kind == ExternalFolder` **且** 该路径不是**桌面源目录**
  （`desktop_dir`）。桌面栅栏一旦解除链接会让栅栏清空，而真实桌面图标正被壳层接管隐藏，
  用户会看到"桌面全空"，必须禁止。
- 目录比较为**大小写不敏感 + 忽略尾部分隔符**（`same_dir` 私有辅助），不触碰文件系统。
- `elide_middle` 只按 `char` 边界切分，绝不切断多字节字符；预算 ≤ 0 返回空串；
  能整段放下则原样返回。

### 2.2 渲染层契约（`crates/render/src/scene.rs`）

`SceneFenceDetail` 新增字段（几何字段同时供绘制与命中模型使用，二者必须同源）：

```rust
/// 落地模式（决定状态芯片文案与配色）。
pub storage_kind: StorageKind,
/// 中段省略后的路径文本（App 层按当前 DPI 预算预计算，绘制层不再二次截断）。
pub storage_path_text: String,
/// 「恢复默认」按钮；`h <= 0.0` 表示不可回退（不绘制、不参与命中）。
pub storage_reset: RectF,
/// 路径行整体（可点击 = 更改位置…）。
pub storage_path_row: RectF,
```

### 2.3 命中契约（`crates/render/src/overlay.rs`）

`ConsoleZone` 新增一个变体：

```rust
/// 栅栏管理页：把选中栅栏的存储位置恢复为应用内部库（解除外部链接）。
ResetStoragePath,
```

路径行点击**复用**既有 `ChangeStoragePath`，不新增变体（语义完全相同）。

### 2.4 度量口径

- 字体：沿用 `formats.detail`（`theme.label.size * 0.72`，已按 DPI 缩放）。
- 省略预算：`path_w - 4.0 * s`（物理像素），与 `draw_text` 的 `estimate_width` 口径一致。
- 存储行占 **2 个行槽**（行高 30 × s），第 2 行位于 `row_y(r) + 26 * s`，高 `18 * s`。

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

1. **常驻后台冲突律 (Daemon Invariant)**
   - 后台 `SyncLibrary` 周期任务按 `fence.storage_path` 决定是否执行 `mirror_linked_fence`。
   - 「恢复默认」把 `storage_path` 置为 `None` 后，该栅栏**立即退出**镜像集合，后台不再为它
     枚举目录，不存在"局部视角缺失 → 循环重注册"路径。
   - 关键约束：解除链接时**不得**移动任何磁盘文件（详见 §4.2），否则与后台 4s 周期的
     集合比对形成竞争，会出现注册/注销抖动。

2. **空闲性能归零律 (Zero-Idle Invariant)**
   - 本改造纯几何 + 纯文本，不新增定时器；`elide_middle` 为 O(路径长度) 的字符串计算，
     仅在 `build_console` 时执行一次（面板展开期间每帧一次，成本可忽略）。
   - 无状态变更时不置 `redraw`。

3. **孤儿状态回收律 (Orphan Invariant)**
   - 解除链接后，原外部文件夹内的文件仍受**文件系统**管理，且不再属于任何管理区
     （`is_managed_path` 变为 false）。栅栏中原本镜像自该文件夹的成员必须被摘除，
     否则会留下"栅栏项仍在、但已不在管理区"的幽灵项——即删除它不再删文件，
     与用户认知冲突。摘除走既有 `remove_icon_entirely`，只删引用不删文件。

4. **语义精准定位律 (Semantic Invariant)**
   - 禁止用 `fences.first()` / "第一个非空栅栏"推断详情区行数。本计划将
     `detail_visible_rows` 的入参由"猜测的栅栏"改为**选中下标**（`rt.selected_fence`），
     与 `build_console` 渲染的栅栏严格同源。

5. **状态全集校验律 (Compound State)**
   - 「恢复默认」按钮的可交互性由 `StorageInfo::can_reset` 复合判定
     （`ExternalFolder && !desktop_source`），不得仅凭 `storage_path.is_some()` 判定。
   - `can_reset == false` 时按钮几何为零矩形，绘制与命中模型同时忽略，杜绝死按钮。

6. **旁路数据对齐律 (Sidecar Invariant)**
   - 存储行由 1 行变 2 行：`detail_visible_rows` 的行数计数（`n += 2`）与
     `build_console` 的 `row += 2` 必须在同一提交内同步修改。
   - **实测澄清**：底部按钮的 `btn_top` 由 `detail_visible_rows` 推出，详情区底边**并不绘制**，
     `CONSOLE_FENCE_DETAIL_H` 只被写进 `SceneFenceDetail.rect.h`，绘制与命中均不读它
     （`draw.rs` 仅用 `d.rect` 的 x/y/w）。因此真正保证布局正确性的是
     `detail_visible_rows` 与 `build_console` 的同源；该常量仍提升至 278（= 24 + 8 × 30 + 14），
     作为最坏情况（8 行）的**书面上界**与后续绘制底板的预留，避免常量与真实行数脱节。
   - `desk.fences` 增删时无需额外处理：本改造不新增与栅栏平行的旁路容器。

---

## 4. 分层改动清单 (Implementation Steps)

### 4.1 核心层（`crates/core/`）

- **[`crates/core/src/lib.rs`](file:///g:/Codes/winbosk/crates/core/src/lib.rs)**：注册 `pub mod storage;`。
- **[`crates/core/src/storage.rs`](file:///g:/Codes/winbosk/crates/core/src/storage.rs)**（新增）：
  `StorageKind` / `StorageInfo` / `describe` / `elide_middle` / `same_dir` + 单元测试
  （见 §6）。依赖 `crate::text::estimate_width`，零 Win32。

### 4.2 驱动/渲染层（`crates/render/`）

- **[`crates/render/src/scene.rs`](file:///g:/Codes/winbosk/crates/render/src/scene.rs)**：
  `SceneFenceDetail` 新增 4 个字段（§2.2），并更新 `storage_btn` 注释为「更改位置…」。
- **[`crates/render/src/overlay.rs`](file:///g:/Codes/winbosk/crates/render/src/overlay.rs)**：
  `ConsoleZone` 新增 `ResetStoragePath`（§2.3）。
- **[`crates/render/src/draw.rs`](file:///g:/Codes/winbosk/crates/render/src/draw.rs)**：
  - 行标签 `"存储"` → `"文件位置"`（`formats.detail` 下 4 个 CJK 字宽 34.6 DIP < `label_w` 40，
    无需改动 `label_w`，其余各行几何完全不动）；
  - 第一行：`更改位置…`（既有 120 × s）+ `恢复默认`（新增 84 × s，`h <= 0.0` 时跳过）；
  - 第二行：状态芯片（`storage_kind.badge()`，圆角矩形 + 描边，宽 56 × s）+ 路径文本
    （`storage_path_text`，`formats.detail`，颜色弱化）；
  - 芯片配色：`ExternalFolder` 用 accent 描边（提示"文件在你自己选的目录里"），
    `AppLibrary` 用中性灰，避免误导为"错误状态"。

### 4.3 组装/交互层（`crates/app/`）

- **[`crates/app/src/main.rs`](file:///g:/Codes/winbosk/crates/app/src/main.rs)**：
  - `CONSOLE_FENCE_DETAIL_H`：248.0 → 278.0；
  - `ConsoleClick` 分派新增 `ConsoleZone::ResetStoragePath` → `reset_fence_storage(rt, i)`。
- **[`crates/app/src/scene.rs`](file:///g:/Codes/winbosk/crates/app/src/scene.rs)**：
  - `detail_visible_rows` / `console_full_height` / `console_geometry` 增加 `selected: usize`
    入参，改按选中栅栏取 `layout` / `bg_style`（修正既有错配）；
  - `detail_visible_rows` 中「更改位置」由 `n += 1` 改为 `n += 2`；
  - `build_console` 的存储行改为两行布局（§2.4），并计算
    `winbosk_core::storage::describe(...)` 与 `elide_middle(...)`；
  - 命中区新增 `(ResetStoragePath, d.storage_reset)` 与
    `(ChangeStoragePath, d.storage_path_row)`；零矩形不入表。
- **[`crates/app/src/file_ops.rs`](file:///g:/Codes/winbosk/crates/app/src/file_ops.rs)**：
  - 新增 `reset_fence_storage(rt, fence_idx)`：解除外部链接（见下）；
  - `clear_stale_linked_items` 的 `new_path: &Path` 改为 `Option<&Path>`（`None` = 摘除该目录内全部成员）；
  - `change_fence_storage` 增加**防御守卫**：拒绝把存储位置设为内部库本身、其子目录或其祖先目录
    （`path_within` 双向 + 等值比较，大小写不敏感）。当前仅校验 `is_dir()`，
    用户若选中 `<exe>\data\library`，`mirror_linked_fence` 会把**整个共享库**（含其它栅栏的项）
    镜像进该栅栏，产生跨栅栏重复项。此守卫为**超出所选范围的附带修复**，如不需要可直接回退。

**`reset_fence_storage` 语义（明确契约）**：

1. 取 `old_dir = fence.storage_path`；若为 `None` 或等于桌面源目录 → 直接返回（不改状态）；
2. `fence.storage_path = None`；
3. `clear_stale_linked_items(rt, idx, old_dir, None)`：把路径位于 `old_dir` 内的 `added` 项
   从栅栏摘除（`remove_icon_entirely`，**磁盘文件一个不动**）；
4. `store.save(&rt.desk)` 持久化。
5. 不移动、不复制、不删除任何文件；不触发 `reconcile_fences`。

### 4.4 文档

- **[`AGENTS.md`](file:///g:/Codes/winbosk/AGENTS.md)**：在「领域术语表」补充
  **Storage Mode（存储模式）** 条目（应用内部 / 外部文件夹）与「恢复默认」语义，
  使后续 Agent 不再把该字段误判为"磁盘容量"。

### 4.5 对抗性审查后的追加修复（超出原始范围，随本次一并交付）

独立子代理审查（见 §7）暴露了三处与本改动**直接耦合**的既有缺陷，均已修复：

1. **位图槽碰撞（`file_ops.rs`）**：`register_fence_item` 原先用 `rt.items.len()` 当位图槽号。
   `remove_icon_entirely` 会从 `items` 池移除元素并重建下标，之后再注册就会与既有槽号重合，
   新项覆盖旧项的图标位图。「恢复默认」正是**批量删除**入口，紧接着用户拖回文件即触发，
   故改为与 `editing.rs` 改名路径同一口径的单调分配：
   `rt.bitmap_ids.values().copied().max().unwrap_or(0) + 1`。
2. **跨栅栏归属保护（`clear_stale_linked_items`）**：两个栅栏的存储位置互为父子目录时，
   解除其中一个链接会把另一个栅栏的成员一并注销，≤4s 后被它的后台镜像重新注册，
   表现为图标反复消失/重建。新增 `claimed_by_other_fence` 判定，落在其它栅栏链接文件夹内的
   成员不摘下。该保护对 `change_fence_storage` 的换链接路径同样生效。
3. **桌面目录缓存（`Runtime::desktop_dir`）**：`build_console` 原先每帧调用一次 COM
   `SHGetKnownFolderPath`（`shell_desktop_path()`）来判定「桌面镜像栅栏不可恢复默认」。
   改为启动时解析一次存入 `Runtime`，控制中心绘制读缓存；迁移/镜像等低频路径仍用原函数。

**未采纳的审查意见**：审查者建议在 `change_fence_storage` 中拒绝"与另一栅栏存储位置重叠"的
选择。未采纳——该守卫只写日志、无 UI 反馈，会变成静默无操作的坏体验；改由上述归属保护
（第 2 条）在**摘除侧**兜底，既消除跨栅栏误注销，又不引入新的静默失败点。

---

## 5. 防御性自查清单 (Defensive Invariants)

- [x] **常驻后台冲突律**：解除链接仅改配置，不移动文件 → 与 `SyncLibrary` 无写竞争；
      置 `None` 后该栅栏立即退出镜像集合。
- [x] **空闲性能归零律**：零新增定时器；`elide_middle` 为纯字符串计算。
- [x] **孤儿状态回收律**：解除链接时摘除原文件夹内的镜像成员，杜绝"已不在管理区却仍在栅栏"的幽灵项。
- [x] **语义精准定位律**：详情区高度改为按 `selected_fence` 计算，不再用"第一个非空栅栏"猜测。
- [x] **状态全集校验律**：`can_reset = ExternalFolder && !desktop_source`；桌面栅栏禁止解除链接。
- [x] **旁路数据对齐律**：`n += 2` 与 `row += 2` 同提交修改；`CONSOLE_FENCE_DETAIL_H` 同步提升至 278。
- [x] **零迁移**：不改字段类型、不改 `desk.json` 结构，旧配置直接可用。
- [x] **越界防御**：`fence_idx` 一律经 `min(len.saturating_sub(1))` 收敛；`clear_stale_linked_items`
      入参由 `&Path` 变 `Option<&Path>` 后，`None` 分支不构造任何路径。

---

## 6. 验证与交付门禁 (Verification Gates)

1. **单元测试（`cargo test -p winbosk-core`）** —— `storage::tests`：
   - `describe_none_is_app_library`：`None` → `AppLibrary`，`path == library_dir`，`can_reset == false`；
   - `describe_some_is_external_and_resettable`：普通外部目录 → `ExternalFolder` + `can_reset == true`；
   - `describe_desktop_source_is_not_resettable`：路径等于 `desktop_dir` → `can_reset == false`；
   - `describe_dir_compare_is_case_and_separator_insensitive`：`C:\Foo\` vs `c:\foo` 视为同目录；
   - `elide_keeps_leaf`：长路径省略后必以叶子名结尾且含 `…`；
   - `elide_fits_returns_unchanged`：短路径原样返回；
   - `elide_never_splits_multibyte`：中文路径省略结果为合法 UTF-8 且字符数不增；
   - `elide_zero_budget_is_empty`：预算 0 → 空串（零输入场景不 panic）。
2. **幂等性**：连续两次「恢复默认」→ 第二次为 0 变动（`storage_path` 已为 `None`，提前返回）。
3. **静态质量门禁**（CI 阻断项）：
   ```powershell
   cargo build --workspace
   cargo test --workspace
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```
4. **真实走查**：`$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe`（干净退出，勿硬杀）。
   逐项确认：
   - 默认桌面栅栏显示「外部文件夹」+ 真实桌面路径，且**无**「恢复默认」按钮；
   - 新建空白栅栏显示「应用内部」+ `…\data\library` 路径；
   - 「更改位置…」选一个空目录 → 徽标变「外部文件夹」且出现「恢复默认」；第二行路径随之更新；
   - 点「恢复默认」→ 回到「应用内部」，按钮消失，磁盘文件未被动过；
   - 在文件夹里新增/删除文件，≤4s 反映到栅栏；解除链接后不再跟随。
5. **独立上下文子代理审查**：实施完成后派发无对话历史的子代理，重点对抗性走查
   `reset_fence_storage` 的孤儿清理、`clear_stale_linked_items` 的 `Option` 分支、
   高度预算与几何同源、以及 `SyncLibrary` 后台循环的交互。

---

## 7. 对抗性审查结论（已执行）

派发独立干净上下文的子代理完成对抗性走查，逐条结论：

| # | 审查点 | 结论 |
| :-- | :--- | :--- |
| 1 | `reset_fence_storage` 孤儿清理 | **确认缺陷**：跨栅栏重叠存储位置时会误摘另一栅栏成员 → 已修（§4.5-2）。另发现位图槽碰撞 → 已修（§4.5-1）。`added == false` 的桌面项被正确排除；父目录不会被 `path_within` 误判；无漏摘。 |
| 2 | 后台 `SyncLibrary` 交互 | **无问题**：置 `None` 后 `mirror_linked_fence` 首行即返回，该栅栏退出镜像集合；无重注册、无条目膨胀、无常刷。附带发现每帧 COM 调用 → 已修（§4.5-3）。 |
| 3 | 高度预算与几何同源 | **逐行核对一致**（`row +=` 8 处与 `detail_visible_rows` 完全对应，最坏 8 行 = 264 ≤ 278）。但 `CONSOLE_FENCE_DETAIL_H` 为死几何 → 已在 §3.6 澄清。 |
| 4 | 最小宽度溢出/重叠 | **无问题**：`panel.w = 260` 时 `d.w = 236`，`恢复默认` 被 clamp 到 68，右缘 234 ≤ 236；路径区宽 170 > 0。 |
| 5 | `elide_middle` 正确性 | **无问题**：无字节切片，不 panic、不切多字节，终串宽度 ≤ 预算。 |
| 6 | 零 OS 依赖 | **无问题**：`storage.rs` 仅依赖 `crate::text`，`core/Cargo.toml` 无 `windows`。 |
| 7 | 幂等/重入 | **无问题**：第二次点击提前返回且不落盘；桌面栅栏两道防线；`fence_idx` 收敛到位。 |
| 8 | 旁路数据对齐 | **无问题**：zones 与 `SceneFenceDetail` 同源，零矩形不进命中表，无"可见不可点/可点不可见"。 |

**审查未覆盖、留待人工确认的一点**：`恢复默认` 的文案暗示"把文件搬回来"，实际语义是
"解除链接并把原本镜像自该文件夹的成员从栅栏摘除（磁盘文件留在原处）"。若用户预期是
"搬回内部库"，应改文案为「解除链接」或改为真正的反向迁移。**当前按"解链 + 摘除"实现**，
如需反向迁移请另行提出。
