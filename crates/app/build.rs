// 仅 Windows：把 winbosk.ico 编译进 exe 资源，并内嵌 DPI 感知清单。
// 资源 ID 1 = 主图标（Explorer / 文件属性 / Alt+Tab / 任务栏默认都读它）。
//
// 关键：winres 默认**不**内嵌任何清单——没有 DPI 感知清单时，进程被 Windows 判定为
// DPI 非感知/系统感知，高缩放下 DWM 把整窗位图缩放（整窗发糊 + 拖拽/动画时重采样
// 与合成竞争 → 闪烁）。这里显式嵌入 Per-Monitor v2 清单：窗口按真实物理像素渲染，
// 跨显示器 DPI 变化由 `WM_DPICHANGED` 实时重排（与 scripts/installer 做法对齐）。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    // 图标收在仓库顶层 `assets/`（crates/app 的上一级两级 = 仓库根）。
    let icon = std::path::Path::new(&manifest)
        .join("..")
        .join("..")
        .join("assets")
        .join("winbosk.ico");
    let mut res = winres::WindowsResource::new();
    // 工具链目录：winres 默认靠 `reg.exe` 查注册表定位 Windows SDK。受限环境（禁用
    // `reg.exe` 的沙箱、精简 CI 镜像）下查询失败 → 工具链目录为空 → 退化成相对路径
    // `bin\x64\rc.exe`，报「系统找不到指定的路径。(os error 3)」而中断构建。
    // 这里给一条不依赖注册表的回退；探测不到时不干预，仍交由 winres 自身逻辑。
    if let Some(rc_dir) = locate_winsdk_rc_dir() {
        println!("[build.rs] 使用探测到的 Windows SDK rc 目录: {rc_dir}");
        res.set_toolkit_path(&rc_dir);
    }
    // 路径交给 rc.exe 时按字面使用：直接给绝对路径，避免 cwd 歧义
    res.set_icon(&icon.to_string_lossy());
    res.set_manifest(MANIFEST);
    match res.compile() {
        Ok(()) => {}
        Err(e) => {
            // 找不到 rc.exe 等工具链问题时给出明确提示，不静默失败
            eprintln!(
                "[build.rs] winres 嵌入图标失败（{}）——检查 MSVC/Windows SDK 资源编译器",
                e
            );
            std::process::exit(1);
        }
    }
}

/// 不依赖注册表地定位 Windows SDK 的 **rc 目录**——即 winres 要求的
/// `toolkit_path` 语义：直接含 `rc.exe` 的目录（形如 `<kits>\bin\<版本>\x64`）。
///
/// 顺序（先显式、后约定、最后扫描默认安装位置；都失败返回 `None`，不干预 winres
/// 自身的注册表逻辑）：
/// 1. `WINBOSK_WINSDK_ROOT` —— 环境变量显式指定，供 CI / 受限环境使用；
/// 2. `WindowsSdkDir` —— VS 开发者提示符 / `vcvars*.bat` 会设置；
/// 3. 扫描 `%ProgramFiles(x86)%\Windows Kits\10\bin\*`，取版本号最大的一个。
///
/// 每次构建都要判断，故保持廉价：只做若干次 `exists()` 探测。三条来源都接受
/// 「SDK 根 / `bin\<版本>` / `<版本>\x64`」三种形状，内部统一下探到 rc 目录。
fn locate_winsdk_rc_dir() -> Option<String> {
    use std::path::{Path, PathBuf};

    /// 从候选路径下探出全部「直接含 rc.exe 的目录」，并带上版本号用于择优。
    ///
    /// 兼容三种形状：`<x64 目录>`（自身含 rc.exe）、`<kits>\bin\<版本>`（其下
    /// `x64\rc.exe`）、`<kits>`（其下 `bin\<版本>\x64\rc.exe`）。
    fn rc_dirs(base: &Path) -> Vec<(Vec<u64>, PathBuf)> {
        let mut out = Vec::new();
        if base.join("rc.exe").exists() {
            out.push((Vec::new(), base.to_path_buf()));
        }
        if base.join("x64").join("rc.exe").exists() {
            out.push((Vec::new(), base.join("x64")));
        }
        if let Ok(entries) = std::fs::read_dir(base.join("bin")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                // 只认 10.x 的 SDK（本工程要求 Win10+，更早的布局不在此形态内）
                if !name.starts_with("10.") {
                    continue;
                }
                let x64 = entry.path().join("x64");
                if x64.join("rc.exe").exists() {
                    let ver = name.split('.').map(|p| p.parse().unwrap_or(0)).collect();
                    out.push((ver, x64));
                }
            }
        }
        out
    }

    /// 同源候选里取版本号最大的一个 rc 目录。
    fn pick(cands: Vec<(Vec<u64>, PathBuf)>) -> Option<PathBuf> {
        cands
            .into_iter()
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, dir)| dir)
    }

    for var in ["WINBOSK_WINSDK_ROOT", "WindowsSdkDir"] {
        if let Ok(p) = std::env::var(var) {
            if let Some(dir) = pick(rc_dirs(Path::new(&p))) {
                return Some(dir.to_string_lossy().into_owned());
            }
        }
    }
    for pf in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Ok(base) = std::env::var(pf) else {
            continue;
        };
        let kits = Path::new(&base).join("Windows Kits").join("10");
        if let Some(dir) = pick(rc_dirs(&kits)) {
            return Some(dir.to_string_lossy().into_owned());
        }
    }
    None
}

/// Per-Monitor v2 感知清单：高缩放下原生渲染（清晰），跨显示器 DPI 变化实时重排。
/// `true/pm`（SMI/2005）供 Win8.1 回退，`PerMonitorV2`（SMI/2016）为 Win10 1703+ 主路径。
/// 与 scripts/installer 的清单一致，仅 assemblyIdentity 名称不同。
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="WinBosk.App" version="0.1.0.0" processorArchitecture="*"/>
  <description>WinBosk Desktop Fences</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>"#;
