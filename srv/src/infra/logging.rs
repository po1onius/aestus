use std::{env, io, path::Path};

use logroller::{LogRollerBuilder, Rotation, RotationSize};
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{
    EnvFilter, fmt::writer::MakeWriterExt, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::err::{AppError, AppResult};

const DEFAULT_LOG_DIRECTORY: &str = "logs";
const DEFAULT_LOG_FILE_NAME: &str = "aestus-gateway.log";
const LOG_ROTATION_SIZE_MIB: u64 = 2;
const LOG_QUEUE_CAPACITY_LINES: usize = 128_000;

/// 持有全部非阻塞日志 worker 的生命周期守卫。
///
/// `tracing-appender` 的 writer 只负责把日志投递到有界队列，实际 stdout 与文件 I/O
/// 由独立线程完成。守卫必须一直存活到 `main` 返回，Drop 时才会冲刷尚未落盘的日志。
pub struct LoggingGuards {
    _stdout_guard: WorkerGuard,
    _file_guard: WorkerGuard,
}

/// 初始化 stdout 与大小滚动文件两路 JSON 日志。
///
/// 两个 writer 都使用 lossy 非阻塞队列：极端日志洪峰下允许丢弃日志，绝不让日志磁盘或
/// stdout 背压阻塞模型请求。文件达到 2 MiB 前会先滚动为编号归档，再写入新的当前文件；
/// 未配置删除上限，因此本模块不会擅自清理历史日志。
pub fn init() -> AppResult<LoggingGuards> {
    let log_directory = env_or_default("AESTUS_LOG_DIRECTORY", DEFAULT_LOG_DIRECTORY)?;
    let log_file_name = env_or_default("AESTUS_LOG_FILE_NAME", DEFAULT_LOG_FILE_NAME)?;
    let log_path = Path::new(&log_directory).join(&log_file_name);

    let file_appender = LogRollerBuilder::new(&log_directory, &log_file_name)
        .rotation(Rotation::SizeBased(RotationSize::MB(LOG_ROTATION_SIZE_MIB)))
        .build()
        .map_err(|source| AppError::Startup {
            message: format!("初始化滚动日志文件 {} 失败: {source}", log_path.display()),
        })?;

    let (stdout_writer, stdout_guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(LOG_QUEUE_CAPACITY_LINES)
        .lossy(true)
        .thread_name("tracing-stdout-writer")
        .finish(io::stdout());
    let (file_writer, file_guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(LOG_QUEUE_CAPACITY_LINES)
        .lossy(true)
        .thread_name("tracing-file-writer")
        .finish(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("aestus=info,tower_http=info,axum=info"));
    // `and` 只格式化一次事件，再将同一行分别投递给两个非阻塞 writer。
    let writer = stdout_writer.and(file_writer);
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().json().with_writer(writer))
        .try_init()
        .map_err(|source| AppError::Startup {
            message: format!("初始化 tracing subscriber 失败: {source}"),
        })?;

    tracing::info!(
        log_file = %log_path.display(),
        rotation_size_mib = LOG_ROTATION_SIZE_MIB,
        queue_capacity_lines = LOG_QUEUE_CAPACITY_LINES,
        lossy = true,
        "stdout 与大小滚动文件非阻塞日志已初始化"
    );

    Ok(LoggingGuards {
        _stdout_guard: stdout_guard,
        _file_guard: file_guard,
    })
}

fn env_or_default(key: &'static str, default: &'static str) -> AppResult<String> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        Ok(_) => Err(AppError::MissingConfig { key }),
        Err(env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(source) => Err(AppError::ReadConfig { key, source }),
    }
}
