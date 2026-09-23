# WinBosk — Agent 操作规范

本项目是基于 Rust 的 Windows 桌面栅栏整理器。完整背景与功能说明见 [`README.md`](file:///g:/Codes/winbosk/README.md)。

---

## 关键目录与职责边界

依赖方向严格自下而上单向流动：`core ← shell ← render ← app`。禁止反向依赖与跨层越级引用。

| 目录 | 职责与修改约束 |
| :--- | :--- |
| [`crates/core/`](file:///g:/Codes/winbosk/crates/core/) | **纯 Rust 领域模型与算法**（布局、磁吸、动画状态机、配置序列化）。<br>**硬约束**：**零 Win32 / OS 依赖**。禁止引入 `windows` crate 或操作系统 API，必须保证在任何环境可独立单元测试。 |
| [`crates/shell/`](file:///g:/Codes/winbosk/crates/shell/) | **Windows 壳层适配层**（COM 初始化、`IShellFolder` 图标枚举、`IShellItemImageFactory` 提取、`WorkerW`/`Progman` 探测接管）。禁止包含 UI 渲染与事件主循环。 |
| [`crates/render/`](file:///g:/Codes/winbosk/crates/render/) | **图形渲染管线**（Direct2D、DirectWrite、WinRT `Windows.UI.Composition`、overlay 穿透窗口、GPU 硬件实时模糊）。只消费物理像素，不处理业务状态。 |
| [`crates/app/`](file:///g:/Codes/winbosk/crates/app/) | **应用组装根**（Win32 主消息循环、系统托盘、全局热键、内联文本编辑、事件总线、内存修剪）。唯一持有完整运行时状态的模块。 |
| [`scripts/installer/`](file:///g:/Codes/winbosk/scripts/installer/) | **独立安装器工程**。注意：拥有独立的 `Cargo.toml`，**不属于**根 workspace，根 `cargo build` 不会编译它。 |
| [`packaging/`](file:///g:/Codes/winbosk/packaging/) | MSIX 与 Winget 打包清单模板，非代码修改无需触碰。 |

---

## 常用命令

CI 采用严格的 `-D warnings` 和格式检查（见 [`.github/workflows/ci.yml`](file:///g:/Codes/winbosk/.github/workflows/ci.yml)）。修改后必须保证以下命令通过：

```powershell
# 编译全工作区
cargo build --workspace

# 运行所有测试
cargo test --workspace

# 单独运行指定 crate 的测试（如 core 纯算法测试）
cargo test -p winbosk-core
cargo test -p winbosk-core -- magnet::tests::move_clamps_to_screen

# 静态检查（CI 硬性阻断项，必须零警告）
cargo clippy --workspace -- -D warnings

# 代码格式检查
cargo fmt --all -- --check

# 发布构建（输出 target\release\winbosk.exe，双击无控制台窗口）
cargo build --release

# 发布打包脚本（编译 release 并归档到 dist\）
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1

# 自动化验证运行（利用退出钩子运行 2 秒后自动退出，避免桌面进程挂起阻塞终端）
$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe
```

（另：**不要把两条 `cargo` 命令并行跑**——同一个 `target/` 目录会互相踩增量构建，偶发 `拒绝访问 (os error 5)` 甚至 rustc ICE。）

**构建环境硬约束（2026-09-23 实测补齐）**：

- **agent 会话跑 `cargo`（build / test / clippy）必须在「无文件系统沙箱」下执行**。沙箱的文件系统拦截会让 cargo 的增量会话收尾写入被拒——
  `error copying object file …deps\lib×.rmeta to …\incremental\<crate>-<hash>\s-<hash>-working\metadata.rmeta: 拒绝访问。 (os error 5)`；
  而 cargo 只打 warning 就照旧 finalize，会话目录于是被**半写**，下一次真编译 rustc 直接 ICE（见下条）。
  **证据**：同一份构建沙箱内 37~46s、无沙箱 2~5s（慢 8 倍）；清空增量缓存后，无沙箱连跑两次真重建**零告警零 ICE**，沙箱内第二次必 ICE。排除项：长路径形式 `\\?\G:\…` 与 ACL 均正常（无沙箱实测 copy 两种形式都成功），不是权限问题。
- **cargo 会对 fresh（未重编译）的单元重放缓存的旧诊断**：一次 0.15s 全 fresh 的 `cargo build` 也会刷出
  `拒绝访问` 告警，甚至点名**早已被删掉**的 lock 文件与不存在的 `-working` 目录。
  **判据**：先看有没有 `Compiling` 行、告警里的会话随机名是不是本轮的——**别把重放的旧告警当成新故障**去改代码。
- **ICE 处置（顺序固定）**：确认没有其它 `cargo` 在跑 → `rm -rf target/debug/incremental`（本项目实测曾累积到
  **771MB / 78 个残留 lock**）→ **无沙箱**重新构建。仅删缓存而不脱离沙箱，会立刻再写坏一次。

**改完代码必须自己跑构建与门禁，不许「只改不验」**：

- 任何代码改动，收尾前**至少要让 `cargo build --workspace` 编过**（含 `winbosk-app`）。编译错误不许留给用户去发现。
- 交付/提交前跑齐：`cargo build --workspace` + `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --all -- --check`。
- **构建前先退出正在运行的 WinBosk**：Windows 会锁住正在运行的可执行映像，链接阶段会报 `LNK1104: 无法打开文件 …\winbosk.exe`。要么先退出实例，要么只构建不被占用的 profile（如 `--release`）。同理，桌面验证前也别让旧实例留着（它还持有单实例互斥，新实例会静默退出）。
- 构建/门禁命令能跑但受环境限制（如受限沙箱）时，**必须在结论里显式写出「哪一步没跑、为什么」**，不允许默不作声地跳过。
- **看到 `the compiler unexpectedly panicked` 先别改代码**：`rustc_metadata\src\rmeta\encoder.rs: no entry found for key`
  这类 ICE 是 `target\<profile>\incremental` **增量缓存损坏**，与业务代码无关——**可能炸在你根本没改过的 crate 上**
  （2026-09-23 实测：改动只在 `render`，ICE 出现在 `core`）。成因是沙箱内构建 / 多会话共用同一个 `target/` 并发构建
  （本项目常有多个 agent 会话并存）。处置见上文「ICE 处置（顺序固定）」：**清 `target\debug\incremental` + 无沙箱重建**。
  根治首选「agent 会话无沙箱构建」（也顺带拿回 8 倍构建速度）；确有并发会话时再给它们各设独立的 `CARGO_TARGET_DIR`。
  **不推荐**用 `.cargo/config.toml` 的 `[build] incremental = false` 兜底——会把开发期的秒级增量构建打成全量重编。

---

## 架构约束与“为什么”（反常设计与证据）

以下设计违背常见直觉，但属于有意为之的架构决定。**禁止在代码重构中“顺手修改”它们**：

1. **`winbosk-core` 必须保持零 OS 依赖**
   - **证据**：[`crates/core/src/lib.rs:3`](file:///g:/Codes/winbosk/crates/core/src/lib.rs#L3) 明确声明 `//! 本 crate 零 Win32 依赖，只包含领域模型与纯算法，保证可以在任何环境单元测试。`
   - **原因**：解耦桌面状态机与 Windows API，使布局与磁吸算法脱离 GPU 与 COM 环境即可瞬时完成测试。

2. **根 `Cargo.toml` 必须显式声明 `windows-numerics` 与 `windows-core`**
   - **证据**：[`Cargo.toml:31-35`](file:///g:/Codes/winbosk/Cargo.toml#L31-L35) 注释说明：`windows-numerics` 包含 D2D 矩阵与向量类型，`windows` crate 引用但未重导出；`implement!` 宏生成的 COM/WinRT 引用 `::windows_core::`。
   - **原因**：Agent 容易将其误判为“未使用的冗余依赖”而移除，导致 COM 宏与图形类型无法编译。

3. **桌面接管只隐藏 `SysListView32`，必须通过 RAII `IconGuard` 保证退出恢复**
   - **证据**：[`crates/app/src/main.rs:9`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L9)、[`crates/app/src/main.rs:145-162`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L145-L162)。
   - **原因**：绝对禁止销毁或重挂 `WorkerW`/`Progman`，否则会破坏 Wallpaper Engine 等动态壁纸；桌面图标隐藏必须由 `IconGuard` 守卫，确保任何崩溃或退出路径图标均能 100% 自动还原。

4. **Overlay 穿透采用 `SetWindowRgn` 区域裁剪，而非 `WS_EX_TRANSPARENT`**
   - **证据**：[`crates/app/src/main.rs:12-13`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L12-L13)、[`crates/render/src/overlay.rs:164`](file:///g:/Codes/winbosk/crates/render/src/overlay.rs#L164)（`apply_hit_model`）。
   - **原因**：若全屏窗口仅依赖透明度，鼠标事件会被全屏死区拦截；通过 `SetWindowRgn` 将窗口边界实时剪裁为栅栏几何体的并集，非栅栏区域天然穿透至桌面底层。

5. **WinRT Compositor 创建必须前置 DispatcherQueue**
   - **证据**：[`crates/render/src/device.rs:6-8`](file:///g:/Codes/winbosk/crates/render/src/device.rs#L6-L8) 注释记录：`缺 DispatcherQueue 时 Compositor::new() 返回 E_ACCESSDENIED`。
   - **原因**：WinRT `Windows.UI.Composition` 依赖线程拥有 STA COM 消息循环及 WinRT DispatcherQueue，必须在调用 `Compositor::new` 前调用 `CreateDispatcherQueueController` 并持久持有其控制器。

6. **主线程事件循环具备再入守卫（`ReentryGuard`）**
   - **证据**：[`crates/app/src/main.rs:242-252`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L242-L252)、[`crates/app/src/main.rs:560-564`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L560-L564)。
   - **原因**：在弹出 Shell 模态右键菜单（`TrackPopupMenu`）或文件属性对话框时，Win32 会在主线程派发嵌套消息。此时外层仍借用 `Runtime`，若不丢弃再入事件将直接引发 `RefCell` panic。

7. **数据目录固定为 `<exe_parent>/data`，严禁迁往 `%APPDATA%`**
   - **证据**：[`crates/app/src/main.rs:291-297`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L291-L297) 注释明确说明与安装器卸载约定一致，并已包含旧版 `%APPDATA%\WinBosk` 的单向迁移逻辑。
   - **原因**：绿色便携与安装器干净卸载要求配置与内部库随程序目录彻底清理，禁止散落至系统漫游目录。

8. **自适应高度栅栏在数据模型中以 `bounds.h <= 0.0` 为标记，碰撞检测必须读 `last_layout_h`**
   - **证据**：[`crates/app/src/main.rs:516`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L516)、[`crates/app/src/main.rs:615-620`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L615-L620)。
   - **原因**：用户未手动拖拽垂直高度的栅栏标记为 `bounds.h = 0.0`（自适应高度）。计算防重叠与夹屏时，禁止直接取 `bounds.h`（否则会被判为 0 高导致栅栏相互叠层），必须取由首帧布局回写的 `last_layout_h`。

9. **渲染层度量统一为物理像素，DPI 缩放必须在 App 层通过 `Theme.scale` 整体缩放**
   - **证据**：[`crates/render/src/lib.rs:17-18`](file:///g:/Codes/winbosk/crates/render/src/lib.rs#L17-L18)、[`crates/app/src/main.rs:359-363`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L359-L363)。
   - **原因**：D2D 渲染目标固定在 96 DPI，所有文字、图标尺寸、行距、列宽、内边距必须同步乘以 `dpi_scale`。单独放大文字字号会导致严重的文字与图标重叠排版事故。

10. **空闲期主线程不得有周期性唤醒 — 唯一的例外是库同步心跳（1 次 / 4s）**
    - **证据**：[`crates/app/src/main.rs:201`](file:///g:/Codes/winbosk/crates/app/src/main.rs#L201)、[`crates/render/src/lib.rs:20-22`](file:///g:/Codes/winbosk/crates/render/src/lib.rs#L20-L22)、[`docs/idle-cpu-feasibility.md`](file:///g:/Codes/winbosk/docs/idle-cpu-feasibility.md)。
    - **判据（必须可实测，不要写成纯描述性文字——那样 CI 与人都检查不了）**：用 `QueryThreadCycleTime` 对窗口线程（= 主线程）分桶采样，**非心跳桶必须精确为 0**，心跳桶稳定在 1 次 / 4s。进程级总量只做趋势对比、不设等式：它含 GPU 用户态驱动线程与第三方 Shell 扩展线程，本机实测 0.15%~0.25%，且**同一台机器不同时刻能差近 2 倍**。那些不是本工程能关掉的，既不要拿它们当借口，**也不要再声称"空闲 0% CPU"**。
    - **原因**：背景模糊由 WinRT DWM GPU 实时渲染，无截屏高斯。所有补间动画（`AnimTick`）结束后必须立刻停用节拍时钟——现役为 `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` 可等待定时器，`SetTimer`/`WM_TIMER` 仅作降级兜底（见 `docs/plans/06-console-reveal-animation.md` §7-§8），空闲时不得占用 CPU 周期。
    - **历史（为什么这条要写死口径）**：原作者 08-15（`4b74f80`）引入 4s 库同步轮询，13 天后（08-28）又在渲染层写下「空闲时 0% CPU……无刷新定时器」——这条约束**从诞生起就带着例外**，只是此前无人量化，才让"0%"的说法以讹传讹了一个月。

11. **收起栅栏的「原大小占位框」必须同时并入窗口区域与合成表面，且必须白名单式清理**
    - **证据**：[`crates/app/src/scene.rs`](file:///g:/Codes/winbosk/crates/app/src/scene.rs)（`reserved_frames`）、[`crates/render/src/overlay.rs`](file:///g:/Codes/winbosk/crates/render/src/overlay.rs)（`build_region`）、[`crates/render/src/scene.rs`](file:///g:/Codes/winbosk/crates/render/src/scene.rs)（`content_rect`）。
    - **原因**：折叠只改变视觉高度，碰撞/夹屏仍按原矩形计算；拖动期间把该矩形画成虚线淡框，用户才不会觉得在撞空气墙。两个裁剪口缺一不可——**窗口区域（`SetWindowRgn`）之外既不渲染也不收事件**，**合成表面按 `content_rect` 裁剪**，漏任一处都表现为「框看不见」。而并入窗口区域意味着该区域短暂不再点击穿透，所以占位框只在拖动期间存在，并由 App 层**白名单式清理**（只有 `FenceMove` 等拖动事件保留，其余任何事件 + 左键已松开都清空），清理后必须强制重绘一帧让区域收缩——否则滞留的占位框会持续吞掉那块区域的桌面点击。

12. **碰撞口径唯一真源是 `Fence::collision_rect`**
    - **证据**：[`crates/core/src/model.rs`](file:///g:/Codes/winbosk/crates/core/src/model.rs)（`collision_height` / `collision_rect`）。
    - **原因**：`bounds.h > 0` 为手动缩放的固定高度，`bounds.h <= 0` 为自动高度（真实高度由 App 层旁路表 `last_layout_h` 提供）。拖动、避让、启动重叠消解、占位框提示必须共用同一函数——历史上 `FenceMove` 直接取 `bounds.h`，自动高度栅栏退化成 0 高后自身漏检邻居、还能被拖出屏幕。

13. **桌面镜像栅栏的「已归属」判据是「归属」，不是「全局图标池」；且桌面 = 用户桌面 + 公共桌面**
    - **证据**：[`crates/app/src/file_ops.rs`](file:///g:/Codes/winbosk/crates/app/src/file_ops.rs)（`mirror_existing_paths` / `mirror_converged` / `desktop_source_dirs`）。
    - **原因**：启动时的元数据补齐会把**枚举到的每一项**无条件写进 `desk.icons`（无论有没有栅栏归属），所以拿全局池判重会把所有桌面项都误判为「已归属」→ 桌面镜像栅栏**永远为空**；而真实桌面图标已被 `IconGuard` 隐藏、无归属的图标在渲染层又没有任何绘制入口，用户看到的是「桌面全空」。判据必须取「已被任一栅栏持有」（`fences[].icon_ids`）——它同时保证「一键整理」搬进分类栅栏的图标不被镜像抢回。未分组区 `free_icons` **不计入**归属（它没有绘制入口，算归属会让「移出栅栏」的项彻底消失）。源目录是两个：`FOLDERID_Desktop` + `FOLDERID_PublicDesktop`，只扫前者会让 `C:\Users\Public\Desktop` 的公共快捷方式凭空消失；`storage_path` 仍只指向用户桌面（新文件落盘位置与删除语义依赖它）。**同步快路径也必须用「两条子集断言」（`owned ⊆ 磁盘全部条目` 且 `可见条目 ⊆ owned`），禁止改成集合相等**：已归属但被外部置为隐藏的文件会让相等判定永久为假，每 4s 白跑一遍全量注册 + 全量 `exists()` 探测。

14. **绘制会话必须「有始有终」——`Frame` 的 `Drop` 兜底 `EndDraw`；且任何 D2D 调用都要假定它可能返回参数错误**
    - **证据**：[`crates/render/src/surface.rs`](file:///g:/Codes/winbosk/crates/render/src/surface.rs)（`impl Drop for Frame`）、[`crates/render/src/draw.rs`](file:///g:/Codes/winbosk/crates/render/src/draw.rs)（`dashed_props` / `DASHES` / `draw_scene_with_reserved_frame_succeeds`）。
    - **原因**：`BeginDraw` 之后绘制表面处于「已获取」状态，只有 `EndDraw` 会把它还回去。`present` 里任何一次 `?` 提前返回都会跳过 `EndDraw`，表面便漏在已获取态，此后**每一帧都失败且无法自愈**（线上实测：一次虚线参数错误 → 每帧 `E_INVALIDARG` 刷屏 → 约 700 帧后转为不可恢复的 `0x80131509`，界面彻底冻死只能重启进程）。故失败路径由 `Drop` 兜底归还；`finish` 先置位再 `EndDraw`，保证恰好调用一次。另一半教训是 D2D 的**参数错误编译期完全看不出来**：`dashes` 非空时 `dashStyle` 必须是 `D2D1_DASH_STYLE_CUSTOM`，否则 `CreateStrokeStyle` 返回 `E_INVALIDARG`。新增绘制代码后，必须靠「内存 DIB + DC 渲染目标真跑一遍 `draw_scene`」的测试（无需 GPU/窗口，CI 可跑）兜住这一类。

---

## 边界与禁区

- **禁止修改桌面根窗口层级结构**：禁止调用 `DestroyWindow` 或永久更改 `Progman`/`WorkerW`/`SHELLDLL_DefView` 的父子拓扑。
- **禁止在 `crates/core` 中引入平台代码**：禁止在该 crate 中添加 `#[cfg(windows)]` 或 `windows` 依赖。
- **禁止在渲染主线程中执行耗时同步 I/O**：桌面项图标初次加载、大文件 Shell 属性查询必须后台运行或使用预热机制（见 [`crates/app/src/shell_menu.rs`](file:///g:/Codes/winbosk/crates/app/src/shell_menu.rs) 中的 `prime_startup`）。
- **禁止私自引入重型运行时**：禁止引入 `tokio`、`WebView2`、`Tauri` 等，保持无额外运行时的单线程轻量架构。

---

## 常见任务配方

- **添加 / 修改栅栏外观样式与配置**：
  1. [`crates/core/src/model.rs`](file:///g:/Codes/winbosk/crates/core/src/model.rs)：在 `FenceAppearance` 或 `Desk` 添加字段，**必须加 `#[serde(default)]`** 确保向后兼容旧版 `desk.json`；
  2. [`crates/app/src/scene.rs`](file:///g:/Codes/winbosk/crates/app/src/scene.rs)：在 `build_scene` / `build_console` 中加入交互控键计算；
  3. [`crates/render/src/draw.rs`](file:///g:/Codes/winbosk/crates/render/src/draw.rs)：在 D2D 绘制管线中绘制新增视觉元素；
  4. [`crates/app/src/context_menu.rs`](file:///g:/Codes/winbosk/crates/app/src/context_menu.rs) 或 [`crates/app/src/editing.rs`](file:///g:/Codes/winbosk/crates/app/src/editing.rs)：绑定点击与编辑交互并持久化。
- **调整图标排列、间距与吸附对齐算法**：
  1. 网格与列表图标流式排布：修改 [`crates/core/src/layout.rs`](file:///g:/Codes/winbosk/crates/core/src/layout.rs)；
  2. 边界磁吸、推离防重叠与夹屏约束：修改 [`crates/core/src/magnet.rs`](file:///g:/Codes/winbosk/crates/core/src/magnet.rs)；
  3. 运行对应的纯算法单测：`cargo test -p winbosk-core -- magnet`。
- **处理 Shell 交互、外部文件拖拽入栅栏**：
  1. 壳层底层提取与项构建：[`crates/shell/src/items.rs`](file:///g:/Codes/winbosk/crates/shell/src/items.rs)；
  2. 文件复制与内部库同步逻辑：[`crates/app/src/file_ops.rs`](file:///g:/Codes/winbosk/crates/app/src/file_ops.rs)；
  3. 窗口拖放消息接收与命中栅栏反查：[`crates/render/src/overlay.rs`](file:///g:/Codes/winbosk/crates/render/src/overlay.rs)（`WM_DROPFILES`）。

---

## 实施计划（Plan）编写规范与审查标准

凡涉及跨模块重构、交互逻辑调整、状态机变更或非平凡功能实施，必须严格遵循 [`docs/engineering-plan-guidelines.md`](file:///g:/Codes/winbosk/docs/engineering-plan-guidelines.md) 的编制规范与「隐性动力学自查六律」（常驻后台冲突、0% 空闲 CPU、孤儿清理、语义精准定位、状态全集校验、旁路数据对齐）。实施完成后必须派发独立干净上下文的子代理执行对抗性代码审查。

---

## 环境前置条件

- **操作系统**：Windows 10 / Windows 11（x86_64，MSVC 工具链）。
- **Rust 版本**：Edition 2021（`Cargo.toml` 声明），需安装 stable 工具链。
- **关键环境变量**：
  - `WINBOSK_AUTOSTOP_MS`：自动化测试退出钩子。值为毫秒数（例如 `2000`）。设置后进程到时自动向主循环发送 `WM_APP_QUIT` 干净退出，无需人工干预。
  - `WINBOSK_WINSDK_ROOT`：**仅**在 `cargo build` 报「winres 嵌入图标失败 / 找不到 rc.exe」时才需要。显式指定 Windows SDK 的 rc 目录（`build.rs` 会用它调用 `winres::set_toolkit_path`）。正常情况下 `build.rs` 会自动探测（`WindowsSdkDir` → `%ProgramFiles(x86)%\Windows Kits\10\bin\<版本>\x64`），无需手工设置。

---

## 测试约定

- `cargo test --workspace` 可直接运行全部测试。
- `winbosk-core` 内所有测试均为纯内存测试，瞬时完成，零外部依赖。
- `winbosk-shell` 中的单元测试包含只读探测真实系统的测试（如 `live_enumerate_real_desktop`），此类测试仅做只读枚举，不会变动系统桌面文件。

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
  - **该行的排版契约（plan 09 立，改它之前先读 [`docs/plans/09-storage-row-ux.md`](file:///g:/Codes/sylva/docs/plans/09-storage-row-ux.md)）**：
    值行 = `文件位置` 标签 + 状态标签（「应用内部库」/「外部文件夹」）+ 真实路径；
    动作行 = 后果提示（左，宽度放不下时整条不画）+ 「更改文件位置…」「恢复默认」（右端对齐）。
    **值行是零按钮的纯信息行**：状态标签**不是热区**；路径本身是热区，点击 = 在资源管理器里
    打开该落地目录（hover 提亮 + 下划线把"可点"画出来）。
    **路径必须吃满值行剩余宽度**（右缘精确落在详情区内缘）——值行里塞任何占位控件都会从路径
    身上抢宽度：实测一个 40·s 的按钮会让默认面板宽下的库路径从"完整显示（40 字）"退化成
    23 字，故「打开」按钮已被移除（由路径承担该动作）。
    命中表里不得再出现「路径 → ChangeStoragePath」这种"看得见的值是隐形按钮"的接法——
    路径只接**无害且可逆**的 `OpenStoragePath`，破坏性的搬文件动作必须留在动作行的具名按钮上。
    值行内三段 detail 字号文字（行标签 / 状态标签 / 路径）**共用唯一顶线**
    `SceneFenceDetail::storage_text_top`，不要再给任何一段自带偏移。
  - 该行仍是**两行**：`detail_visible_rows` 的 `n += 2` 与 `build_console` 的 `row += 2` 同步，
    详情区行预算 `24 + n × 30 ≤ 278` 在侧边栏布局下已用满 8 行，**不允许再加行**。
- **Reset Storage（恢复默认）**：把外部文件夹模式还原为应用内部模式。**不移动、不复制、不删除
  任何磁盘文件**；原本镜像自该文件夹的成员从栅栏摘除（文件留在原处）。桌面镜像栅栏
  （`storage_path == 真实桌面目录`）**禁止**执行——会让栅栏清空而真实图标正被壳层隐藏。

---

## 本文档有意未包含的内容

- 项目详细背景、功能特性清单与视觉效果图：见 [`README.md`](file:///g:/Codes/winbosk/README.md)
- 完整依赖项清单与第三方库具体版本：见 [`Cargo.toml`](file:///g:/Codes/winbosk/Cargo.toml)
- 软件工程实施计划编写规范与自查六律：见 [`docs/engineering-plan-guidelines.md`](file:///g:/Codes/winbosk/docs/engineering-plan-guidelines.md)
- 架构设计演进历史与废弃技术方案（如 egui 移除记录）：见 [`docs/design/2026-08-14-desktop-fence-organizer-design.md`](file:///g:/Codes/winbosk/docs/design/2026-08-14-desktop-fence-organizer-design.md)
- 完整 CI 自动化流水线配置：见 [`.github/workflows/ci.yml`](file:///g:/Codes/winbosk/.github/workflows/ci.yml)
- 安装器实现细节与打包细节：见 [`scripts/installer/`](file:///g:/Codes/winbosk/scripts/installer/) 与 [`scripts/package-msix.ps1`](file:///g:/Codes/winbosk/scripts/package-msix.ps1)
