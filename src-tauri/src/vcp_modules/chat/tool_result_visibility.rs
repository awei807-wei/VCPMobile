//! 工具结果（插件回调）可见性控制。
//!
//! VCP 后端一次性注入的 `[[VCP调用结果信息汇总: ... VCP调用结果结束]]` 块默认参与
//! 上下文装配。用户可以在消息气泡上手动把某个工具结果标记为“隐藏”，标记写回原始
//! 消息内容（`[HIDDEN]` 前缀或 `- 注入上下文: 隐藏` 元数据行），此后：
//! 1. `context_assembler` 装配 AI 上下文时会把该块整段剔除，等价于节省 Token；
//! 2. 消息仍然完整保留在历史里，用户可以随时恢复注入。
//!
//! 本模块不依赖 `content_parser`，避免循环导入。

use lazy_static::lazy_static;
use regex::Regex;

/// 工具结果块起始标记（与前端、content_parser 保持一致）。
pub const TOOL_RESULT_START_MARKER: &str = "[[VCP调用结果信息汇总:";
/// 工具结果块结束标记。
pub const TOOL_RESULT_END_MARKER: &str = "VCP调用结果结束]]";
/// 隐藏标记，写在起始标记之后。
pub const HIDDEN_TOKEN: &str = "[HIDDEN]";

lazy_static! {
    /// 起始标记之后的隐藏标记（`[HIDDEN]` / `<!--HIDDEN-->` / `(HIDDEN)`）。
    static ref HIDDEN_TOKEN_PREFIX: Regex =
        Regex::new(r"(?i)^[ \t]*(?:\[HIDDEN\]|<!--HIDDEN-->|\(HIDDEN\))").unwrap();
    /// 元数据行形式的隐藏声明，例如 `- 注入上下文: 隐藏`。
    static ref HIDDEN_METADATA: Regex = Regex::new(concat!(
        r"(?im)(?:\r?\n|^)[ \t]*-[ \t]*",
        r"(?:注入上下文|上下文注入|上下文状态|Context)",
        r":\s*(?:隐藏|hidden|false|否)",
    ))
    .unwrap();
}

/// 工具结果块在原始文本中的区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolResultRange {
    pub start: usize,
    pub end: usize,
    pub closed: bool,
}

/// 嵌套感知地查找工具结果的结束位置。
///
/// 工具结果内部可能包含字面量起止标记（例如 AI 读取了本模块源码）。
/// 采用深度计数：起始标记 +1、结束标记 -1，深度回到 0 时外层块才闭合。
pub fn find_tool_result_end(text: &str, start_index: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut cursor = start_index + TOOL_RESULT_START_MARKER.len();
    if cursor > text.len() {
        return None;
    }
    let mut next_start = text[cursor..]
        .find(TOOL_RESULT_START_MARKER)
        .map(|index| index + cursor);

    loop {
        let next_end = text[cursor..]
            .find(TOOL_RESULT_END_MARKER)
            .map(|index| index + cursor);
        let next_end = match next_end {
            Some(value) => value,
            None => return None,
        };

        if let Some(next_start_index) = next_start {
            if next_start_index < next_end {
                depth += 1;
                cursor = next_start_index + TOOL_RESULT_START_MARKER.len();
                next_start = text[cursor..]
                    .find(TOOL_RESULT_START_MARKER)
                    .map(|index| index + cursor);
                continue;
            }
        }

        depth -= 1;
        cursor = next_end + TOOL_RESULT_END_MARKER.len();
        if depth == 0 {
            return Some(cursor);
        }
        if let Some(next_start_index) = next_start {
            if next_start_index < cursor {
                next_start = text[cursor..]
                    .find(TOOL_RESULT_START_MARKER)
                    .map(|index| index + cursor);
            }
        }
    }
}

/// 收集最外层工具结果区间（按源码顺序、互不重叠）。
/// 未闭合区间延伸到文末（`closed: false`）。
pub fn collect_tool_result_ranges(text: &str) -> Vec<ToolResultRange> {
    let mut ranges = Vec::new();
    if text.is_empty() {
        return ranges;
    }

    let mut cursor = 0usize;
    while cursor < text.len() {
        let start = match text[cursor..].find(TOOL_RESULT_START_MARKER) {
            Some(index) => index + cursor,
            None => break,
        };

        match find_tool_result_end(text, start) {
            Some(end) => {
                ranges.push(ToolResultRange {
                    start,
                    end,
                    closed: true,
                });
                cursor = end;
            }
            None => {
                ranges.push(ToolResultRange {
                    start,
                    end: text.len(),
                    closed: false,
                });
                break;
            }
        }
    }

    ranges
}

/// 只保留已闭合的最外层工具结果区间。
pub fn collect_closed_tool_result_ranges(text: &str) -> Vec<ToolResultRange> {
    collect_tool_result_ranges(text)
        .into_iter()
        .filter(|range| range.closed)
        .collect()
}

/// 去掉可选的起始/结束标记，返回块内部正文。
pub fn tool_result_body(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let after_start = match trimmed.strip_prefix(TOOL_RESULT_START_MARKER) {
        Some(rest) => rest,
        None => trimmed,
    };
    match after_start.rfind(TOOL_RESULT_END_MARKER) {
        Some(index) if after_start[index + TOOL_RESULT_END_MARKER.len()..].trim().is_empty() => {
            &after_start[..index]
        }
        _ => after_start,
    }
}

/// 判断一个工具结果块是否被标记为隐藏（不注入上下文）。
pub fn is_tool_result_hidden(raw: &str) -> bool {
    if raw.is_empty() {
        return false;
    }
    let body = tool_result_body(raw);
    if HIDDEN_TOKEN_PREFIX.is_match(body) {
        return true;
    }
    // 兼容元数据行形式：只看首部 300 个字符，避免扫描大块内容。
    let head: String = body.chars().take(300).collect();
    HIDDEN_METADATA.is_match(&head)
}

/// 去掉正文首部的隐藏标记与元数据行，用于正常解析工具字段。
pub fn strip_hidden_marker(body: &str) -> String {
    let without_token = HIDDEN_TOKEN_PREFIX.replace(body, "");
    let trimmed = without_token.trim_start_matches([' ', '\t']);
    HIDDEN_METADATA.replace(trimmed, "").into_owned()
}

/// 设置工具结果块的隐藏状态，返回更新后的文本。
///
/// 只有首部（前导空白之后）就是起始标记的切片才会被修改，其余原样返回。
pub fn set_tool_result_hidden(raw: &str, hidden: bool) -> String {
    if raw.is_empty() {
        return raw.to_string();
    }
    if hidden == is_tool_result_hidden(raw) {
        return raw.to_string();
    }

    let marker_pos = match raw.find(TOOL_RESULT_START_MARKER) {
        Some(position) => position,
        None => return raw.to_string(),
    };
    if !raw[..marker_pos].trim().is_empty() {
        return raw.to_string();
    }

    let body_start = marker_pos + TOOL_RESULT_START_MARKER.len();
    if hidden {
        let mut out = String::with_capacity(raw.len() + HIDDEN_TOKEN.len() + 1);
        out.push_str(&raw[..body_start]);
        out.push(' ');
        out.push_str(HIDDEN_TOKEN);
        out.push_str(&raw[body_start..]);
        out
    } else {
        let mut out = String::with_capacity(raw.len());
        out.push_str(&raw[..body_start]);
        let restored = strip_hidden_marker(&raw[body_start..]);
        out.push_str(&restored);
        out
    }
}

/// 剥离文本中所有被标记为隐藏的工具结果块，并收拢多余空行。
///
/// 在装配提交给 AI 的上下文时调用，效果与物理删除一致（节省 Token、净化上下文）。
pub fn strip_hidden_tool_results(text: &str) -> String {
    if !text.contains(TOOL_RESULT_START_MARKER) {
        return text.to_string();
    }
    let ranges = collect_closed_tool_result_ranges(text);
    if ranges.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut stripped = false;
    for range in ranges {
        if is_tool_result_hidden(&text[range.start..range.end]) {
            out.push_str(&text[cursor..range.start]);
            cursor = range.end;
            stripped = true;
        }
    }
    if !stripped {
        return text.to_string();
    }
    out.push_str(&text[cursor..]);
    collapse_blank_lines(&out)
}

/// 与前端 `\n{3,}` -> `\n\n` 保持一致。
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newline_run = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(ch);
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }
    out
}

/// FNV-1a 64 位哈希（十六进制），与前端 `fnv1aHashHex` 逐字节一致。
pub fn fnv1a_hash_hex(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 工具结果正文（去首尾空白）的哈希，用于前端精确回指到原始文本中的块。
pub fn tool_result_body_hash(body: &str) -> String {
    fnv1a_hash_hex(body.trim())
}

#[cfg(test)]
mod tests {
    use super::{
        collect_closed_tool_result_ranges, is_tool_result_hidden, set_tool_result_hidden,
        strip_hidden_tool_results, tool_result_body_hash,
    };

    fn visible_block() -> String {
        "[[VCP调用结果信息汇总:\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n- 返回内容: 结果\nVCP调用结果结束]]".to_string()
    }

    #[test]
    fn visible_block_is_not_hidden() {
        assert!(!is_tool_result_hidden(&visible_block()));
    }

    #[test]
    fn hide_then_restore_is_lossless() {
        let original = visible_block();
        let hidden = set_tool_result_hidden(&original, true);
        assert!(hidden.starts_with("[[VCP调用结果信息汇总: [HIDDEN]"));
        assert!(is_tool_result_hidden(&hidden));

        let restored = set_tool_result_hidden(&hidden, false);
        assert_eq!(restored, original);
        assert!(!is_tool_result_hidden(&restored));
    }

    #[test]
    fn metadata_line_marks_block_hidden() {
        let raw = "[[VCP调用结果信息汇总:\n- 注入上下文: 隐藏\n- 工具名称: WebSearch\nVCP调用结果结束]]";
        assert!(is_tool_result_hidden(raw));
        let restored = set_tool_result_hidden(raw, false);
        assert!(!is_tool_result_hidden(&restored));
        assert!(restored.contains("- 工具名称: WebSearch"));
        assert!(!restored.contains("注入上下文"));
    }

    #[test]
    fn strip_keeps_visible_blocks_and_removes_hidden_ones() {
        let hidden = set_tool_result_hidden(&visible_block(), true);
        let visible = visible_block();
        let text = format!("前文\n\n{hidden}\n\n{visible}后文");
        let stripped = strip_hidden_tool_results(&text);
        assert!(!stripped.contains("[HIDDEN]"));
        assert!(stripped.contains("- 工具名称: WebSearch"));
        assert!(stripped.starts_with("前文"));
        assert!(stripped.ends_with("后文"));
    }

    #[test]
    fn nested_end_marker_does_not_truncate_outer_block() {
        let text = "[[VCP调用结果信息汇总:\n- 工具名称: Reader\n- 返回内容: 源码里有 [[VCP调用结果信息汇总: 内层 VCP调用结果结束]] 字样\nVCP调用结果结束]]尾部";
        let ranges = collect_closed_tool_result_ranges(text);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&text[ranges[0].end..], "尾部");
    }

    #[test]
    fn body_hash_is_stable_across_markers() {
        let body = "\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n";
        let raw = format!("[[VCP调用结果信息汇总:{body}VCP调用结果结束]]");
        assert_eq!(
            tool_result_body_hash(super::tool_result_body(&raw)),
            tool_result_body_hash(body)
        );
    }
}
