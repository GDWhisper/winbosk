# 发布指导（Release Guide）

> 本文件是 WinBosk 发版流程的唯一记录。**结论先行：发版走 GitHub Actions，不在本机构建 release 产物。**

---

## 1. 为什么不在本机构建 release

| # | 本机发版的障碍 | 表现 | 后果 |
| :-- | :-- | :-- | :-- |
| 1 | **Windows 锁住正在运行的可执行映像** | 链接阶段 `error: failed to remove file …\target\release\winbosk.exe` / `拒绝访问 (os error 5)`（`LNK1104` 的 cargo 等价物） | 必须先干净退出运行中的实例才能构建；漏一步就整次构建白跑 |
| 2 | **退出实例本身有代价** | 硬杀会跳过 `Drop`，`IconGuard` 不恢复 → 真实桌面图标永久隐藏 | 要么走 `WM_APP_QUIT`（默认热键 `Ctrl+Shift+F10` / 托盘退出）干净退出，要么手工把 `SysListView32` 显示回来 |
| 3 | **旧实例还占着单实例互斥** | 新实例静默退出，看起来「启动没反应」 | 桌面验证前也必须先退出旧实例 |
| 4 | **本机构建受沙箱与增量缓存影响** | 沙箱内 cargo 增量会话收尾写入被拒 → `os error 5` → 半写会话 → 下次真编译 rustc ICE | 见 `AGENTS.md`「构建环境硬约束」；排查成本远高于收益 |

CI runner（`windows-latest`）是全新 checkout + 空 `target/`，上面四条一条都不成立。**发版产物一律由 CI 产出，本机只做开发期 debug 构建与验证。**

---

## 2. 发布流水线

工作流：[`.github/workflows/release.yml`](file:///g:/Codes/sylva/.github/workflows/release.yml)

**触发方式（二选一）**：

1. **打标签推送**（推荐，正式发版）：`git tag v0.1.0 && git push origin v0.1.0`。标签必须匹配 `v*`。
2. **Actions 页面手动触发**（`workflow_dispatch`）：填 `tag` 输入框，例如 `v0.1.0`。用于试跑，**不会**创建 GitHub Release（该步骤有 `startsWith(github.ref, 'refs/tags/')` 守卫），只上传 artifact。

**执行步骤**：

| 步骤 | 命令 / 动作 | 说明 |
| :-- | :-- | :-- |
| 1 | `actions/checkout@v4` | |
| 2 | `dtolnay/rust-toolchain@stable` | |
| 3 | `actions/cache@v4` | 缓存 `~/.cargo/registry`、`~/.cargo/git`、`target`、`scripts/installer/target` |
| 4 | 解析发布标签 | tag 事件取 `GITHUB_REF_NAME`；dispatch 取输入框；都没有则兜底 `v0.1.0` |
| 5 | `cargo build --release` | 主程序，输出 `target\release\winbosk.exe` |
| 6 | `cargo build --release --manifest-path scripts/installer/Cargo.toml` | 安装器**不在根 workspace**，必须单独编 |
| 7 | `scripts\package-release.ps1 -Tag <tag>` | 打包便携版 ZIP / 单文件 exe / 安装包 + `SHA256SUMS.txt` |
| 8 | `actions/upload-artifact@v4` | 上传 `dist/*` |
| 9 | `softprops/action-gh-release@v2` | 仅 tag 事件：建 Release、附全部产物、`generate_release_notes: true` |

**产物清单**（`dist/`，四个文件）：

- `WinBosk-Setup-<tag>.exe` — 图形化安装包，推荐日常使用；
- `WinBosk-<tag>-win64.zip` — 绿色便携版，目录结构 `WinBosk\winbosk.exe`（与 winget 清单契约一致）；
- `winbosk-<tag>-x64.exe` — 单文件版；
- `SHA256SUMS.txt` — 校验清单。

---

## 3. 发版前检查清单

按顺序过一遍，任何一条不过就不要打标签：

1. **`main` 干净**：`git status` 无未提交改动。注意 `.dsh-acl-reports/` 之类工具产出的目录不该进仓库（目前未加入 `.gitignore`，提交前手工排除）。
2. **四道门禁全过**：`cargo build --workspace` + `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --all -- --check`。
3. **CI 覆盖确认**：见下方「已知缺口」——直推 `main` 不触发 CI，门禁要么本地跑，要么走 PR 让 CI 跑。
4. **版本号与标签一致**：标签 `v0.1.0` 会直接进产物文件名，打错标签只能删标签重打（已发布的 Release 无法改名）。
5. **安装器能编**：步骤 6 是独立工程，根 `cargo build --workspace` 不覆盖它。本地预检可用
   `cargo build --release --manifest-path scripts/installer/Cargo.toml`。

---

## 4. 本地构建（仅调试 / 预检用）

以下命令**只用于开发期验证**，不要用它们的产物发版：

```powershell
# debug 构建 + 2 秒自动退出验证（不会长时间占住桌面）
$env:WINBOSK_AUTOSTOP_MS="2000"; .\target\debug\winbosk.exe

# release 预检（必须先干净退出正在运行的实例，否则 LNK1104）
cargo build --release
```

`scripts\build-release.ps1`（编译 release 并拷到 `dist\`）与 `scripts\package-release.ps1`（打包 + 校验和）是 CI 步骤 5 / 7 的本机复刻，仅用于在打标签前本地试跑打包逻辑。**跑之前同样要先退出运行实例。**

`cargo build` 报 `winres 嵌入图标失败 / 找不到 rc.exe` 时，设 `WINBOSK_WINSDK_ROOT` 指向 Windows SDK 的 `rc` 目录（见 `AGENTS.md`「环境前置条件」）。

---

## 5. 已知缺口

- **直推 `main` 不跑 CI**：[`ci.yml`](file:///g:/Codes/sylva/.github/workflows/ci.yml) 的触发条件是 `pull_request` + `workflow_dispatch`，没有 `push`。本仓库惯例是直接提交到 `main`，因此 CI 实际不会自动执行——门禁靠本地跑，或临时开一个 PR。要改成「推送即跑」，给 `ci.yml` 补 `push: branches: [main]` 即可；这是流水线行为变更，未擅自改动，留待决定。
- **release 工作流不跑测试**：步骤 5 直接 `cargo build --release`，没有 `cargo test`。门禁由 `ci.yml` 负责，两条流水线是分开的。

---

## 6. 相关文档

- CI 配置：[`.github/workflows/ci.yml`](file:///g:/Codes/sylva/.github/workflows/ci.yml)
- 发布工作流：[`.github/workflows/release.yml`](file:///g:/Codes/sylva/.github/workflows/release.yml)
- 构建环境硬约束与 LNK1104 口径：[`AGENTS.md`](file:///g:/Codes/sylva/AGENTS.md)「常用命令」与「构建环境硬约束」
- MSIX / Winget 打包：[`scripts/package-msix.ps1`](file:///g:/Codes/sylva/scripts/package-msix.ps1)、[`scripts/winget-submit.ps1`](file:///g:/Codes/sylva/scripts/winget-submit.ps1)
- 安装器工程：[`scripts/installer/`](file:///g:/Codes/sylva/scripts/installer/)
