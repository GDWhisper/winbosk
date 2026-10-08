# 实施记录 15：退出生命周期根治与控制中心显式退出交互

## 1. 背景与根因

用户在执行 `cargo build --release` 时偶发 `拒绝访问 (os error 5): failed to remove file target\release\winbosk.exe`，反馈已手动退出但仍被占锁。

现场与代码排查发现双重原因：
1. **交互认知误导**：控制中心标题栏右上角带有红底「✕」按钮，用户习惯性误认为是“关闭/退出 WinBosk”；但实际上该按钮仅执行 `set_console_open(rt, false)`（收起控制面板），桌面栅栏与后台进程依然常驻。
2. **底层 CRT ExitProcess 死锁漏洞**：若从托盘退出，`main()` 自然 return 时触发 Windows CRT `ExitProcess`。其标准流程为先杀除当前线程外的所有线程，再获取 Loader Lock（`ntdll!LdrpLoaderLock`）调用 DLL 的 `DLL_PROCESS_DETACH`。系统中的第三方注入 DLL（RTSSHooks64、显卡驱动、外壳扩展等）极易在 detach 时死锁；而此前设置的 45 秒兜底强杀线程在第一步就被系统杀死了，导致进程永久沦为无界面、占文件锁的僵尸进程。另外，45 秒兜底等待也过长。

---

## 2. 实施改动清单

### A. 退出生命周期根治（[`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs)）
1. **进程内核级瞬时自杀**：在 `main()` 尾部，正常完成三阶段资源有序 Drop 后，显式 `drop(_guard)`（刷盘所有收尾日志）和 `CloseHandle(_mutex)`（释放单实例互斥句柄），随后立即调用 `TerminateProcess(GetCurrentProcess(), 0)`。彻底绕过脆弱的 CRT `ExitProcess` 与第三方 DLL `DLL_PROCESS_DETACH` 加载器锁死锁，由 Windows 内核直接回收所有资源与文件锁。
2. **看门狗强杀超时压缩**：将启动期创建的退出超时兜底常数从 `EXIT_WARN_MS = 15_000` / `EXIT_FORCE_MS = 45_000` 压缩至 `2_000`（2 秒警告）和 `5_000`（5 秒强杀），确保极端卡顿下最多 5 秒内必放文件锁。

### B. 消除交互认知误区（[`crates/render/src/`](file:///g:/Codes/sylva/crates/render/src/) & [`crates/app/src/`](file:///g:/Codes/sylva/crates/app/src/)）
1. **控制中心标题栏收起按钮去警示化**：将控制中心右上角「✕」按钮的 hover 背景色从误导性的红色（`[0.85, 0.28, 0.28]`）调整为中性半透明浅白高亮（`[1.0, 1.0, 1.0, 0.15]`），与设置按钮风格保持一致，表明其为“面板收起”而非“杀进程”。
2. **设置页提供显式退出按钮**：
   - 在 [`crates/render/src/overlay.rs`](file:///g:/Codes/sylva/crates/render/src/overlay.rs) 新增 `ConsoleZone::QuitApp`；
   - 在 [`crates/render/src/scene.rs`](file:///g:/Codes/sylva/crates/render/src/scene.rs) 和 [`crates/app/src/scene.rs`](file:///g:/Codes/sylva/crates/app/src/scene.rs) 调整设置页高度与布局，在底部新增全宽危险操作按钮「⏻ 退出 WinBosk (Ctrl+Shift+F10)」；
   - 在 [`crates/render/src/draw.rs`](file:///g:/Codes/sylva/crates/render/src/draw.rs) 绘制红底/浅红描边的危险操作按钮样式，hover 时高亮红底；
   - 在 [`crates/app/src/main.rs`](file:///g:/Codes/sylva/crates/app/src/main.rs) 的 `ConsoleClick` 事件中接入，点击即向主窗口投递 `WM_APP_QUIT` 干净退出。

---

## 3. 验收与门禁

- `cargo test --workspace`：全工作区 178+ 单元测试全部通过（含更新后的设置页自适应高度契约测试 `console_settings_page_height_contract` 424.0px、`quit_btn` 几何契约测试）。
- `cargo clippy --workspace -- -D warnings`：零警告。
- `cargo fmt --all -- --check`：格式完全吻合。
- `cargo build --release`：成功编译输出，无文件锁冲突。
- 自动化生命周期退出测试：`$env:WINBOSK_AUTOSTOP_MS="1500"` 顺利退出并释放全部句柄。
