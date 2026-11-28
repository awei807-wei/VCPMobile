//! 服务端托管 Function Call（`<<<[HOSTED_TOOL_CALL]>>>`）的展示解析与上下文剥离。
//!
//! VCPToolBox 在 `VCP_FC_FORWARD_TO_CLIENT=true` 时会把原生 OpenAI `tool_calls`
//! 原样转发给客户端，客户端**只做展示与存档，绝不重复执行**。
//! 这里负责把内嵌协议块还原成卡片字段，并在回灌模型前把展示标记剥离干净。

use serde_json::Value;

use crate::vcp_modules::chat::daily_note::DailyNoteDetails;

pub(crate) const HOSTED_TOOL_CALL_START: &str = "<<<[HOSTED_TOOL_CALL]>>>";
pub(crate) const HOSTED_TOOL_CALL_END: &str = "<<<[END_HOSTED_TOOL_CALL]>>>";

/// 单条托管 Function Call 的展示视图。
pub(crate) struct HostedToolCallView {
    /// 卡片标题：`args.tool_name` 优先，回退 `function.name`。
    pub display_name: String,
    /// 折叠详情：`args` 对象展开为 `key: value`，非合法 JSON 时保留原文。
    pub details: String,
    pub call_id: String,
    pub function_name: String,
    /// `vcptools` + `tool_name=dailynote` 时直接渲染为日记卡片。
    pub daily_note: Option<DailyNoteDetails>,
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn value_to_field(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) if text.trim().is_empty() => None,
        other => Some(value_to_text(other)),
    }
}

fn field(object: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value_to_field(value))
    })
}

/// `vcptools` 里的 DailyNote 调用还原为日记卡片字段，其余返回 None。
fn daily_note_from_args(args: &Value) -> Option<DailyNoteDetails> {
    let object = args.as_object()?;
    let command = field(object, &["command"])
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let target = field(object, &["target"]);
    let replace = field(object, &["replace"]);
    let content = field(object, &["content"]);
    let is_update =
        command == "update" || (command.is_empty() && target.is_some() && replace.is_some());
    let is_create =
        !is_update && (command == "create" || (command.is_empty() && content.is_some()));
    if !is_update && !is_create {
        return None;
    }

    let (agent_name, agent_type, agent_label) = match field(object, &["valet"]) {
        Some(valet) => (valet, "valet".to_string(), "Valet".to_string()),
        None => (
            field(object, &["maid"]).unwrap_or_default(),
            "maid".to_string(),
            "Maid".to_string(),
        ),
    };
    let folder = field(object, &["folder"]);

    if is_update {
        return Some(DailyNoteDetails {
            mode: "update".to_string(),
            agent_type,
            agent_label,
            agent_name,
            date: String::new(),
            content: String::new(),
            file_name: None,
            folder,
            tag: None,
            target,
            replace,
        });
    }

    Some(DailyNoteDetails {
        mode: "create".to_string(),
        agent_type,
        agent_label,
        agent_name,
        date: field(object, &["date"]).unwrap_or_default(),
        content: content.unwrap_or_else(|| "[日记内容解析失败]".to_string()),
        file_name: field(object, &["fileName", "file_name"]),
        folder,
        tag: field(object, &["tag"]),
        target: None,
        replace: None,
    })
}

/// 解析协议块内部 JSON（单条 `{"id","type","function":{"name","arguments"}}`）。
pub(crate) fn parse_hosted_tool_call(inner: &str) -> HostedToolCallView {
    let parsed = serde_json::from_str::<Value>(inner.trim()).ok();
    let call_id = parsed
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let function_name = parsed
        .as_ref()
        .and_then(|value| value.get("function"))
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let raw_arguments = parsed
        .as_ref()
        .and_then(|value| value.get("function"))
        .and_then(|function| function.get("arguments"))
        .map(value_to_text)
        .unwrap_or_default();
    let args = serde_json::from_str::<Value>(&raw_arguments).ok();

    let tool_name = args
        .as_ref()
        .and_then(|value| value.get("tool_name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let display_name = if tool_name.is_empty() {
        function_name.clone()
    } else {
        tool_name.clone()
    };
    let display_name = if display_name.is_empty() {
        "未命名".to_string()
    } else {
        display_name
    };

    let daily_note = if function_name.eq_ignore_ascii_case("vcptools")
        && tool_name.eq_ignore_ascii_case("dailynote")
    {
        args.as_ref().and_then(daily_note_from_args)
    } else {
        None
    };

    let details = match args.as_ref() {
        Some(Value::Object(map)) if !map.is_empty() => map
            .iter()
            .map(|(key, value)| format!("{}: {}", key, value_to_text(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => raw_arguments,
    };

    HostedToolCallView {
        display_name,
        details,
        call_id,
        function_name,
        daily_note,
    }
}

fn collapse_blank_lines(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            blank_run += 1;
        } else {
            blank_run = 0;
        }
        if blank_run > 1 {
            continue;
        }
        output.push_str(line);
    }
    output
}

/// 剥离 `<<<[HOSTED_TOOL_CALL]>>>…<<<[END_HOSTED_TOOL_CALL]>>>` 展示块。
///
/// 仅匹配行首标记，代码围栏内的字面量示例原样保留；
/// 存在未闭合块时整体放弃剥离，避免误删正文。
pub(crate) fn strip_hosted_tool_calls(text: &str) -> String {
    if !text.contains("HOSTED_TOOL_CALL") {
        return text.to_string();
    }
    let mut output = String::with_capacity(text.len());
    let mut in_fence = false;
    let mut in_block = false;
    let mut stripped = false;
    let mut closed = true;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !in_block && trimmed.starts_with("```") {
            in_fence = !in_fence;
            output.push_str(line);
            continue;
        }
        if in_fence {
            output.push_str(line);
            continue;
        }
        if !in_block && trimmed.starts_with(HOSTED_TOOL_CALL_START) {
            in_block = true;
            closed = false;
            stripped = true;
            continue;
        }
        if in_block {
            if trimmed.starts_with(HOSTED_TOOL_CALL_END) {
                in_block = false;
                closed = true;
            }
            continue;
        }
        output.push_str(line);
    }
    if !stripped || !closed {
        return text.to_string();
    }
    collapse_blank_lines(&output)
}

#[cfg(test)]
mod tests {
    use super::{parse_hosted_tool_call, strip_hosted_tool_calls};

    const VCPTOOLS_CALL: &str = r#"{"id":"call_1","type":"function","function":{"name":"vcptools","arguments":"{\"tool_name\":\"GitSearch\",\"query\":\"fn main\"}"}}"#;

    #[test]
    fn hosted_tool_call_uses_args_tool_name_and_expands_details() {
        let view = parse_hosted_tool_call(VCPTOOLS_CALL);
        assert_eq!(view.display_name, "GitSearch");
        assert_eq!(view.function_name, "vcptools");
        assert_eq!(view.call_id, "call_1");
        assert!(view.details.contains("query: fn main"));
        assert!(view.daily_note.is_none());
    }

    #[test]
    fn hosted_daily_note_is_rendered_as_diary_fields() {
        let call = r#"{"id":"call_2","type":"function","function":{"name":"vcptools","arguments":"{\"tool_name\":\"DailyNote\",\"command\":\"create\",\"maid\":\"小克\",\"Date\":\"2026.6.11\",\"Content\":\"正文\"}"}}"#;
        let view = parse_hosted_tool_call(call);
        let details = view.daily_note.expect("daily note");
        assert_eq!(details.mode, "create");
        assert_eq!(details.agent_name, "小克");
        assert_eq!(details.content, "正文");
    }

    #[test]
    fn strip_removes_only_line_anchored_blocks() {
        let text = format!(
            "提问\n\n<<<[HOSTED_TOOL_CALL]>>>\n{VCPTOOLS_CALL}\n<<<[END_HOSTED_TOOL_CALL]>>>\n\n后续"
        );
        let stripped = strip_hosted_tool_calls(&text);
        assert!(stripped.contains("提问"));
        assert!(stripped.contains("后续"));
        assert!(!stripped.contains("HOSTED_TOOL_CALL"));

        let fenced = String::from("```text\n<<<[HOSTED_TOOL_CALL]>>>\n```");
        assert_eq!(strip_hosted_tool_calls(&fenced), fenced);
    }
}
