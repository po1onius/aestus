use std::{
    error::Error as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{body::Body, extract::Request};
use bytes::Bytes;
use tempfile::NamedTempFile;
use tokio::{fs, io::AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tracing::info;
use uuid::Uuid;

use crate::err::{AppError, AppResult};

pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
enum CachedBodyStorage {
    Memory(Bytes),
    File(Arc<PathBuf>),
}

/// 可重放请求体缓存。
///
/// 小请求保留在内存里，超过 `body_memory_limit_bytes` 的请求写入临时文件。
/// 后续上游重试从这份不可变缓存生成新的 reqwest body 或请求字节。
#[derive(Debug, Clone)]
pub struct CachedBody {
    request_id: Uuid,
    len: usize,
    storage: CachedBodyStorage,
}

impl CachedBody {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn request_id(&self) -> Uuid {
        self.request_id
    }

    pub fn storage_kind(&self) -> &'static str {
        match &self.storage {
            CachedBodyStorage::Memory(_) => "memory",
            CachedBodyStorage::File(_) => "file",
        }
    }

    pub async fn replay_body(&self) -> AppResult<reqwest::Body> {
        match &self.storage {
            CachedBodyStorage::Memory(bytes) => Ok(reqwest::Body::from(bytes.clone())),
            CachedBodyStorage::File(path) => {
                let file =
                    fs::File::open(path.as_path())
                        .await
                        .map_err(|source| AppError::BodyCache {
                            message: source.to_string(),
                        })?;
                let stream = ReaderStream::new(file);
                Ok(reqwest::Body::wrap_stream(stream))
            }
        }
    }

    /// 为一次上游资源级 override 读取原始请求字节。
    ///
    /// 每次重试都从不可变缓存读取，避免上一资源对 body 的修改泄漏给下一资源。大请求
    /// 保持存盘，只有当前 attempt 应用 JSON Merge Patch 时才读回内存。
    pub async fn replay_bytes(&self) -> AppResult<Bytes> {
        match &self.storage {
            CachedBodyStorage::Memory(bytes) => Ok(bytes.clone()),
            CachedBodyStorage::File(path) => fs::read(path.as_path())
                .await
                .map(Bytes::from)
                .map_err(|source| AppError::BodyCache {
                    message: source.to_string(),
                }),
        }
    }
}

impl Drop for CachedBody {
    fn drop(&mut self) {
        if let CachedBodyStorage::File(path) = &self.storage
            && Arc::strong_count(path) == 1
        {
            let request_id = self.request_id;
            let path = path.as_ref().clone();
            tokio::spawn(async move {
                if let Err(error) = fs::remove_file(path.as_path()).await {
                    tracing::warn!(
                        request_id = %request_id,
                        path = %path.display(),
                        error = %error,
                        "清理请求体临时文件失败"
                    );
                }
            });
        }
    }
}

/// 读取并缓存调用方提交的原始请求体。
///
/// 返回值中的 `Bytes` 只用于 provider 一次性解析 model、粘性字段和请求日志字段；
/// `CachedBody` 则在后续每个 attempt 中重放不可变原始请求。资源级 override 只能在调度
/// 完成后应用，不会修改这里的原始内容。
pub async fn cache_request_body(
    request: Request<Body>,
    request_id: Uuid,
    memory_limit_bytes: usize,
) -> AppResult<(CachedBody, Bytes)> {
    let body_bytes = axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|source| {
            if source
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>())
            {
                return AppError::PayloadTooLarge {
                    limit_bytes: MAX_BODY_BYTES,
                };
            }

            if request_body_was_interrupted(&source) {
                return AppError::RequestBodyInterrupted {
                    message: source.to_string(),
                };
            }

            AppError::BodyCache {
                message: source.to_string(),
            }
        })?;

    if body_bytes.len() <= memory_limit_bytes {
        info!(
            request_id = %request_id,
            body_bytes = body_bytes.len(),
            memory_limit_bytes,
            "请求体已缓存到内存"
        );

        return Ok((
            CachedBody {
                request_id,
                len: body_bytes.len(),
                storage: CachedBodyStorage::Memory(body_bytes.clone()),
            },
            body_bytes,
        ));
    }

    let path = write_temp_body_file(&body_bytes).await?;

    info!(
        request_id = %request_id,
        body_bytes = body_bytes.len(),
        memory_limit_bytes,
        path = %path.display(),
        "请求体超过内存阈值，已缓存到临时文件"
    );

    Ok((
        CachedBody {
            request_id,
            len: body_bytes.len(),
            storage: CachedBodyStorage::File(Arc::new(path)),
        },
        body_bytes,
    ))
}

/// 判断 `to_bytes` 失败是否来自调用方请求体网络流提前终止。
///
/// Axum 会用自己的错误类型逐层包装 Hyper 的传输错误，因此必须遍历完整 source chain
/// 并使用 Hyper 提供的稳定分类方法。这里不匹配错误文本，避免依赖不同协议或版本下会
/// 变化的展示字符串；临时文件创建、写入和重放错误也不会进入本函数，仍归类为缓存故障。
fn request_body_was_interrupted(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(source) = current {
        if source.downcast_ref::<hyper::Error>().is_some_and(|error| {
            error.is_incomplete_message() || error.is_closed() || error.is_canceled()
        }) {
            return true;
        }

        if source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::NotConnected
                )
            })
        {
            return true;
        }

        current = source.source();
    }

    false
}

async fn write_temp_body_file(bytes: &Bytes) -> AppResult<PathBuf> {
    let named_temp_file = NamedTempFile::new().map_err(|source| AppError::BodyCache {
        message: source.to_string(),
    })?;
    let (_file, path) = named_temp_file
        .keep()
        .map_err(|source| AppError::BodyCache {
            message: source.error.to_string(),
        })?;

    write_all(&path, bytes).await?;
    Ok(path)
}

async fn write_all(path: &Path, bytes: &Bytes) -> AppResult<()> {
    let mut file = fs::File::create(path)
        .await
        .map_err(|source| AppError::BodyCache {
            message: source.to_string(),
        })?;

    file.write_all(bytes)
        .await
        .map_err(|source| AppError::BodyCache {
            message: source.to_string(),
        })?;

    file.flush().await.map_err(|source| AppError::BodyCache {
        message: source.to_string(),
    })
}
