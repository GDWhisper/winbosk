//! 日志初始化：滚动文件 + 控制台，`RUST_LOG` 环境变量可覆盖。

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 初始化日志，返回保活句柄（文件 appender 的 non-blocking worker）。
///
/// 调用方必须在 main 中持有返回的 guard，否则日志会被截断。
pub fn init(log_dir: &Path) -> Result<WorkerGuard, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "winbosk.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // 顺序有意义：`Layered` 逐层**自内向外**派发（见 tracing-subscriber `layered.rs`），
    // 先 `.with` 者先执行。非阻塞文件层放前面，保证「日志已进文件队列」先于 stdout 的
    // 同步写——控制台被冻结（选中文本）时 stdout 写会阻塞，看门狗在停摆时靠的就是
    // 文件层先落盘，不能被它挡住。
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stdout))
        .init();

    Ok(guard)
}
