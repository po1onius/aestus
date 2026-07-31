use axum::body::Bytes;

/// 从网络缓冲中取出所有完整的原始 SSE item，并保留原始换行字节。
///
/// SSE 允许 LF、CRLF 和 CR 三种行结束符；空行结束一个 item。这里不解析 `event`/`data`
/// 字段，避免宿主在插件接管前改变 wire 内容。尚未出现结束空行的尾部继续留在缓冲中。
pub(crate) fn drain_complete_items(buffer: &mut Vec<u8>) -> Vec<Bytes> {
    let mut items = Vec::new();
    while let Some(item_end) = first_item_end(buffer) {
        items.push(Bytes::from(buffer.drain(..item_end).collect::<Vec<_>>()));
    }
    items
}

/// 插件的一个输出元素必须恰好是一个完整 SSE item。这样宿主可以稳定地在 item 与
/// effects 之间建立一一对应关系；需要补发多个 item 时应使用 `finish-output.items`。
pub(crate) fn is_exact_item(bytes: &[u8]) -> bool {
    first_item_end(bytes).is_some_and(|item_end| item_end == bytes.len())
}

fn first_item_end(bytes: &[u8]) -> Option<usize> {
    let mut line_start = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let ending_len = match bytes[cursor] {
            b'\n' => 1,
            b'\r' if bytes.get(cursor + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                cursor += 1;
                continue;
            }
        };
        let next_line_start = cursor + ending_len;
        if cursor == line_start {
            return Some(next_line_start);
        }
        line_start = next_line_start;
        cursor = next_line_start;
    }
    None
}
