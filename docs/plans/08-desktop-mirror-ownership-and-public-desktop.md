# 桌面镜像栅栏「内容为空」根因修复与桌面全量镜像 实施计划

**缘起（实证）**：debug 构建启动后「桌面」侧边栏栅栏**恒为空**，控制中心「文件位置」行显示
`外部文件夹 C:\Users\vacio\Desktop`。实测 `target/debug/data/desk.json`：

```
fences = 1  (title=桌面, layout=Sidebar, storage_path=C:\Users\vacio\Desktop, icon_ids = 0)
icons  = 155 个，其中「归属任一栅栏 / 未分组区」的 = 0 个
```

即：不但栅栏空，**所有桌面图标在屏幕上都不存在**（真实桌面图标已被 `IconGuard` 隐藏，而无归属的
图标在渲染层没有任何绘制入口）。集合核对给出直接原因：

```
磁盘上应镜像的条目(105) == 全局池口径的「已存在」(105)   → 提前 return，一条都注册不进来
```

**根因**：`mirror_linked_fence` 对「桌面源栅栏」的 `existing` 集合取的是**全局图标元数据池**
（`rt.desk.icons`），而启动时**枚举到的每一项都被无条件写进该池**（`main.rs:450-461`，无论有没有
栅栏归属）。于是「池里有」被当成「栅栏里已有」，注册循环被 `file_ops.rs:504-506` 提前 return 掐断；
`seed_fences` 又刻意把 `icon_ids` 留空等同步来填 → 栅栏永远为空。普通目录镜像栅栏没有这个问题，
因为它取的是**本栅栏成员**（`file_ops.rs:476-490`）——桌面分支是唯一的例外。

**ground truth（真实桌面到底显示什么）**：只读读取 explorer 的桌面 `SysListView32`
（`scripts/desktop-probe.py`）得到 **140 项** = 用户桌面文件 105 + 公共桌面文件 34 + 虚拟项 **1**
（只有「回收站」）。据此，本次修复的目标是让栅栏内容与这 140 项一致。

---

## 1. 目标与非目标 (Goals & Non-Goals)

### Goals

1. **桌面镜像栅栏按「归属」判重**：`existing` 改为「已被任一栅栏持有的桌面文件」，未归属的桌面文件
   必须被注册进栅栏。修复后首次启动，105 个用户桌面项立即入栏（数据自愈，用户无需删 `desk.json`）。
2. **桌面镜像的源目录 = 用户桌面 + 公共桌面**：Windows 桌面上显示的是两者并集（用户桌面 105 项 +
   公共桌面 34 项）。当前只扫用户桌面，那 34 个快捷方式（Chrome / Firefox / NVIDIA App…）在真实
   桌面被隐藏后无处可去，等于凭空消失。
3. **保持「一键整理」的防回流语义**：被分类整理进其它栅栏的桌面文件**不得**被镜像抢回桌面栅栏
   （`existing` 必须覆盖其它栅栏的持有）。
4. **保持既有廉价快路径**：目录集合与栅栏成员一致时，每 4s 的 `SyncLibrary` 只做 `read_dir`+集合
   比较，不进注册/移除分支；空闲仍 0% CPU、无额外 I/O。
5. **孤儿可回收**：公共桌面里的文件被外部删除后，栅栏里的对应图标必须被移除（不能只认用户桌面）。

### Non-Goals

1. **不引入「虚拟壳项」的镜像**（此电脑 / 网络 / 控制面板 / 用户文件夹 / 库 / 图库 / Linux…）。
   实测这些项的 shell 属性 **不置 `SFGAO_HIDDEN`/`GHOSTED`**，即无法用属性区分「Windows 是否把它
   显示在桌面上」；而它们在栅栏里是**死图标**——`DesktopItem::launch()` 对无路径项直接返回
   （`items.rs:50-53`），双击无任何反应。要不要做（可见性规则 + 虚拟项打开支持）是独立议题。
   **唯一因此缺失的是「回收站」1 项**（ground truth 里唯一的虚拟项），见 §6.3。
2. **不改「移出栅栏」的既有语义**：桌面项移出 = 回未分组区（`main.rs:1653-1655`），而镜像 ≤4s 会把
   它加回来 → 对桌面镜像而言「移出」等效于无效操作。这与 `shell_menu.rs:111-112` 既有的
   「链接栅栏即文件夹镜像，移出 = 删除」一致，本次仅**如实记录**，不做破坏性改动
   （把「移出」改成删文件是另一个决定，必须用户显式拍板）。
3. **不改 `storage_path` 语义**：桌面镜像栅栏的 `storage_path` 仍只有用户桌面一个目录
   （新文件落盘位置 / 删除语义 / 控制中心展示都依赖它）。公共桌面只进「扫描集合」，不进
   `storage_path`；因此公共桌面项**不属于** WinBosk 管理区（`is_managed_path`），右键「删除」走
   Shell 原生动词（回收站）而非 WinBosk 直删，符合安全方向。
4. **不新增配置项、不改数据结构、不动 `desk.json` 格式**。
5. **不动渲染层/命中模型**：本计划只改 app 层同步口径。

---

## 2. 状态契约与接口设计 (Contracts & Data Models)

无新增持久化结构。新增/改写的是 app 层**纯函数**（可内存单测）：

```rust
// crates/app/src/scene.rs —— 壳层路径解析（与 shell_desktop_path 同风格）
/// 公共桌面目录（`FOLDERID_PublicDesktop`）。不存在/不可用 → None。
pub(crate) fn shell_public_desktop_path() -> Option<String>;

// crates/app/src/file_ops.rs —— 纯逻辑
/// 目录去重追加：大小写不敏感、忽略尾部分隔符（纯函数，便于单测）。
fn push_unique_dir(dirs: &mut Vec<PathBuf>, extra: PathBuf);

/// 桌面镜像的源目录集合 = 用户桌面 + 公共桌面（后者可用且不重复时）。
/// shell 解析结果由 `locals` 注入 → 纯函数可测。
fn desktop_source_dirs(user: &Path, public: Option<PathBuf>) -> Vec<PathBuf>;

/// 镜像栅栏的「已存在」路径集合（小写，限定在 `roots` 内）。
/// `is_desktop` = 桌面镜像栅栏 → **归属口径**；否则 → 本栅栏成员口径。
fn mirror_existing_paths(desk: &Desk, idx: usize, roots: &[PathBuf], is_desktop: bool)
    -> HashSet<String>;

/// 读一个镜像源目录下应当镜像的条目（跳过隐藏/系统，与资源管理器默认一致）。
fn read_mirror_entries(dir: &Path) -> Vec<PathBuf>;
```

口径定义（**必须与注释一字不差地落地**）：

| 场景 | `mirror_existing_paths` 语义 | 为什么 |
| :--- | :--- | :--- |
| 桌面镜像栅栏 | 「**归属**口径」：`fences[].icon_ids` 中路径落在 `roots` 内的项 | 池子里有 ≠ 栅栏里有；池在启动时被无条件灌满，用它判重等于永远「已归属」 |
| 普通目录镜像栅栏 | 「本栅栏成员」口径（现状不变） | 严格维持当前成员，不受其它栅栏影响 |

- **未分组区（`free_icons`）不计入归属**：它在渲染层**没有任何绘制入口**，把它当归属会让「移出
  栅栏」的桌面项彻底消失在用户视野里（真实桌面已被接管隐藏）。不计入 → 镜像会把它加回栅栏，
  最坏结果是「移出无效果」，优于「图标凭空消失」。
- 单位口径：全部是**小写化的绝对路径字符串**（与 `item_id` 同口径），大小写不敏感比较由
  `path_within`（组件级）与 `to_ascii_lowercase` 共同保证。

### 2.1 快路径判据：两条子集断言，**不是**集合相等

```rust
/// 一次 `read_dir` 产出两套磁盘视图：
/// - `all`    ：目录列出的全部条目（含隐藏/系统，如 desktop.ini）→ 已归属项是否还在磁盘上；
/// - `visible`：应当镜像的条目（`should_mirror` 过滤掉隐藏/系统）→ 有没有新项要注册。
fn mirror_converged(owned: &HashSet<String>, all: &HashSet<String>, visible: &[PathBuf]) -> bool {
    owned.is_subset(all)                                          // 无孤儿
        && visible.iter().all(|p| owned.contains(&lower(p)))       // 无新项
}
```

**禁止改成 `visible == owned`**（对抗性审查抓到的 P1）：某项被外部加上隐藏/系统属性后，它仍在
`all` 里、却永远不在 `visible` 里；只要它已归属，`visible != owned` 就**永久**成立 → 每 4s 都白跑
一遍注册循环 + 全量 `Path::exists()` 探测，违反空闲 0% CPU 铁律（实测 139 项）。两条子集断言各自
只关心自己那一侧，且都只用 `read_dir` 的结果（**零额外系统调用**）。反过来说，`owned` 也**不能**
按 `should_mirror` 过滤：那样会把「文件已被外部删除」的项一并滤掉，孤儿回收分支永远不会触发。

---

## 3. 隐性机制与系统动力学推演 (Hidden Dynamics Analysis)

### 3.1 与 4s 同步引擎（`SyncLibrary`）的交互

`reconcile_fences`（`file_ops.rs:432-440`）→ 逐栅栏 `mirror_linked_fence`。改动后每周期仍是
**两个** `read_dir`（用户桌面 + 公共桌面）而不是一个；收敛即 `mirror_converged` 返回 true、
提前 return，**不进入**注册/移除分支。139 项的集合构造是纯内存操作，属既有量级，且**不额外
产生系统调用**（两套磁盘视图来自同一次 `read_dir`）。

**必须保住快路径**：判据见 §2.1。收敛态推导：注册完成后，`roots` 内每个可见条目都被某栅栏持有
（`visible ⊆ owned`），且每个已归属项都还在磁盘上（`owned ⊆ all`）。✔

### 3.1.1 与文件属性变化的交互（P1 防御）

用户在资源管理器里给桌面文件勾上「隐藏/系统」，或第三方工具改写属性时：

- 该文件**仍会被回收判定看到**（在 `all` 里）→ 不会被误删；
- 它**不会被注册**（不在 `visible` 里）——与资源管理器默认视图一致（WinBosk 不显示隐藏桌面文件）；
- 它若**此前已归属**，则会留在栅栏里（不主动摘除）——这是可见性属性的变化，不是文件消失，
  不触发任何回收动作；收敛后同步立即回到纯 `read_dir` 快路径（**零抖动**）。

### 3.2 与「一键整理」（`auto_organize_all`）的交互（防回流）

一键整理把桌面栅栏（源）的项搬进各分类栅栏 → 这些项**被其它栅栏持有** → 落在 `existing` 里 →
镜像**不会**把它们抢回桌面栅栏。这正是原先用「全局池」想要达到的效果，用**归属**表达才正确
（池会连「还没归属的项」一起误判）。

### 3.3 孤儿回收（外部删除回流）

`roots` 内的文件被外部删除 → 其图标 `path` 仍在 `desk.icons` 里 → `existing` 含它但 `entries`
不含 → 集合不等 → 走移除分支：`roots` 内任一路径不存在即 `remove_icon_entirely`（含其它栅栏里
的同类项，现状语义不变，只是把「在 `dir` 内」放宽为「在任一根目录内」）。公共桌面的删除因此同样
能被回收——这是 Goals#5。

### 3.4 空闲性能归零律

无新增定时器、无新增补间、无新增写盘（有实质变化才 `store.save`）。空闲仍是纯阻塞 `GetMessageW`。
唯一增量是每 4s 多一次 `read_dir(公共桌面)`（34 项）。

### 3.5 旁路数据对齐律

新增图标仍走既有 `register_fence_item`：位图槽单调递增分配、`items`/`item_index`/`bitmap_ids`
同事务更新（`file_ops.rs:94-114`）。本次只新增一条：注册成功时把该 id 从 `free_icons` 摘除，
避免「同一项既是未分组又在栅栏里」的脏状态。

### 3.6 语义精准定位律

- 桌面判定不改语义：仍是「`storage_path` 等于用户桌面目录」（新增大小写/尾分隔符归一化比较）。
- 公共桌面**不允许**成为 `is_desktop` 的判据（否则用户把某个栅栏链接到公共桌面目录时会被误判为
  桌面镜像）。判据只看用户桌面目录。
- `roots` 去重：当公共桌面与用户桌面是同一目录（域环境/极端重定向）时只扫一次。

---

## 4. 分层改动清单 (Implementation Steps)

依赖拓扑自底向上；本次不触及 core / render（无越层）。

### 4.1 `crates/app/src/scene.rs`

- 新增 `shell_public_desktop_path()`（`FOLDERID_PublicDesktop`，`is_dir` 校验，失败返回 `None`），
  与既有 `shell_desktop_path()` 并列。

### 4.2 `crates/app/src/file_ops.rs`

- 新增 `push_unique_dir` / `desktop_source_dirs` / `list_dir_entries` / `should_mirror`（既有）/
  `mirror_existing_paths` / `mirror_converged` / `dir_eq` 等函数（纯函数 + 注入式 shell 结果）。
- `mirror_linked_fence`：
  1. `let roots = if is_desktop { desktop_source_dirs(&dir, shell_public_desktop_path().map(PathBuf::from)) } else { vec![dir.clone()] };`
  2. 一次遍历 `roots` 产出两套磁盘视图 `all_set` / `visible`（`read_dir` 只跑一次）；
  3. `owned` 改走 `mirror_existing_paths`（桌面 → 归属口径；普通 → 本栅栏成员）；
  4. 快路径改用 `mirror_converged(&owned, &all_set, &visible)`（见 §2.1）；
  5. 注册循环遍历 `visible`，移除分支的「在 `dir` 内」判定改为「在任一 `root` 内」；
  6. `rehome_linked_library_items` 仍只针对 `dir`（库内项迁到栅栏自己的落盘目录，公共桌面不接收）。
- `register_fence_item`：推入 `icon_ids` 时同步 `free_icons.retain`（见 §3.5）；
  「是否桌面镜像栅栏」的判定由裸字符串相等改为 `dir_eq`（与其它两处同口径）。
- `reset_fence_storage`：原先自带的 `same_as` 闭包改用 `dir_eq`（判据唯一）。

### 4.2.1 `crates/core/src/storage.rs`

- `same_dir` 增加分隔符归一化（`/` → `\`），与 App 层 `dir_eq` 同口径。否则当 `storage_path`
  用正斜杠保存时，控制中心会**显示**一个「恢复默认」按钮，而点击后必然被
  `reset_fence_storage` 的守卫拒绝（死按钮）。

### 4.3 `AGENTS.md`

- 「架构约束与为什么」顺延补 **13**：桌面镜像栅栏的 `existing` 是**归属口径**，**禁止**改回读全局
  图标池；桌面镜像的源目录是「用户桌面 + 公共桌面」。记录症状（栅栏空 + 图标全体消失）与证据。

### 4.4 文档

- 本计划即 `docs/plans/08-desktop-mirror-ownership-and-public-desktop.md`。
- `docs/TODO.md`：把「回收站等虚拟项未镜像」登记为已知缺口（含原因：死图标 + 无属性依据）。

---

## 5. 防御性自查清单 (Defensive Invariants)

| 序号 | 校验律 | 本方案的针对性设计 |
| :---: | :--- | :--- |
| 1 | 常驻后台冲突律 | `existing` 改用**归属**口径，恰好满足「实体跨容器移动后全局事实来源一致」：整理走的项不会被同步抢回；收敛态集合相等 → 快路径生效，无循环重注册/无磁盘常刷。 |
| 2 | 空闲性能归零律 | 无新增定时器；收敛后每周期仅 2 次 `read_dir` + 集合比较；有变化才落盘、才重绘。 |
| 3 | 孤儿状态回收律 | 移除分支判定放宽为「在任一根目录内 + 文件不存在」，公共桌面的外部删除同样被回收；虚拟项无路径 → 天然不进该分支（不会被误删）。 |
| 4 | 语义精准定位律 | `is_desktop` 仍只认「storage_path == 用户桌面」；根目录集合去重且大小写/尾分隔符归一化；栅栏下标一律 `get()` 校验。 |
| 5 | 状态全集校验律 | 注册成功必须同时满足「目标栅栏存在」并清掉 `free_icons` 里的同名项，避免「既未分组又在栅栏」的复合脏状态。 |
| 6 | 旁路数据对齐律 | 复用既有 `register_fence_item` 事务：`items` / `item_index` / `bitmap_ids` / `pending_uploads` 同帧对齐；位图槽单调递增。 |

附加防线：

- **幂等**：连续两次 `reconcile_fences`，第二次必须 0 变动（集合相等 → 提前 return）。
- **空目录/无公共桌面**：`read_dir` 失败 → 空集合（既有容错）；`roots` 至少含用户桌面。
- **`free_icons` 回流**：只影响渲染不可见的项，不产生动画/定时器。
- **不写盘**：无变化不 `store.save`。

---

## 6. 验证与交付门禁 (Verification Gates)

### 6.1 自动化

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings        # 本机需临时门控 winres（跑完还原）
cargo test --workspace
```

用例矩阵（新增，`crates/app/src/file_ops.rs::tests` 与 `crates/core/src/storage.rs::tests`）：

| 场景 | 期望 |
| :--- | :--- |
| 桌面源 + 图标只在元数据池（无归属） | **不在** `owned`（修复前这里判为「已归属」→ 恒空） |
| 桌面源 + 图标被本栅栏持有 | 在 `owned` |
| 桌面源 + 图标被其它栅栏持有（已整理） | 在 `owned`（防回流） |
| 桌面源 + 图标在 `free_icons` | **不在** `owned`（未分组不算归属） |
| 桌面源 + 路径在 `roots` 之外 | 不在 `owned`（库内项不计入） |
| 普通目录镜像栅栏 | 只看本栅栏成员，其它栅栏的项不进 `owned` |
| 快路径矩阵：收敛 / 新文件 / 已归属项被删 / **已归属项被置隐藏** / 空目录 | 依次 true / false / false / **true**（隐藏不得造成抖动）/ true |
| `push_unique_dir` 重复/大小写/尾分隔符 | 只保留一份 |
| `dir_eq` 分隔符/大小写/尾分隔符 | 判为同一目录 |
| `storage::same_dir` 正斜杠写法 | 桌面镜像仍不可回退（无死按钮） |
| 幂等 | 对同一 Desk 连续两次调用结果一致 |

### 6.2 真实走查（debug 构建 + `WINBOSK_AUTOSTOP_MS`，**已实测**）

| 步骤 | 实测结果 |
| :--- | :--- |
| 修复前 `target/debug/data/desk.json` | `fences=1`，栅栏 `icon_ids = 0`，155 个图标零归属 |
| 修复后首次启动 | 栅栏 `icon_ids = **139**`（用户桌面 105 + 公共桌面 34），无报错日志 |
| 对 ground truth（真实桌面 140 项） | 只差「回收站」1 项虚拟壳项（见 §6.3） |
| 桌面新建文件 → 启动 | 139 → **140**，且包含该文件 |
| 给该文件加隐藏属性 → 启动 | 仍为 **140** 且仍在栏里（不误删），收敛不抖动 |
| 删除该文件 → 启动 | 回到 **139**，不再包含 |
| 连续两次运行 `desk.json` | 语义 **deep-equal**（仅 `HashMap` 序列化顺序不同），`icon_ids` 顺序一致 |
| 空闲 | 无新增定时器/写盘（详细 0% CPU 依据见 `docs/idle-cpu-feasibility.md`） |

> 本机沙箱跑门禁时需临时门控 `crates/app/build.rs` 的 winres（`reg.exe` 被安全策略拦截），
> **跑完立即还原**（`git diff crates/app/build.rs` 为空已确认）。

### 6.3 已知缺口（本轮不做，须如实记录）

对完 ground truth 的 140 项，修复后仍缺 **1 项：回收站**（唯一的虚拟桌面项）。原因见 Non-Goals#1：
无路径项在栅栏里是死图标。若要补齐，需要独立计划：① 判定虚拟项是否「显示在桌面」（需读
`HKCU\...\HideDesktopIcons\{NewStartPanel,ClassicStartMenu}` 的 per-CLSID 开关并处理「缺失即默认」
语义；实测本机仅 `{5C899A03-…}=1`，而 DefView 只显示了回收站 → 属性位无法区分）；② 给无路径项
实现打开/右键（PIDL 级 `ShellExecuteEx`/`IContextMenu`）。已登记到 `docs/TODO.md` §7。

### 6.4 独立子代理对抗性审查结论（已执行）

独立干净上下文的子代理完成对抗性走查，结论 **P0 无 / 可安全落地**，并提出 1 项 P1 + 2 项 P2：

| 级别 | 问题 | 处置 |
| :--- | :--- | :--- |
| **P1** | 「已归属但被外部置为隐藏/系统」的文件会让集合相等永假 → 每 4s 白跑全量注册 + 全量 `exists()` 探测，破坏空闲 0% CPU 铁律 | **已修**：快路径改为 §2.1 的两条子集断言（`mirror_converged`），并补单测矩阵 + 真机隐藏属性走查 |
| P2 | 桌面判定存在三套口径（`dir_eq` / 裸 `==` / `storage::same_dir`） | **已修**：三处统一（§4.2 / §4.2.1） |
| P2 | 8.3 短名 / OneDrive 重定向桌面的路径串可能不一致 | 记录为已知风险（低频；Shell 与 `read_dir` 在实测中一致：105 == 105）。若出现，应在入库时统一做长路径规范化 |
| P2 | 桌面子目录若被另建为 Folder Portal，其子项会被桌面分支顺带回收 | 与自身分支语义一致，判定为无害，不改 |

### 6.4 独立子代理对抗性审查要点

- 收敛态 `existing == entries` 是否**真的**成立（否则每 4s 走全量注册/移除）？
- 「一键整理」→ 镜像回流是否存在竞态（整理当帧与 `SyncLibrary` 同帧交替）？
- 公共桌面项被删除/重命名/改名冲突时，是否产生幽灵项或重复项？
- 大小写/短名（8.3）/尾部斜杠差异是否导致集合不相等而破坏快路径？
- 是否有新的写盘、定时器、或空闲重绘？
