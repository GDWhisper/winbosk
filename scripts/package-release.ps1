# 打包 Release 发布资产：
# 1. 验证 target\release\winbosk.exe 与 scripts\installer\target\release\winbosk-installer.exe
# 2. 生成便携版压缩包 WinBosk-<Tag>-win64.zip (包含 WinBosk\winbosk.exe 等文件)
# 3. 复制安装包 WinBosk-Setup-<Tag>.exe
# 4. 复制独立可执行文件 winbosk-<Tag>-x64.exe
# 5. 生成 SHA256 校验和文件 SHA256SUMS.txt
#
# 用法：
#   pwsh -ExecutionPolicy Bypass -File scripts\package-release.ps1 -Tag "v0.1.0"
param(
    [string]$Tag = 'v0.1.0',
    [string]$OutDir = 'dist'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "==> 开始打包 Release 资产，版本标签：$Tag"

$mainExe = Join-Path $root 'target\release\winbosk.exe'
if (-not (Test-Path $mainExe)) {
    throw "未找到主程序二进制文件：$mainExe。请先执行 cargo build --release"
}

$installerExe = Join-Path $root 'scripts\installer\target\release\winbosk-installer.exe'
$hasInstaller = Test-Path $installerExe
if (-not $hasInstaller) {
    Write-Warning "未找到安装程序二进制文件：$installerExe。将跳过安装程序打包。"
}

$targetDist = Join-Path $root $OutDir
if (Test-Path $targetDist) {
    # 清理 dist 目录下旧的打包文件
    Get-ChildItem -Path $targetDist -File | Remove-Item -Force
} else {
    New-Item -ItemType Directory -Force -Path $targetDist | Out-Null
}

# 1. 打包便携版 ZIP (结构：WinBosk\winbosk.exe，与 winget 清单契约一致)
Write-Host "==> 生成便携版 ZIP 压缩包..."
$stagingDir = Join-Path $targetDist 'staging\WinBosk'
if (Test-Path $stagingDir) { Remove-Item -Path $stagingDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null

Copy-Item $mainExe (Join-Path $stagingDir 'winbosk.exe') -Force
$readme = Join-Path $root 'README.md'
if (Test-Path $readme) { Copy-Item $readme (Join-Path $stagingDir 'README.md') -Force }
$license = Join-Path $root 'LICENSE'
if (Test-Path $license) { Copy-Item $license (Join-Path $stagingDir 'LICENSE') -Force }

$zipPath = Join-Path $targetDist "WinBosk-$Tag-win64.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
Compress-Archive -Path $stagingDir -DestinationPath $zipPath -Force
Remove-Item (Join-Path $targetDist 'staging') -Recurse -Force
Write-Host "    已生成: $zipPath"

# 2. 复制独立可执行文件
$standaloneExe = Join-Path $targetDist "winbosk-$Tag-x64.exe"
Copy-Item $mainExe $standaloneExe -Force
Write-Host "    已生成: $standaloneExe"

# 3. 复制安装程序
if ($hasInstaller) {
    $setupExe = Join-Path $targetDist "WinBosk-Setup-$Tag.exe"
    Copy-Item $installerExe $setupExe -Force
    Write-Host "    已生成: $setupExe"
}

# 4. 生成 SHA-256 校验清单
Write-Host "==> 计算 SHA-256 校验和..."
$checksumFile = Join-Path $targetDist 'SHA256SUMS.txt'
$filesToHash = Get-ChildItem -Path $targetDist -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object Name

$lines = @()
foreach ($file in $filesToHash) {
    $hash = (Get-FileHash -Path $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    $line = "$hash  $($file.Name)"
    $lines += $line
    Write-Host "    $line"
}
[System.IO.File]::WriteAllLines($checksumFile, $lines, [System.Text.Encoding]::UTF8)

Write-Host "==> 打包完成！输出目录：$targetDist"
