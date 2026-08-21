use serde_json::Value;

/// 判断完整 body 是否包含真正的 SSE 行级 framing。只匹配物理行开头，避免普通 JSON
/// 字符串中的 `data:`/`event:` 文本被误判成事件流。
pub fn body_has_sse_framing(body: &[u8]) -> bool {
    body.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.starts_with(b"data:") || line.starts_with(b"event:")
    })
}

/// 遍历完整 SSE body 中的 JSON data payload。
///
/// 标准多行 `data:` 按换行拼接；同时兼容缺失空行、连续多条 data 各自都是 JSON 的
/// 上游形态，此时逐行解析。空 data、`[DONE]` 和无法解析的扩展事件会被忽略，调用方可
/// 在找不到终止事件时保留原始 body，行为与 sub2api 的非流式兜底一致。
pub fn for_each_json_data_value(body: &[u8], mut visit: impl FnMut(Value)) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    let mut data_lines = Vec::<&str>::new();
    let mut saw_data = false;
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some(data) = data_field_value_ref(line) {
            saw_data = true;
            data_lines.push(data);
            continue;
        }
        if line.trim().is_empty() {
            emit_json_data_values(&data_lines, &mut visit);
            data_lines.clear();
        }
    }
    emit_json_data_values(&data_lines, &mut visit);
    saw_data
}

/// 按 SSE 空行边界切分一个完整响应体，并保留每个 item 的原始换行风格和终止空行。
/// 测试服务用它模拟宿主交给流式插件的“完整 SSE item”输入，避免自行复制事件转换逻辑。
pub fn split_sse_items(body: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let text =
        std::str::from_utf8(body).map_err(|error| format!("SSE 响应体不是合法 UTF-8: {error}"))?;
    let mut items = Vec::new();
    let mut current = String::new();
    for line in split_lines(text) {
        current.push_str(&line.content);
        current.push_str(line.ending);
        if line.content.is_empty() && !current.is_empty() {
            items.push(std::mem::take(&mut current).into_bytes());
        }
    }
    if !current.is_empty() {
        items.push(current.into_bytes());
    }
    Ok(items)
}

fn emit_json_data_values(lines: &[&str], visit: &mut impl FnMut(Value)) {
    if lines.is_empty() {
        return;
    }
    if lines.len() == 1 {
        emit_json_data_value(lines[0], visit);
        return;
    }
    let joined = lines.join("\n");
    if let Ok(value) = serde_json::from_str::<Value>(joined.trim()) {
        visit(value);
        return;
    }
    // 一些兼容上游省略事件间空行；拼接后不是一个 JSON 时按独立 data 行处理。
    for line in lines {
        emit_json_data_value(line, visit);
    }
}

fn emit_json_data_value(data: &str, visit: &mut impl FnMut(Value)) {
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return;
    }
    if let Ok(value) = serde_json::from_str::<Value>(data) {
        visit(value);
    }
}

/// 宿主传给 stream 插件的是一个已经切好的完整 SSE item。本类型只解析 `data`
/// 字段；`event`、`id`、`retry`、注释和原始换行风格均保留。多条 data 行按 SSE 规范
/// 用换行拼接，改写后收敛成一条紧凑 JSON data 行。
#[derive(Debug)]
pub struct JsonSseItem {
    lines: Vec<Line>,
    value: Value,
}

#[derive(Debug)]
struct Line {
    content: String,
    ending: &'static str,
    is_data: bool,
}

impl JsonSseItem {
    pub fn parse(item: &[u8]) -> Result<Option<Self>, String> {
        let text = std::str::from_utf8(item)
            .map_err(|error| format!("SSE item 不是合法 UTF-8: {error}"))?;
        let mut lines = split_lines(text);
        let data = lines
            .iter_mut()
            .filter_map(|line| {
                let value = data_field_value(&line.content)?;
                line.is_data = true;
                Some(value)
            })
            .collect::<Vec<_>>();
        if data.is_empty() {
            return Ok(None);
        }
        let data = data.join("\n");
        if data.trim().is_empty() || data.trim() == "[DONE]" {
            return Ok(None);
        }
        let value = serde_json::from_str::<Value>(&data)
            .map_err(|error| format!("SSE data 不是合法 JSON: {error}"))?;
        Ok(Some(Self { lines, value }))
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut Value {
        &mut self.value
    }

    pub fn render(self) -> Result<Vec<u8>, String> {
        let encoded = serde_json::to_string(&self.value)
            .map_err(|error| format!("改造后的 SSE data 无法序列化: {error}"))?;
        let mut output = String::new();
        let mut data_written = false;
        for line in self.lines {
            if line.is_data {
                if data_written {
                    continue;
                }
                output.push_str("data: ");
                output.push_str(&encoded);
                output.push_str(line.ending);
                data_written = true;
                continue;
            }
            output.push_str(&line.content);
            output.push_str(line.ending);
        }
        Ok(output.into_bytes())
    }
}

fn split_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut remaining = text;
    while let Some(newline) = remaining.find('\n') {
        let (raw_line, rest) = remaining.split_at(newline);
        let (content, ending) = if let Some(content) = raw_line.strip_suffix('\r') {
            (content, "\r\n")
        } else {
            (raw_line, "\n")
        };
        lines.push(Line {
            content: content.to_owned(),
            ending,
            is_data: false,
        });
        remaining = &rest[1..];
    }
    if !remaining.is_empty() {
        lines.push(Line {
            content: remaining.to_owned(),
            ending: "",
            is_data: false,
        });
    }
    lines
}

fn data_field_value(line: &str) -> Option<String> {
    data_field_value_ref(line).map(str::to_owned)
}

fn data_field_value_ref(line: &str) -> Option<&str> {
    if line == "data" {
        return Some("");
    }
    let value = line.strip_prefix("data:")?;
    Some(value.strip_prefix(' ').unwrap_or(value))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn event_metadata_and_crlf_are_preserved() {
        let input = b"event: response.completed\r\nid: 7\r\ndata: {\"type\":\"response.completed\",\"n\":1}\r\n\r\n";
        let mut item = JsonSseItem::parse(input).unwrap().unwrap();
        item.value_mut()["n"] = json!(2);
        let output = item.render().unwrap();
        assert_eq!(
            output,
            b"event: response.completed\r\nid: 7\r\ndata: {\"n\":2,\"type\":\"response.completed\"}\r\n\r\n"
        );
    }

    #[test]
    fn done_and_comment_items_pass_the_json_parser_fast_path() {
        assert!(JsonSseItem::parse(b"data: [DONE]\n\n").unwrap().is_none());
        assert!(JsonSseItem::parse(b": keepalive\n\n").unwrap().is_none());
    }

    #[test]
    fn multiple_data_lines_are_joined_and_compacted() {
        let input = b"event: x\ndata: {\"a\":\ndata: 1}\n\n";
        let item = JsonSseItem::parse(input).unwrap().unwrap();
        assert_eq!(item.value(), &json!({"a": 1}));
        assert_eq!(item.render().unwrap(), b"event: x\ndata: {\"a\":1}\n\n");
    }

    #[test]
    fn complete_body_parser_handles_frames_and_missing_blank_lines() {
        let body = b"event: response.created\r\ndata: {\"type\":\"response.created\"}\r\n\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\ndata: {\"type\":\"response.completed\"}\ndata: [DONE]\n\n";
        assert!(body_has_sse_framing(body));
        let mut types = Vec::new();
        assert!(for_each_json_data_value(body, |value| {
            types.push(value["type"].as_str().unwrap().to_owned());
        }));
        assert_eq!(
            types,
            [
                "response.created",
                "response.output_text.delta",
                "response.completed"
            ]
        );
        assert!(!body_has_sse_framing(
            br#"{"output":[{"text":"data: not framing"}]}"#
        ));
    }
}
