//! GPU 设备丢失恢复。
//!
//! 驱动重载（显卡驱动更新 / TDR / GPU 重置）会移除进程内的 D3D11 设备，
//! `DXGI_ERROR_DEVICE_REMOVED` 不会自愈。不恢复的后果是进程「活着但画不出
//! 任何东西」：热键与托盘照响、事件回路照跑，画面全停——控制中心永远呼不
//! 出来，用户看到的就是「程序运行异常」（2026-10-10 实测：nvlddmkm 重载后
//! 进程持续 16 小时画不出，重启才好）。
//!
//! 恢复路径：识别设备丢失（`winbosk_render::is_device_lost`）→ 换绑图形设备
//! 并重建绘制表面（`Compositor::recover_from_device_loss`）→ 全量重新提取图标
//! 位图并上传（App 层持有 `DesktopItem`，渲染层不持有）→ 用同一场景立刻重绘
//! 一帧。

use std::time::{Duration, Instant};

use winbosk_shell::icons::IconData;

use winbosk_render::Scene;

use crate::{Runtime, ICON_EXTRACT_SIZE};

/// 重建失败后的重试间隔。设备丢失时补间动画仍在请求重绘，不限频会变成每帧
/// 一次全量重建尝试（含 `D3D11CreateDevice` + 图标重提取），既刷日志又拖慢
/// 主线程。
const RETRY_MIN_INTERVAL: Duration = Duration::from_secs(2);

/// 尝试从 GPU 设备丢失中恢复。成功返回 `true`（场景已用新设备重绘一帧）。
///
/// `scene` 是刚绘制失败的同一场景：重建后直接用它重绘（失败前后运行时状态
/// 没有变化，不必再走一遍 `build_scene`）。失败时按 [`RETRY_MIN_INTERVAL`]
/// 限频，由后续重绘请求自然重试。
pub(crate) fn try_recover(rt: &mut Runtime, scene: &Scene) -> bool {
    if let Some(last) = rt.device_recover_at {
        if last.elapsed() < RETRY_MIN_INTERVAL {
            return false;
        }
    }
    // 先问旧设备为什么死的（TDR / 驱动更新 / 硬件），换绑后旧对象即被替换
    let reason = rt
        .compositor
        .device_removed_reason()
        .err()
        .map(|e| e.code());
    tracing::error!(?reason, "GPU 设备已丢失，重建渲染设备");
    if let Err(e) = rt.compositor.recover_from_device_loss() {
        tracing::warn!("渲染设备重建失败，稍后重试: {e}");
        rt.device_recover_at = Some(Instant::now());
        return false;
    }
    // 图标位图随旧设备失效：全量重新提取并排队上传（与启动时同一段逻辑，
    // 见 `main.rs` 首帧上传）。提取是同步 Shell IO；设备丢失是低频事件，
    // 一次性停顿可接受，换来的是「不重启进程就恢复」。
    rt.bitmap_ids.clear();
    rt.pending_uploads.clear();
    let mut reuploaded = 0usize;
    for (i, item) in rt.items.iter().enumerate() {
        match winbosk_shell::icons::extract_icon(item, ICON_EXTRACT_SIZE) {
            Ok(data) => {
                rt.bitmap_ids.insert(item.id.clone(), i as u64);
                rt.pending_uploads.push((i as u64, data));
                reuploaded += 1;
            }
            Err(e) => tracing::warn!(name = %item.display_name, "设备重建后图标提取失败: {e}"),
        }
    }
    let ups = std::mem::take(&mut rt.pending_uploads);
    let upload_refs: Vec<(u64, &IconData)> = ups.iter().map(|(id, d)| (*id, d)).collect();
    if let Err(e) = rt.compositor.present(scene, &upload_refs) {
        tracing::warn!("设备重建后重绘失败: {e}");
        rt.device_recover_at = Some(Instant::now());
        return false;
    }
    // 恢复成功即清零：设备可能再次丢失，下一次要能立刻重试
    rt.device_recover_at = None;
    tracing::info!(reuploaded, "GPU 设备已重建，界面恢复绘制");
    true
}
