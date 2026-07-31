use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use tracing::info;
use uuid::Uuid;

use crate::err::{AppError, AppResult};

const MAX_OVERRIDE_HEADER_ENTRIES: usize = 64;
const MAX_OVERRIDE_HEADER_VALUES_PER_NAME: usize = 16;
const MAX_OVERRIDE_HEADER_VALUE_BYTES: usize = 16 * 1024;
const MAX_OVERRIDE_BODY_TOP_LEVEL_KEYS: usize = 128;
const MAX_OVERRIDE_BODY_BYTES: usize = 128 * 1024;
const MAX_OVERRIDE_KEY_BYTES: usize = 256;

/// 单个上游资源拥有的请求覆盖配置。
///
/// `body` 使用 JSON Merge Patch 语义：普通值替换，object 递归合并，`null` 删除字段。
/// `header` 的值允许 string、string array 或 null；分别表示替换、设置多值或删除 header。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestOverride {
    #[serde(default)]
    pub header: Map<String, Value>,
    #[serde(default)]
    pub body: Map<String, Value>,
}

impl RequestOverride {
    pub fn from_value(value: Value) -> AppResult<Self> {
        let parsed =
            serde_json::from_value::<Self>(value).map_err(|source| AppError::BadRequest {
                message: format!(
                    "override 必须且只能包含 JSON object 类型的 header 和 body: {source}"
                ),
            })?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("RequestOverride 固定可序列化")
    }

    pub fn validate(&self) -> AppResult<()> {
        if self.header.len() > MAX_OVERRIDE_HEADER_ENTRIES {
            return Err(AppError::BadRequest {
                message: format!(
                    "override.header 最多包含 {MAX_OVERRIDE_HEADER_ENTRIES} 个 header"
                ),
            });
        }
        for (name, value) in &self.header {
            if name.len() > MAX_OVERRIDE_KEY_BYTES {
                return Err(AppError::BadRequest {
                    message: format!(
                        "override.header 名称长度不能超过 {MAX_OVERRIDE_KEY_BYTES} 字节"
                    ),
                });
            }
            HeaderName::from_bytes(name.as_bytes()).map_err(|source| AppError::BadRequest {
                message: format!("override.header 包含非法 header 名称 {name:?}: {source}"),
            })?;

            match value {
                Value::Null => {}
                Value::String(value) => validate_header_value(name, value)?,
                Value::Array(values)
                    if values.len() <= MAX_OVERRIDE_HEADER_VALUES_PER_NAME
                        && values.iter().all(Value::is_string) =>
                {
                    for value in values {
                        validate_header_value(
                            name,
                            value
                                .as_str()
                                .expect("分支条件已保证 header array 元素均为 string"),
                        )?;
                    }
                }
                _ => {
                    return Err(AppError::BadRequest {
                        message: format!(
                            "override.header.{name} 必须是 string、最多 {MAX_OVERRIDE_HEADER_VALUES_PER_NAME} 项的 string array 或 null"
                        ),
                    });
                }
            }
        }

        if self.body.len() > MAX_OVERRIDE_BODY_TOP_LEVEL_KEYS {
            return Err(AppError::BadRequest {
                message: format!(
                    "override.body 顶层最多包含 {MAX_OVERRIDE_BODY_TOP_LEVEL_KEYS} 个字段"
                ),
            });
        }
        if let Some(key) = self
            .body
            .keys()
            .find(|key| key.len() > MAX_OVERRIDE_KEY_BYTES)
        {
            return Err(AppError::BadRequest {
                message: format!(
                    "override.body 字段名称过长: {key:?}，最多 {MAX_OVERRIDE_KEY_BYTES} 字节"
                ),
            });
        }
        let body_bytes = serde_json::to_vec(&self.body)
            .map_err(|source| AppError::BadRequest {
                message: format!("override.body 无法序列化: {source}"),
            })?
            .len();
        if body_bytes > MAX_OVERRIDE_BODY_BYTES {
            return Err(AppError::BadRequest {
                message: format!("override.body 序列化后不能超过 {MAX_OVERRIDE_BODY_BYTES} 字节"),
            });
        }

        Ok(())
    }

    /// 在 provider 构造基础请求后应用覆盖。
    ///
    /// 认证信息尚未注入，因此即使管理员配置了 Authorization，provider 随后的单一请求
    /// 最终化 hook 也会覆盖它。`body=None` 表示仍可从缓存流式重放；只有存在 body
    /// override 时才要求调用方提前物化完整字节。Content-Length 始终删除并交给 reqwest
    /// 按最终 body 重新计算。
    pub fn apply(
        &self,
        request_id: Uuid,
        resource: &UpstreamResource,
        headers: &mut HeaderMap,
        body: Option<Bytes>,
    ) -> AppResult<Option<Bytes>> {
        // override 的完整结构与容量限制已在配置写入时统一校验。请求热路径这里只完成
        // 实际转换；转换失败仍返回受控错误，避免异常内存数据触发进程 panic。
        let header_names = self.header.keys().cloned().collect::<Vec<_>>();
        for (raw_name, value) in &self.header {
            let name = HeaderName::from_bytes(raw_name.as_bytes()).map_err(|source| {
                AppError::BadRequest {
                    message: format!("override.header 包含非法 header 名称 {raw_name:?}: {source}"),
                }
            })?;
            headers.remove(&name);

            match value {
                Value::Null => {}
                Value::String(value) => {
                    headers.insert(name, parse_header_value(raw_name, value)?);
                }
                Value::Array(values) => {
                    for value in values {
                        let value = value.as_str().ok_or_else(|| AppError::BadRequest {
                            message: format!(
                                "override.header.{raw_name} 的 array 元素必须是 string"
                            ),
                        })?;
                        headers.append(name.clone(), parse_header_value(raw_name, value)?);
                    }
                }
                _ => {
                    return Err(AppError::BadRequest {
                        message: format!(
                            "override.header.{raw_name} 必须是 string、string array 或 null"
                        ),
                    });
                }
            }
        }
        headers.remove(header::CONTENT_LENGTH);

        if self.body.is_empty() {
            if !header_names.is_empty() {
                info!(
                    request_id = %request_id,
                    provider = %resource.provider,
                    resource_type = resource.kind.as_str(),
                    resource_id = %resource.id,
                    override_header_names = ?header_names,
                    "已静默应用上游资源请求 header override"
                );
            }
            return Ok(body);
        }

        let original_body = body.as_ref().ok_or_else(|| AppError::BadRequest {
            message: "请求体尚未物化，无法应用资源级 body override".to_owned(),
        })?;
        let mut body_value = serde_json::from_slice::<Value>(original_body).map_err(|source| {
            AppError::BadRequest {
                message: format!("请求体不是合法 JSON，无法应用资源级 body override: {source}"),
            }
        })?;
        let original_model = body_value.get("model").cloned();
        json_patch::merge(&mut body_value, &Value::Object(self.body.clone()));
        let upstream_model = body_value.get("model").cloned();
        let body_keys = self.body.keys().cloned().collect::<Vec<_>>();
        let body = serde_json::to_vec(&body_value)
            .map(Bytes::from)
            .map_err(|source| AppError::BadRequest {
                message: format!("应用资源级 body override 后无法序列化请求体: {source}"),
            })?;

        info!(
            request_id = %request_id,
            provider = %resource.provider,
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            override_header_names = ?header_names,
            override_body_keys = ?body_keys,
            requested_model = original_model.as_ref().map(json_log_value).unwrap_or("<missing>"),
            upstream_model = upstream_model.as_ref().map(json_log_value).unwrap_or("<missing>"),
            model_replaced = original_model != upstream_model,
            "已静默应用上游资源请求 override"
        );

        Ok(Some(body))
    }
}

fn parse_header_value(name: &str, value: &str) -> AppResult<HeaderValue> {
    HeaderValue::from_str(value).map_err(|source| AppError::BadRequest {
        message: format!("override.header.{name} 包含非法 header value: {source}"),
    })
}

fn validate_header_value(name: &str, value: &str) -> AppResult<()> {
    if value.len() > MAX_OVERRIDE_HEADER_VALUE_BYTES {
        return Err(AppError::BadRequest {
            message: format!(
                "override.header.{name} 的单个值不能超过 {MAX_OVERRIDE_HEADER_VALUE_BYTES} 字节"
            ),
        });
    }
    parse_header_value(name, value).map(|_| ())
}

fn json_log_value(value: &Value) -> &str {
    value.as_str().unwrap_or("<non-string>")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamResourceKind {
    Account,
    ApiKey,
}

impl UpstreamResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::ApiKey => "api_key",
        }
    }
}

/// Redis 中可直接参与调度并构造上游请求的最小资源投影。
///
/// 该结构只保存请求热路径需要的认证材料、API Key Base URL、请求覆盖参数与 provider
/// 请求上下文，不复制 PostgreSQL 持久模型中的 refresh token、维护时间、状态和管理展示
/// 字段。`revision` 仅是 Redis 投影顺序栅栏；账号认证回执使用独立
/// `credential_generation` 判断请求属于哪一版 token，避免管理员 enabled/override 等
/// 无关更新误伤已经完成的 token 轮换。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamResource {
    pub id: Uuid,
    pub provider: String,
    /// 分组是调度强边界。runtime 只保存稳定 UUID；展示名称始终来自 PostgreSQL。
    pub group_id: Uuid,
    pub kind: UpstreamResourceKind,
    pub auth_secret: String,
    /// 官方 API Key 导入时确定的通用上游地址；账号使用 provider 全局地址，因此为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub request_context: Value,
    #[serde(rename = "override")]
    pub request_override: RequestOverride,
    /// 账号 token 世代；官方 API Key 导入后不可修改，因此该字段为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_generation: Option<i64>,
    /// PostgreSQL `updated_at` 派生的 Redis 投影版本，不参与 token/health 业务判断。
    pub revision: i64,
}

impl UpstreamResource {
    pub fn resource_member(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }

    /// API Key runtime 的 `base_url` 由唯一构造入口写入。这里仅恢复扁平资源结构中的
    /// kind-specific 字段，不重新执行导入时的 URL 语义校验。
    pub fn api_key_base_url(&self) -> AppResult<&str> {
        self.base_url
            .as_deref()
            .ok_or_else(|| AppError::BadRequest {
                message: format!(
                    "API Key runtime 缺少 base_url: provider={}, resource_id={}",
                    self.provider, self.id
                ),
            })
    }

    pub fn parse_request_context<T: DeserializeOwned>(&self) -> AppResult<T> {
        serde_json::from_value(self.request_context.clone()).map_err(|source| {
            AppError::BadRequest {
                message: format!(
                    "provider 请求上下文无法解析: provider={}, resource_type={}, resource_id={}, error={source}",
                    self.provider,
                    self.kind.as_str(),
                    self.id
                ),
            }
        })
    }
}

/// 将 provider 定义的请求上下文序列化为 Redis 投影使用的 JSON object。
///
/// 持久模型中的 `specific` 可以包含管理与维护字段；provider 必须显式构造独立上下文，
/// 由这里统一保证 Redis 热路径不会接收非 object 形状的数据。
pub fn serialize_request_context<T: Serialize>(context: &T) -> AppResult<Value> {
    let value = serde_json::to_value(context).map_err(|source| AppError::BadRequest {
        message: format!("provider 请求上下文无法序列化: {source}"),
    })?;
    if !value.is_object() {
        return Err(AppError::BadRequest {
            message: "provider 请求上下文必须序列化为 JSON object".to_owned(),
        });
    }
    Ok(value)
}
