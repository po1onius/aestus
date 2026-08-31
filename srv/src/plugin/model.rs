use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

pub mod schema {
    diesel::table! {
        plugin_suites (id) {
            id -> Uuid,
            tenant_id -> Uuid,
            name -> Text,
            description -> Text,
            provider -> Text,
            enabled -> Bool,
            created_by -> Uuid,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
        }
    }

    diesel::table! {
        plugin_suite_releases (id) {
            id -> Uuid,
            suite_id -> Uuid,
            version -> Int8,
            manifest_sha256 -> Text,
            created_by -> Uuid,
            published_at -> Timestamptz,
        }
    }

    diesel::table! {
        plugin_suite_artifacts (id) {
            id -> Uuid,
            release_id -> Uuid,
            slot -> Text,
            abi_version -> Int4,
            wasm_sha256 -> Text,
            wasm_size -> Int8,
            wasm_bytes -> Bytea,
        }
    }

    diesel::allow_tables_to_appear_in_same_query!(
        plugin_suites,
        plugin_suite_releases,
        plugin_suite_artifacts,
    );
}

pub const SLOT_REQUEST: &str = "request";
pub const SLOT_BUFFERED_RESPONSE: &str = "buffered_response";
pub const SLOT_STREAM_RESPONSE: &str = "stream_response";

/// 套件内三个固定执行位置。数据库和日志只使用这里定义的稳定字符串，避免各层分别维护
/// 拼写；新增插槽时也必须同时扩展发布校验和运行时 ABI 分派。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSlot {
    Request,
    BufferedResponse,
    StreamResponse,
}

impl PluginSlot {
    pub const ALL: [Self; 3] = [Self::Request, Self::BufferedResponse, Self::StreamResponse];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => SLOT_REQUEST,
            Self::BufferedResponse => SLOT_BUFFERED_RESPONSE,
            Self::StreamResponse => SLOT_STREAM_RESPONSE,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            SLOT_REQUEST => Some(Self::Request),
            SLOT_BUFFERED_RESPONSE => Some(Self::BufferedResponse),
            SLOT_STREAM_RESPONSE => Some(Self::StreamResponse),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Queryable)]
pub struct PluginSuite {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub enabled: bool,
}

#[derive(Insertable)]
#[diesel(table_name = schema::plugin_suites)]
pub struct NewPluginSuite {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub created_by: Uuid,
}

#[derive(Insertable)]
#[diesel(table_name = schema::plugin_suite_releases)]
pub struct NewPluginSuiteRelease {
    pub suite_id: Uuid,
    pub version: i64,
    pub manifest_sha256: String,
    pub created_by: Uuid,
}

#[derive(Insertable)]
#[diesel(table_name = schema::plugin_suite_artifacts)]
pub struct NewPluginSuiteArtifact {
    pub release_id: Uuid,
    pub slot: String,
    pub abi_version: i32,
    pub wasm_sha256: String,
    pub wasm_size: i64,
    pub wasm_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginArtifactSummary {
    pub id: Uuid,
    pub slot: PluginSlot,
    pub abi_version: i32,
    pub wasm_sha256: String,
    pub wasm_size: usize,
}

/// Dashboard 和 API Key 绑定展示使用完整套件 release 快照，但列表绝不读取 BYTEA。
#[derive(Debug, Clone, Serialize)]
pub struct PluginReleaseSummary {
    pub id: Uuid,
    pub suite_id: Uuid,
    pub tenant_id: Uuid,
    pub suite_name: String,
    pub description: String,
    pub provider: String,
    pub suite_enabled: bool,
    pub version: i64,
    pub manifest_sha256: String,
    pub artifacts: Vec<PluginArtifactSummary>,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginArtifactBinding {
    pub id: Uuid,
    pub slot: PluginSlot,
    pub abi_version: i32,
    pub wasm_sha256: String,
}

/// 请求热路径携带的不可变套件选择。三个 artifact 独立缓存和执行；缺失的插槽由调用方
/// 明确回落到 provider 原生实现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginBinding {
    pub release_id: Uuid,
    pub suite_id: Uuid,
    pub tenant_id: Uuid,
    pub suite_name: String,
    pub provider: String,
    pub version: i64,
    pub manifest_sha256: String,
    pub artifacts: Vec<PluginArtifactBinding>,
}

impl PluginBinding {
    pub fn artifact(&self, slot: PluginSlot) -> Option<&PluginArtifactBinding> {
        self.artifacts.iter().find(|artifact| artifact.slot == slot)
    }
}

pub struct PluginArtifact {
    pub binding: PluginArtifactBinding,
    pub suite_binding: PluginBinding,
    pub wasm_bytes: Vec<u8>,
}

/// HTTP 上传在进入数据库事务前完成全部 artifact 的编译和摘要计算。
pub struct PluginArtifactUpload {
    pub slot: PluginSlot,
    pub wasm_sha256: String,
    pub wasm_bytes: Vec<u8>,
}
