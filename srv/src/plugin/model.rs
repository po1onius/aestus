use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;
use wasmtime::component::Component;

pub mod schema {
    diesel::table! {
        plugins (id) {
            id -> Uuid,
            tenant_id -> Nullable<Text>,
            provider -> Text,
            slot -> Text,
            name -> Text,
            description -> Text,
            wasm_sha256 -> Text,
            wasm_size -> Int8,
            wasm_bytes -> Bytea,
            created_by -> Uuid,
            created_at -> Timestamptz,
        }
    }
    diesel::table! {
        plugin_suites (id) {
            id -> Uuid,
            tenant_id -> Nullable<Text>,
            name -> Text,
            description -> Text,
            provider -> Text,
            enabled -> Bool,
            request_plugin_id -> Nullable<Uuid>,
            buffered_response_plugin_id -> Nullable<Uuid>,
            stream_response_plugin_id -> Nullable<Uuid>,
            created_by -> Uuid,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
        }
    }
    diesel::allow_tables_to_appear_in_same_query!(plugins, plugin_suites);
}

pub const SLOT_REQUEST: &str = "request";
pub const SLOT_BUFFERED_RESPONSE: &str = "buffered_response";
pub const SLOT_STREAM_RESPONSE: &str = "stream_response";

/// 套件内三个固定执行位置。数据库和日志只使用这里定义的稳定字符串，避免各层分别维护
/// 拼写；新增插槽时也必须同时扩展套件校验和运行时 ABI 分派。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSlot {
    Request,
    BufferedResponse,
    StreamResponse,
}

impl PluginSlot {
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

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::plugins)]
pub struct PluginSummary {
    pub id: Uuid,
    pub tenant_id: Option<String>,
    pub provider: String,
    pub slot: String,
    pub name: String,
    pub description: String,
    pub wasm_sha256: String,
    pub wasm_size: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = schema::plugins)]
pub struct NewPlugin {
    pub tenant_id: Option<String>,
    pub provider: String,
    pub slot: String,
    pub name: String,
    pub description: String,
    pub wasm_sha256: String,
    pub wasm_size: i64,
    pub wasm_bytes: Vec<u8>,
    pub created_by: Uuid,
}

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::plugin_suites)]
pub struct PluginSuiteSummary {
    pub id: Uuid,
    pub tenant_id: Option<String>,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub enabled: bool,
    pub request_plugin_id: Option<Uuid>,
    pub buffered_response_plugin_id: Option<Uuid>,
    pub stream_response_plugin_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl PluginSuiteSummary {
    pub fn slots(&self) -> [(PluginSlot, Option<Uuid>); 3] {
        [
            (PluginSlot::Request, self.request_plugin_id),
            (
                PluginSlot::BufferedResponse,
                self.buffered_response_plugin_id,
            ),
            (PluginSlot::StreamResponse, self.stream_response_plugin_id),
        ]
    }
}

#[derive(Insertable)]
#[diesel(table_name = schema::plugin_suites)]
pub struct NewPluginSuite {
    pub tenant_id: Option<String>,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub request_plugin_id: Option<Uuid>,
    pub buffered_response_plugin_id: Option<Uuid>,
    pub stream_response_plugin_id: Option<Uuid>,
    pub created_by: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginArtifactBinding {
    pub id: Uuid,
    pub slot: PluginSlot,
    pub wasm_sha256: String,
}

/// 单个请求持有固定组合和全部组件引用；响应处理和重试不会再查询数据库或公共缓存。
#[derive(Clone)]
pub struct PluginBinding {
    pub suite_id: Uuid,
    pub tenant_id: String,
    pub provider: String,
    pub artifacts: Vec<PluginArtifactBinding>,
    pub components: HashMap<Uuid, Arc<Component>>,
}

impl PluginBinding {
    pub fn artifact(&self, slot: PluginSlot) -> Option<&PluginArtifactBinding> {
        self.artifacts.iter().find(|artifact| artifact.slot == slot)
    }
}
