//! GPU 上下文：D3D11 设备 + D2D/DWrite 工厂 + WinRT 合成器 + 合成图形设备。
//!
//! 一个进程一个实例，跨帧复用。底层 D3D11 / DXGI / D2D 设备必须存活到
//! 所有合成对象释放为止，因此一并持有。
//!
//! WinRT `Windows.UI.Composition` 要求当前线程先初始化 COM + DispatcherQueue
//! （探针实测：缺 DispatcherQueue 时 `Compositor::new()` 返回 E_ACCESSDENIED）。
//! DispatcherQueue 控制器必须存活到合成器销毁，这里一并持有。

use windows::core::{Error, Interface, Result};
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Device, ID2D1Factory, ID2D1Factory1, D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_ISOLATED,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_DRIVER_INTERNAL_ERROR,
};
use windows::Win32::System::WinRT::Composition::ICompositorInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_STA, DQTYPE_THREAD_CURRENT,
};
use windows::UI::Composition::{CompositionGraphicsDevice, Compositor as WinCompositor};

/// 渲染设备集合。
pub struct RenderDevice {
    /// D2D 工厂（构造 D2D 设备用；绘制表面直接由合成图形设备提供设备上下文）。
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    /// WinRT 合成器（`Windows.UI.Composition`）。创建视觉树、效果工厂、绘制表面。
    pub compositor: WinCompositor,
    /// 合成图形设备：`CreateDrawingSurface` 创建内容/区域绘制表面。
    pub gfx_device: CompositionGraphicsDevice,
    // 保持底层对象存活（合成器 / D2D 设备 / D3D11 / DXGI / DispatcherQueue 都依赖）
    #[allow(dead_code)]
    _d2d_device: ID2D1Device,
    #[allow(dead_code)]
    _d3d: ID3D11Device,
    #[allow(dead_code)]
    _dxgi: IDXGIDevice,
    #[allow(dead_code)]
    _dq: DispatcherQueueController,
}

impl RenderDevice {
    /// 创建全部 GPU 上下文。失败通常意味着无硬件加速（远程桌面/虚拟机），
    /// 调用方可降级处理。
    ///
    /// 要求调用线程已 `CoInitializeEx`（STA）——应用在 `shell::com::init` 完成；
    /// 本方法再补上线程的 DispatcherQueue，满足 `Compositor::new()` 的前置条件。
    pub fn new() -> Result<Self> {
        // WinRT 合成器要求当前线程先初始化 DispatcherQueue（STA）。
        // 控制器须存活到合成器销毁。
        let dq: DispatcherQueueController = unsafe {
            CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_STA,
            })?
        };

        // D3D11 设备需带 BGRA 支持才能被 D2D 使用。
        let d3d = create_d3d_device()?;

        let dxgi: IDXGIDevice = d3d.cast()?;
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let d2d1: ID2D1Factory1 = d2d.clone().cast()?;
        let d2d_device: ID2D1Device = unsafe { d2d1.CreateDevice(&dxgi)? };
        // 独立工厂：不依赖进程 MTA（我们以 STA 初始化 COM），也避免与其他进程共享
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED)? };

        // WinRT 合成器 + 合成图形设备（经 ICompositorInterop 绑定 D2D 设备）
        let compositor = WinCompositor::new()?;
        let c_interop: ICompositorInterop = compositor.cast()?;
        let gfx_device = unsafe { c_interop.CreateGraphicsDevice(&d2d_device)? };

        Ok(Self {
            d2d,
            dwrite,
            compositor,
            gfx_device,
            _d2d_device: d2d_device,
            _d3d: d3d,
            _dxgi: dxgi,
            _dq: dq,
        })
    }

    /// GPU 设备丢失后的重建：只换绑依赖 D3D11 设备的部分（D3D11 → DXGI → D2D
    /// 设备 → 合成图形设备）。D2D / DWrite 工厂与 WinRT 合成器不绑定具体 D3D
    /// 设备，连同 DispatcherQueue 一并保留。
    ///
    /// 驱动重载（显卡驱动更新 / TDR / GPU 重置）后旧设备永久失效，
    /// `DXGI_ERROR_DEVICE_REMOVED` 不会自愈。不换绑的后果是进程「活着但画不出
    /// 任何东西」：热键与托盘照响、事件回路照跑，画面全停。
    pub fn rebind_graphics_device(&mut self) -> Result<()> {
        let d3d = create_d3d_device()?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        let d2d1: ID2D1Factory1 = self.d2d.clone().cast()?;
        let d2d_device: ID2D1Device = unsafe { d2d1.CreateDevice(&dxgi)? };
        // 图形设备从**既有**合成器重新创建：合成器与 DispatcherQueue 不随设备
        // 丢失失效，重建合成器反而要重新挂 DesktopWindowTarget（会闪烁）。
        let c_interop: ICompositorInterop = self.compositor.cast()?;
        let gfx_device = unsafe { c_interop.CreateGraphicsDevice(&d2d_device)? };
        self._d3d = d3d;
        self._dxgi = dxgi;
        self._d2d_device = d2d_device;
        self.gfx_device = gfx_device;
        tracing::info!("渲染设备已换绑新图形设备");
        Ok(())
    }

    /// 设备移除原因（`GetDeviceRemovedReason`）。设备健康时返回 `Ok(())`；
    /// 丢失时返回带 HRESULT 的 `Err`（TDR / 驱动更新 / 硬件问题的区别在这里）。
    pub fn device_removed_reason(&self) -> Result<()> {
        unsafe { self._d3d.GetDeviceRemovedReason() }
    }
}

/// 创建 D3D11 设备：优先硬件加速；硬件驱动失败（远程桌面 / 虚拟机 / 驱动异常）
/// 降级 WARP 软件光栅化——糊但能启动，不再「启动即退出」。设备丢失后重建也走这里。
fn create_d3d_device() -> Result<ID3D11Device> {
    const HW_FLS: [D3D_FEATURE_LEVEL; 4] = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    const WARP_FLS: [D3D_FEATURE_LEVEL; 3] = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut d3d: Option<ID3D11Device> = None;
    let mut last_err: Option<Error> = None;
    for (driver, fls) in [
        (D3D_DRIVER_TYPE_HARDWARE, &HW_FLS[..]),
        (D3D_DRIVER_TYPE_WARP, &WARP_FLS[..]),
    ] {
        let mut dev: Option<ID3D11Device> = None;
        let r = unsafe {
            D3D11CreateDevice(
                None, // 默认适配器
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(fls),
                D3D11_SDK_VERSION,
                Some(&mut dev),
                None,
                None,
            )
        };
        match r {
            Ok(()) => {
                d3d = dev;
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    d3d.ok_or_else(|| last_err.expect("至少尝试过一次 D3D11CreateDevice，必有错误可上报"))
}

/// 该错误是否表示 D3D11 设备已丢失（继续绘制前必须换绑设备）。
///
/// 驱动重载（显卡驱动更新 / TDR / GPU 重置）后进程内设备被永久移除，此后每次
/// 绘制都返回这几个 HRESULT；它们不会自愈，只能重建（见
/// [`RenderDevice::rebind_graphics_device`]）。其余错误（参数错误等）与此无关，
/// 走原告警路径，不触发重建。
pub fn is_device_lost(err: &Error) -> bool {
    let code = err.code();
    code == DXGI_ERROR_DEVICE_REMOVED
        || code == DXGI_ERROR_DEVICE_RESET
        || code == DXGI_ERROR_DEVICE_HUNG
        || code == DXGI_ERROR_DRIVER_INTERNAL_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_creation_works_or_graceful() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        // 应用真实环境：STA COM（RenderDevice::new 内部还需 DispatcherQueue）。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
        }
        // 有硬件加速的机器应创建成功；无 GPU 环境（CI headless）允许失败。
        match RenderDevice::new() {
            Ok(d) => {
                let _ = (&d.d2d, &d.dwrite, &d.compositor, &d.gfx_device);
            }
            Err(e) => eprintln!("无 GPU 上下文，跳过（CI/远程环境预期行为）: {e:?}"),
        }
    }

    /// 设备丢失错误码分类：DXGI 设备移除族必须识别为「需重建」，
    /// 其余错误（参数错误等）不得触发重建路径。
    #[test]
    fn device_lost_codes_are_classified() {
        use windows::Win32::Foundation::E_INVALIDARG;
        use windows::Win32::Graphics::Dxgi::{
            DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
            DXGI_ERROR_DRIVER_INTERNAL_ERROR,
        };
        for code in [
            DXGI_ERROR_DEVICE_REMOVED,
            DXGI_ERROR_DEVICE_RESET,
            DXGI_ERROR_DEVICE_HUNG,
            DXGI_ERROR_DRIVER_INTERNAL_ERROR,
        ] {
            assert!(
                is_device_lost(&Error::from(code)),
                "{code:?} 应判为设备丢失"
            );
        }
        assert!(!is_device_lost(&Error::from(E_INVALIDARG)));
    }

    /// 设备丢失恢复的地基：同一 STA 线程反复换绑图形设备必须成功（合成器 /
    /// DispatcherQueue 保留，只重建 D3D11 → D2D → 合成图形设备），且换绑后
    /// 仍能创建绘制表面。无 GPU 环境（CI headless）允许跳过。
    #[test]
    fn render_device_rebinds_graphics_on_same_thread() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        // 应用真实环境：STA COM（RenderDevice::new 内部还需 DispatcherQueue）。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
        }
        let Ok(mut dev) = RenderDevice::new() else {
            eprintln!("无 GPU 上下文，跳过（CI/远程环境预期行为）");
            return;
        };
        // 健康设备问原因应返回 Ok（设备没被移除）
        assert!(dev.device_removed_reason().is_ok());
        // 连续两次换绑：恢复路径可能被重复触发（重建失败后重试）
        assert!(dev.rebind_graphics_device().is_ok(), "首次换绑失败");
        assert!(dev.rebind_graphics_device().is_ok(), "二次换绑失败");
        // 换绑后绘制表面必须还能创建（否则恢复后照样一帧都画不出）
        let surface = crate::surface::CompositionSurface::new(&dev.gfx_device, 8, 8);
        assert!(
            surface.is_ok(),
            "换绑后无法创建绘制表面: {:?}",
            surface.err()
        );
    }

    #[test]
    fn dwrite_text_format_creates_ok() {
        // 定位 CreateTextFormat E_INVALIDARG：单独验证 DWrite 工厂与文本格式创建。
        use windows::core::PCWSTR;
        use windows::Win32::Graphics::DirectWrite::{
            DWriteCreateFactory, DWRITE_FACTORY_TYPE_ISOLATED, DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
        };
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

        // 应用真实环境：STA COM；locale 必须显式（NULL 会 E_INVALIDARG）
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
        }

        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }
        let factory: windows::core::Result<IDWriteFactory> =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED) };
        let factory = factory.expect("DWrite 工厂创建失败");
        for name in ["Microsoft YaHei UI", "Segoe UI", "Arial"] {
            let family = wide(name);
            let locale = wide("zh-CN");
            let r = unsafe {
                factory.CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    16.0,
                    PCWSTR(locale.as_ptr()),
                )
            };
            match r {
                Ok(_) => eprintln!("CreateTextFormat({name}) OK"),
                Err(e) => eprintln!("CreateTextFormat({name}) FAIL: {e:?}"),
            }
        }
    }
}
