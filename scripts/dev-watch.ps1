# 开发热重载：监听源码变更 → 重编 dev-fast → 优雅重启运行实例。
#
# 用法：powershell -ExecutionPolicy Bypass -File scripts\dev-watch.ps1
#       （Ctrl+C 结束监听；结束前会保留当前正在运行的实例）
#
# ── 为什么「cargo 产物」与「运行实例」是两个文件名（本次修复的核心） ──────────
#   Windows 禁止删除/覆盖正在运行的映像（DeleteFile → os error 5 拒绝访问）。
#   过去脚本直接运行 cargo 的产物 target\<profile>\winbosk.exe，于是实例在跑时，
#   cargo 链接阶段必然失败：
#       error: failed to remove file `G:\Codes\winbosk\target\dev-fast\winbosk.exe`
#       Caused by: 拒绝访问。 (os error 5)
#   现在把两者解耦：
#       · cargo 只写 target\<profile>\winbosk.exe     —— 构建期没有任何进程占用它；
#       · 真正运行的实例是 target\<profile>\winbosk-run.exe —— 被占用也不影响构建。
#   流程：cargo build（旧实例在不在跑都能成功）→ 构建成功才优雅停旧实例
#         → 覆盖 winbosk-run.exe → 启动；构建失败则原样保留运行中的实例。
#   注意：winbosk-run.exe 与 winbosk.exe 同目录，因此「<exe 同级>/data」数据目录不变
#         （见 crates/app/src/main.rs:299），栅栏配置与内部库都不会跑丢。
#
# ── 为什么不能简单地「杀掉再启动」（均已实测） ─────────────────────────────
#   1. WinBosk 有单实例互斥（`WinBosk.Desktop.Fences`，见 crates/app/src/main.rs:258）。
#      旧实例没退干净就启动新实例，新实例会直接自退 —— 必须等旧进程真正结束。
#   2. 硬杀（Stop-Process -Force / taskkill /F）会跳过 `IconGuard::drop`，
#      被隐藏的真实桌面图标将不会恢复（见 crates/render/src/overlay.rs:1280 的注释）。
#   3. `taskkill /IM winbosk.exe`（不带 /F，本意是投递 WM_CLOSE）实测**不可靠**：
#      进程没有退出，随后静默消失且图标仍处于隐藏状态。不要用。
#
# 正确做法（已实测 PASS）：向 overlay 窗口投递 `WM_APP_QUIT`（0x8001）——
#   overlay.rs:1276 的该分支会 PostQuitMessage(0)，走干净退出，RAII 恢复桌面图标。
param(
    [string]$Profile         = 'dev-fast',
    [int]$PollMs             = 500,
    [int]$ExitWaitSec        = 10,
    [int]$SettleMs           = 300,
    [int]$SettleMaxTries     = 20
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$exe    = Join-Path $root "target\$Profile\winbosk.exe"      # cargo 产物（构建期必须空闲）
$runExe = Join-Path $root "target\$Profile\winbosk-run.exe"  # 实际运行的映像（可被占用）

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host '==> 找不到 cargo：请在「Developer PowerShell for VS」或已配置 Rust 工具链的终端里运行本脚本。'
    exit 1
}

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinBoskWin {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string w);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static IntPtr Overlay() { return FindWindowW("WinBoskOverlay", null); }
}
"@

$WM_APP_QUIT = 0x8001

# 按进程名匹配（而不是按路径）：手工从别处启动的实例也要能被优雅接管，
# 否则新实例会撞上单实例互斥后静默自退。
function Get-WinBoskProcess {
    @(Get-Process -Name 'winbosk', 'winbosk-run' -ErrorAction SilentlyContinue)
}

# 优雅关闭正在运行的实例，并等待其完全退出。返回 $true 表示可以安全接管。
function Stop-WinBosk {
    if ((Get-WinBoskProcess).Count -eq 0) { return $true }

    $hwnd = [WinBoskWin]::Overlay()
    if ($hwnd -eq [IntPtr]::Zero) {
        Write-Host '==> 警告：实例在运行，但找不到 WinBoskOverlay 窗口，无法安全关闭。'
        Write-Host '    请手动按 Ctrl+Shift+F10 退出后再保存文件。本次不重启。'
        return $false
    }

    Write-Host '==> 优雅关闭旧实例（WM_APP_QUIT，保证恢复桌面图标）'
    [void][WinBoskWin]::PostMessageW($hwnd, $WM_APP_QUIT, [IntPtr]::Zero, [IntPtr]::Zero)

    $deadline = (Get-Date).AddSeconds($ExitWaitSec)
    while ((Get-WinBoskProcess).Count -gt 0 -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }

    if ((Get-WinBoskProcess).Count -gt 0) {
        Write-Host "==> 警告：旧实例 ${ExitWaitSec}s 内未退出（可能卡在模态菜单）。"
        Write-Host '    请手动按 Ctrl+Shift+F10 退出后再保存文件；本次不重启，避免单实例自退。'
        return $false
    }
    return $true
}

function Build-And-Run {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Write-Host "==> cargo build --profile $Profile"
    cargo build --profile $Profile
    if ($LASTEXITCODE -ne 0) {
        Write-Host '==> 构建失败，保留当前运行实例（不重启）'
        return
    }
    $sw.Stop()

    if (-not (Test-Path $exe)) {
        Write-Host "==> 异常：构建成功但未找到 $exe，本次不重启。"
        return
    }

    # 构建成功后才动运行中的实例：失败路径完全不碰它。
    if (-not (Stop-WinBosk)) { return }

    # 旧实例已退出，winbosk-run.exe 不再被占用，可以安全覆盖。
    try {
        Copy-Item -LiteralPath $exe -Destination $runExe -Force
    } catch {
        Write-Host "==> 无法覆盖 $runExe：$($_.Exception.Message)"
        Write-Host "    新产物已就绪（target\$Profile\winbosk.exe），可手动运行它或重新执行本脚本。"
        return
    }

    Start-Process -FilePath $runExe -WorkingDirectory (Split-Path -Parent $runExe) | Out-Null
    Write-Host "==> 已启动 winbosk-run.exe（构建 $([math]::Round($sw.Elapsed.TotalSeconds,1))s）"
}

# 源码指纹：crates/ 下所有 .rs 与 .toml 的「路径 + 最后写入时间」。
# 用轮询而非 FileSystemWatcher：事件去抖/丢失的边界情况更少，且开销可忽略。
function Get-SourceStamp {
    $files = Get-ChildItem -Path (Join-Path $root 'crates') -Recurse -File -Include '*.rs', '*.toml'
    $sb = [System.Text.StringBuilder]::new()
    foreach ($f in ($files | Sort-Object FullName)) {
        [void]$sb.Append($f.FullName).Append('|').Append($f.LastWriteTimeUtc.Ticks).Append("`n")
    }
    return $sb.ToString()
}

Write-Host "==> 监听中：$root\crates（Ctrl+C 结束）"
# 先取基线再构建：若首次构建期间又改了文件，下一轮能立刻补一次重建。
$stamp = Get-SourceStamp
Build-And-Run

while ($true) {
    Start-Sleep -Milliseconds $PollMs
    $now = Get-SourceStamp
    if ($now -eq $stamp) { continue }

    # 去抖：编辑器保存（含格式化 / 双写）常在几十毫秒内连续多次改盘，
    # 等指纹连续稳定一个窗口后再构建，避免一次保存触发多次构建。
    $stamp = $now
    $tries = 0
    while ($tries -lt $SettleMaxTries) {
        $tries++
        Start-Sleep -Milliseconds $SettleMs
        $next = Get-SourceStamp
        if ($next -eq $stamp) { break }
        $stamp = $next
    }

    Build-And-Run
    # 刻意不在此处重新基线：若构建期间源码又被改动，下一轮会再触发一次重建。
    # （旧版在这里重置基线，会把「构建期间的最后一次编辑」永久吞掉。）
}
