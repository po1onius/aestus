use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const CONTEXT_VERSION: u8 = 1;

/// 请求插件记录的 namespace 子工具原始身份。key 是实际发给上游的摊平工具名；value
/// 不能只靠 `__` 反推，因为超长名称会截断加哈希，普通工具名也可能原生包含双下划线。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceToolName {
    pub namespace: String,
    pub name: String,
}

/// 套件内部的版本化透明 context。宿主只限制字节大小并按 attempt 转交，不理解此结构。
/// 使用 BTreeMap 保证相同请求产生稳定字节，便于测试和问题复现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseContext {
    version: u8,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    namespace_names: BTreeMap<String, NamespaceToolName>,
}

pub fn encode_namespace_context(
    namespace_names: BTreeMap<String, NamespaceToolName>,
) -> Result<Option<Vec<u8>>, String> {
    if namespace_names.is_empty() {
        return Ok(None);
    }
    validate_namespace_names(&namespace_names)?;
    serde_json::to_vec(&ResponseContext {
        version: CONTEXT_VERSION,
        namespace_names,
    })
    .map(Some)
    .map_err(|error| format!("插件 response context 无法序列化: {error}"))
}

pub fn decode_response_context(bytes: Option<&[u8]>) -> Result<Option<ResponseContext>, String> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let context = serde_json::from_slice::<ResponseContext>(bytes)
        .map_err(|error| format!("插件 request context 不是合法 JSON: {error}"))?;
    if context.version != CONTEXT_VERSION {
        return Err(format!(
            "插件 request context 版本不受支持: expected={CONTEXT_VERSION}, actual={}",
            context.version
        ));
    }
    validate_namespace_names(&context.namespace_names)?;
    Ok(Some(context))
}

/// 只恢复 Responses `function_call`，普通 message 中恰好相同的 name 必须保持不变。
/// 递归覆盖 output、response.output、item 等所有标准生命周期位置，并对未来新增 wrapper
/// 保持兼容。
pub fn restore_namespace_calls(value: &mut Value, context: Option<&ResponseContext>) -> bool {
    let Some(context) = context else {
        return false;
    };
    restore_value(value, &context.namespace_names)
}

fn restore_value(value: &mut Value, names: &BTreeMap<String, NamespaceToolName>) -> bool {
    match value {
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= restore_value(value, names);
            }
            changed
        }
        Value::Object(object) => {
            let mut changed = false;
            if object.get("type").and_then(Value::as_str) == Some("function_call")
                && let Some(flattened) = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .map(str::to_owned)
                && let Some(original) = names.get(&flattened)
            {
                object.insert("name".to_owned(), Value::String(original.name.clone()));
                object.insert(
                    "namespace".to_owned(),
                    Value::String(original.namespace.clone()),
                );
                changed = true;
            }
            for child in object.values_mut() {
                changed |= restore_value(child, names);
            }
            changed
        }
        _ => false,
    }
}

fn validate_namespace_names(names: &BTreeMap<String, NamespaceToolName>) -> Result<(), String> {
    for (flattened, original) in names {
        if flattened.trim().is_empty()
            || original.namespace.trim().is_empty()
            || original.name.trim().is_empty()
        {
            return Err("插件 request context 包含空 namespace 工具名称".to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn context_round_trip_restores_only_function_calls() {
        let mappings = BTreeMap::from([(
            "collaboration__spawn_agent".to_owned(),
            NamespaceToolName {
                namespace: "collaboration".to_owned(),
                name: "spawn_agent".to_owned(),
            },
        )]);
        let bytes = encode_namespace_context(mappings).unwrap().unwrap();
        let context = decode_response_context(Some(&bytes)).unwrap().unwrap();
        let mut value = json!({
            "type": "response.completed",
            "response": {
                "output": [
                    {"type":"function_call","name":"collaboration__spawn_agent"},
                    {"type":"message","name":"collaboration__spawn_agent"}
                ]
            }
        });

        assert!(restore_namespace_calls(&mut value, Some(&context)));
        assert_eq!(value["response"]["output"][0]["name"], "spawn_agent");
        assert_eq!(value["response"]["output"][0]["namespace"], "collaboration");
        assert_eq!(
            value["response"]["output"][1]["name"],
            "collaboration__spawn_agent"
        );
    }

    #[test]
    fn empty_mapping_uses_none_instead_of_empty_context() {
        assert_eq!(encode_namespace_context(BTreeMap::new()).unwrap(), None);
        assert_eq!(decode_response_context(None).unwrap(), None);
    }
}
