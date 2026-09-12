# Sylva — Agent 操作规范

本项目是基于 Rust 的 Windows 桌面栅栏整理器。完整背景与功能说明见 [`README.md`](file:///g:/Codes/sylva/README.md)。

---

## 关键目录与职责边界

依赖方向严格自下而上单向流动：`core ← shell ← render ← app`。禁止反向依赖与跨层越级引用。

| 目录 | 职责与修改约束 |
| :--- | :--- |
| [`crates/core/`](file:///g:/Codes/sylva/crates/core/) | **纯 Rust 领域模型与算法**（布局、磁吸、动画状态机、配置序列化）。<br>**硬约束**：**零 Win32 / OS 依赖**。禁止引入 `windows` crate 或操作系统 API，必须保证在任何环境可独立单元测试。 |
| [`crates/shell/`](file:///g:/Codes/sylva/crates/shell/) | **Windows 壳层适配层**（COM 初始化、`IShellFolder` 图标枚举、`IShellItemImageFactory` 提取、`WorkerW`/`Progman` 探测接管）。禁止包含 UI 渲染与事件主循环。 |
| [`crates/render/`](file:///g:/Codes/sylva/crates/render/) | **图形渲染管线**（Direct2D、DirectWrite、WinRT `Windows.UI.Composition`、overlay 穿透窗口、GPU 硬件实时模糊）。只消费物理像素，不处理业务状态。 |
| [`crates/app/`](file:///g:/Codes/sylva/crates/app/) | **应用组装根**（Win32 主消息循环、系统托盘、全局热键、内联文本编辑、事件总线、内存修剪）。唯一持有完整运行时状态的模块。 |
| [`scripts/installer/`](file:///g:/Codes/sylva/scripts/installer/) | **独立安装器工程**。注意：拥有独立的 `Cargo.toml`，**不属于**根 workspace，根 `cargo build` 不会编译它。 |
| [`packaging/`](file:///g:/Codes/sylva/packaging/) | MSIX 与 Winget 打包清单模板，非代码修改无需触碰。 |

---

## 常用命令

CI 采用严格的 `-D warnings` 和格式检查（见 [`.github/workflows/ci.yml`](file:///g:/Codes/sylva/.github/workflows/ci.yml)）。修改后必须保证以下命令通过：

```powershell
# 编译全工作区
cargo build --workspace

# 运行所有测试
cargo test --workspace

# 单独运行指定 crate 的测试（如 core 纯算法测试）
cargo test -p sylva-core
cargo test -p sylva-core -- magnet::tests::move_clamps_to_screen

# 静态检查（CI 硬性阻断项，必须零警告）
cargo clippy --workspace -- -D warnings

# 代码格式检查
cargo fmt --all -- --check

# 发布构建（输出 target\release\sylva.exe，双击无控制台窗口）
cargo build --release

# 发布打包脚本（编译 release 并归档到 dist\）
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1

# 自动化验证运行（利用退出钩子运行 2 秒后自动退出，避免桌面进程挂起阻塞终端）
$env:SYLVA_AUTOSTOP_MS="2000"; .\target\debug\sylva.exe
```

---

## 架构约束与“为什么”（反常设计与证据）

以下设计违背常见直觉，但属于有意为之的架构决定。**禁止在代码重构中“顺手修改”它们**：

1. **`sylva-core` 必须保持零 OS 依赖**
   - **证据**：[`crates/core/src/lib.rs:3`](file:///g:/Codes/sylva/crates/core/src/lib.rs#L3) 明确声明 `//! 本 crate 零 Win32 依赖，只包含领域模型与纯算法，保证可以在任何环境单元测试。`
   - **原因**：解耦桌面状态机与 Windows API，使布局与磁吸算法脱离 GPU 与 COM 环境即可瞬时完成测试。

2. **根 `Cargo.toml` 必须显式声明 `windows-numerics` 与 `windows-core`**
   - **证据**：[`Cargo.toml:31-35`](file:///g:/Codes/sylva/Cargo.toml#L31-L35) 注释说明：`windows-numerics` 包含 D2D 矩阵与向量类型，`windows` crate 引用但未重导出；`implement!` 宏生成的 COM/WinRT 引用 `::windows_core::`。
   - **原因**：Agent 容易将其误判为“未使用的冗余依赖”而移除，导致 COM 宏与图形类型无法编译。

3. **桌面接管只隐藏 `SysListView32`，必须通过 RAII `IconGuard` 保证退出恢复**
   - **证据**：[`crates/app/src/main.rs:9`](file:///g:/Codes/sylva/crates/app/src/main.rs#L9)、[`crates/app/src/main.rs:145-162`](file:///g:/Codes/sylva/crates/app/src/main.rs#L145-L162)。
   - **原因**：绝对禁止销毁或重挂 `WorkerW`/`Progman`，否则会破坏 Wallpaper Engine 等动态壁纸；桌面图标隐藏必须由 `IconGuard` 守卫，确保任何崩溃或退出路径图标均能 100% 自动还原。

4. **Overlay 穿透采用 `SetWindowRgn` 区域裁剪，而非 `WS_EX_TRANSPARENT`**
   - **证据**：[`crates/app/src/main.rs:12-13`](file:///g:/Codes/sylva/crates/app/src/main.rs#L12-L13)、[`crates/render/src/overlay.rs:164`](file:///g:/Codes/sylva/crates/render/src/overlay.rs#L164)（`apply_hit_model`）。
   - **原因**：若全屏窗口仅依赖透明度，鼠标事件会被全屏死区拦截；通过 `SetWindowRgn` 将窗口边界实时剪裁为栅栏几何体的并集，非栅栏区域天然穿透至桌面底层。

5. **WinRT Compositor 创建必须前置 DispatcherQueue**
   - **证据**：[`crates/render/src/device.rs:6-8`](file:///g:/Codes/sylva/crates/render/src/device.rs#L6-L8) 注释记录：`缺 DispatcherQueue 时 Compositor::new() 返回 E_ACCESSDENIED`。
   - **原因**：WinRT `Windows.UI.Composition` 依赖线程拥有 STA COM 消息循环及 WinRT DispatcherQueue，必须在调用 `Compositor::new` 前调用 `CreateDispatcherQueueController` 并持久持有其控制器。

6. **主线程事件循环具备再入守卫（`ReentryGuard`）**
   - **证据**：[`crates/app/src/main.rs:242-252`](file:///g:/Codes/sylva/crates/app/src/main.rs#L242-L252)、[`crates/app/src/main.rs:560-564`](file:///g:/Codes/sylva/crates/app/src/main.rs#L560-L564)。
   - **原因**：在弹出 Shell 模态右键菜单（`TrackPopupMenu`）或文件属性对话框时，Win32 会在主线程派发嵌套消息。此时外层仍借用 `Runtime`，若不丢弃再入事件将直接引发 `RefCell` panic。

7. **数据目录固定为 `<exe_parent>/data`，严禁迁往 `%APPDATA%`**
   - **证据**：[`crates/app/src/main.rs:291-297`](file:///g:/Codes/sylva/crates/app/src/main.rs#L291-L297) 注释明确说明与安装器卸载约定一致，并已包含旧版 `%APPDATA%\Sylva` 的单向迁移逻辑。
   - **原因**：绿色便携与安装器干净卸载要求配置与内部库随程序目录彻底清理，禁止散落至系统漫游目录。

8. **自适应高度栅栏在数据模型中以 `bounds.h <= 0.0` 为标记，碰撞检测必须读 `last_layout_h`**
   - **证据**：[`crates/app/src/main.rs:516`](file:///g:/Codes/sylva/crates/app/src/main.rs#L516)、[`crates/app/src/main.rs:615-620`](file:///g:/Codes/sylva/crates/app/src/main.rs#L615-L620)。
   - **原因**：用户未手动拖拽垂直高度的栅栏标记为 `bounds.h = 0.0`（自适应高度）。计算防重叠与夹屏时，禁止直接取 `bounds.h`（否则会被判为 0 高导致栅栏相互叠层），必须取由首帧布局回写的 `last_layout_h`。

9. **渲染层度量统一为物理像素，DPI 缩放必须在 App 层通过 `Theme.scale` 整体缩放**
   - **证据**：[`crates/render/src/lib.rs:17-18`](file:///g:/Codes/sylva/crates/render/src/lib.rs#L17-L18)、[`crates/app/src/main.rs:359-363`](file:///g:/Codes/sylva/crates/app/src/main.rs#L359-L363)。
   - **原因**：D2D 渲染目标固定在 96 DPI，所有文字、图标尺寸、行距、列宽、内边距必须同步乘以 `dpi_scale`。单独放大文字字号会导致严重的文字与图标重叠排版事故。

10. **空闲时严格保持 0% CPU — 禁止常驻轮询定时器**
    - **证据**：[`crates/app/src/main.rs:201`](file:///g:/Codes/sylva/crates/app/src/main.rs#L201)、[`crates/render/src/lib.rs:20-22`](file:///g:/Codes/sylva/crates/render/src/lib.rs#L20-L22)。
    - **原因**：背景模糊由 WinRT DWM GPU 实时渲染，无截屏高斯。所有补间动画（`AnimTick`）结束后必须立刻停用 `WM_TIMER`，空闲时不得占用 CPU 周期。

11. **收起栅栏的「原大小占位框」必须同时并入窗口区域与合成表面，且必须白名单式清理**
    - **证据**：[`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)（`reserved_frames`）、[`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)（`build_region`）、[`crates/render/src/scene.rs`](file:///g:/Codes/sylva/crates/render/src/scene.rs)（`content_rect`）。
    - **原因**：折叠只改变视觉高度，碰撞/夹屏仍按原矩形计算；拖动期间把该矩形画成虚线淡框，用户才不会觉得在撞空气墙。两个裁剪口缺一不可——**窗口区域（`SetWindowRgn`）之外既不渲染也不收事件**，**合成表面按 `content_rect` 裁剪**，漏任一处都表现为「框看不见」。而并入窗口区域意味着该区域短暂不再点击穿透，所以占位框只在拖动期间存在，并由 App 层**白名单式清理**（只有 `FenceMove` 等拖动事件保留，其余任何事件 + 左键已松开都清空），清理后必须强制重绘一帧让区域收缩——否则滞留的占位框会持续吞掉那块区域的桌面点击。

12. **碰撞口径唯一真源是 `Fence::collision_rect`**
    - **证据**：[`crates/core/src/model.rs`](file:///g:/Codes/sylva/crates/core/src/model.rs)（`collision_height` / `collision_rect`）。
    - **原因**：`bounds.h > 0` 为手动缩放的固定高度，`bounds.h <= 0` 为自动高度（真实高度由 App 层旁路表 `last_layout_h` 提供）。拖动、避让、启动重叠消解、占位框提示必须共用同一函数——历史上 `FenceMove` 直接取 `bounds.h`，自动高度栅栏退化成 0 高后自身漏检邻居、还能被拖出屏幕。

13. **桌面镜像栅栏的「已归属」判据是「归属」，不是「全局图标池」；且桌面 = 用户桌面 + 公共桌面**
    - **证据**：[`crates/app/src/file_ops.rs`](file:///g:/Codes/sylva/crates/app/src/file_ops.rs)（`mirror_existing_paths` / `mirror_converged` / `desktop_source_dirs`）。
    - **原因**：启动时的元数据补齐会把**枚举到的每一项**无条件写进 `desk.icons`（无论有没有栅栏归属），所以拿全局池判重会把所有桌面项都误判为「已归属」→ 桌面镜像栅栏**永远为空**；而真实桌面图标已被 `IconGuard` 隐藏、无归属的图标在渲染层又没有任何绘制入口，用户看到的是「桌面全空」。判据必须取「已被任一栅栏持有」（`fences[].icon_ids`）——它同时保证「一键整理」搬进分类栅栏的图标不被镜像抢回。未分组区 `free_icons` **不计入**归属（它没有绘制入口，算归属会让「移出栅栏」的项彻底消失）。源目录是两个：`FOLDERID_Desktop` + `FOLDERID_PublicDesktop`，只扫前者会让 `C:\Users\Public\Desktop` 的公共快捷方式凭空消失；`storage_path` 仍只指向用户桌面（新文件落盘位置与删除语义依赖它）。**同步快路径也必须用「两条子集断言」（`owned ⊆ 磁盘全部条目` 且 `可见条目 ⊆ owned`），禁止改成集合相等**：已归属但被外部置为隐藏的文件会让相等判定永久为假，每 4s 白跑一遍全量注册 + 全量 `exists()` 探测。

---

## 边界与禁区

- **禁止修改桌面根窗口层级结构**：禁止调用 `DestroyWindow` 或永久更改 `Progman`/`WorkerW`/`SHELLDLL_DefView` 的父子拓扑。
- **禁止在 `crates/core` 中引入平台代码**：禁止在该 crate 中添加 `#[cfg(windows)]` 或 `windows` 依赖。
- **禁止在渲染主线程中执行耗时同步 I/O**：桌面项图标初次加载、大文件 Shell 属性查询必须后台运行或使用预热机制（见 [`crates/app/src/shell_menu.rs`](file:///g:/Codes/sylva/crates/app/src/shell_menu.rs) 中的 `prime_startup`）。
- **禁止私自引入重型运行时**：禁止引入 `tokio`、`WebView2`、`Tauri` 等，保持无额外运行时的单线程轻量架构。

---

## 常见任务配方

- **添加 / 修改栅栏外观样式与配置**：
  1. [`crates/core/src/model.rs`](file:///g:/Codes/sylva/crates/core/src/model.rs)：在 `FenceAppearance` 或 `Desk` 添加字段，**必须加 `#[serde(default)]`** 确保向后兼容旧版 `desk.json`；
  2. [`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs)：在 `build_scene` / `build_console` 中加入交互控键计算；
  3. [`crates/render/src/draw.rs`](file:///g:/Codes/sylva/crates/render/src/draw.rs)：在 D2D 绘制管线中绘制新增视觉元素；
  4. [`crates/app/src/context_menu.rs`](file:///g:/Codes/sylva/crates/app/src/context_menu.rs) 或 [`crates/app/src/editing.rs`](file:///g:/Codes/sylva/crates/app/src/editing.rs)：绑定点击与编辑交互并持久化。
- **调整图标排列、间距与吸附对齐算法**：
  1. 网格与列表图标流式排布：修改 [`crates/core/src/layout.rs`](file:///g:/Codes/sylva/crates/core/src/layout.rs)；
  2. 边界磁吸、推离防重叠与夹屏约束：修改 [`crates/core/src/magnet.rs`](file:///g:/Codes/sylva/crates/core/src/magnet.rs)；
  3. 运行对应的纯算法单测：`cargo test -p sylva-core -- magnet`。
- **处理 Shell 交互、外部文件拖拽入栅栏**：
  1. 壳层底层提取与项构建：[`crates/shell/src/items.rs`](file:///g:/Codes/sylva/crates/shell/src/items.rs)；
  2. 文件复制与内部库同步逻辑：[`crates/app/src/file_ops.rs`](file:///g:/Codes/sylva/crates/app/src/file_ops.rs)；
  3. 窗口拖放消息接收与命中栅栏反查：[`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs)（`WM_DROPFILES`）。

---

## 实施计划（Plan）编写规范与审查标准

凡涉及跨模块重构、交互逻辑调整、状态机变更或非平凡功能实施，必须严格遵循 [`docs/engineering-plan-guidelines.md`](file:///g:/Codes/sylva/docs/engineering-plan-guidelines.md) 的编制规范与「隐性动力学自查六律」（常驻后台冲突、0% 空闲 CPU、孤儿清理、语义精准定位、状态全集校验、旁路数据对齐）。实施完成后必须派发独立干净上下文的子代理执行对抗性代码审查。

---

## 环境前置条件

- **操作系统**：Windows 10 / Windows 11（x86_64，MSVC 工具链）。
- **Rust 版本**：Edition 2021（`Cargo.toml` 声明），需安装 stable 工具链。
- **关键环境变量**：
  - `SYLVA_AUTOSTOP_MS`：自动化测试退出钩子。值为毫秒数（例如 `2000`）。设置后进程到时自动向主循环发送 `WM_APP_QUIT` 干净退出，无需人工干预。

---

## 测试约定

- `cargo test --workspace` 可直接运行全部测试。
- `sylva-core` 内所有测试均为纯内存测试，瞬时完成，零外部依赖。
- `sylva-shell` 中的单元测试包含只读探测真实系统的测试（如 `live_enumerate_real_desktop`），此类测试仅做只读枚举，不会变动系统桌面文件。

---

## 领域术语表

- **Fence（栅栏）**：桌面图标聚合容器卡片，支持 `Grid`（网格）、`List`（列表）与 `Sidebar`（仿 Dock 侧边栏）三种布局。
- **Shell Takeover（壳层接管）**：仅将原生桌面视图 `SysListView32` 隐去，将 overlay 挂到 `WorkerW` 层，不干扰动态壁纸。
- **IconGuard**：控制真实桌面图标可见性的 RAII 守卫，`Drop` 时强制调用 `restore_icons` 恢复桌面。
- **OverlayWindow**：覆盖全虚拟屏的透明层级窗口，借助 `SetWindowRgn` 镂空非栅栏区域。
- **Control Center（控制中心）**：单页「栅栏管理」控制面板（`Ctrl+Alt+T` 呼出），直接在 DirectComposition 视觉树内绘制，无独立 HWND。
- **InlineEdit**：基于 Direct2D / DirectWrite 的合成表面内联文本编辑系统，用于栅栏与图标就地重命名。
- **Internal Library（内部库）**：`<data_dir>/library` 目录，栅栏非桌面源文件拖入时存放的物理副本。
- **Storage Mode（存储模式）**：由 `Fence::storage_path` 决定文件物理落地位置，**不是**磁盘容量。
  两种模式是机制差异而非程度差异：
  - `None` → **应用内部**：所有无链接栅栏**共享**同一个扁平内部库，栅栏索引其中的副本；
  - `Some(dir)` → **外部文件夹**（链接栅栏 / Folder Portal）：栅栏与该目录双向镜像，后台
    `SyncLibrary`（4s 周期）做集合差集同步。
  该字段**同时决定删除语义**：`is_managed_path` 内的文件从栅栏删除会真删磁盘文件，在外的
  只摘引用。控制中心栅栏详情区的「文件位置」行必须常显模式与真实路径，不得回退为只显示按钮。
- **Reset Storage（恢复默认）**：把外部文件夹模式还原为应用内部模式。**不移动、不复制、不删除
  任何磁盘文件**；原本镜像自该文件夹的成员从栅栏摘除（文件留在原处）。桌面镜像栅栏
  （`storage_path == 真实桌面目录`）**禁止**执行——会让栅栏清空而真实图标正被壳层隐藏。

---

## 本文档有意未包含的内容

- 项目详细背景、功能特性清单与视觉效果图：见 [`README.md`](file:///g:/Codes/sylva/README.md)
- 完整依赖项清单与第三方库具体版本：见 [`Cargo.toml`](file:///g:/Codes/sylva/Cargo.toml)
- 软件工程实施计划编写规范与自查六律：见 [`docs/engineering-plan-guidelines.md`](file:///g:/Codes/sylva/docs/engineering-plan-guidelines.md)
- 架构设计演进历史与废弃技术方案（如 egui 移除记录）：见 [`docs/design/2026-08-14-desktop-fence-organizer-design.md`](file:///g:/Codes/sylva/docs/design/2026-08-14-desktop-fence-organizer-design.md)
- 完整 CI 自动化流水线配置：见 [`.github/workflows/ci.yml`](file:///g:/Codes/sylva/.github/workflows/ci.yml)
- 安装器实现细节与打包细节：见 [`scripts/installer/`](file:///g:/Codes/sylva/scripts/installer/) 与 [`scripts/package-msix.ps1`](file:///g:/Codes/sylva/scripts/package-msix.ps1)
