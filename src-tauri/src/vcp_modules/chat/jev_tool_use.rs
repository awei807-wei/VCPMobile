//! JEV（自然语言工具请求）展示元信息。
//!
//! JEV 是工具请求里的一个字段，形如：
//! `JEV:「始」联络[Grok]，告诉他【...】，并使用[临时通讯]。「末」`
//!
//! 它本身不携带 `tool_name`，因此展示层需要单独识别：一旦工具请求中出现 JEV 字段，
//! 该块就以 `JEVToolUse` 身份渲染（桌面端 `.vcp-jev-tool-use-bubble`）。
//!
//! 展示名优先级（与桌面端 `modules/renderer/jevToolUse.js` 一致）：
//! 1. 引号/反引号内的显式工具名，例如 `'GPT生图'`、`` `GPT生图` ``；
//! 2. 花括号内的能力声明，例如 `{联网搜索}`；
//! 3. 都没有时 `display_name` 为空，卡片只显示 `JEVToolUse` 标签。
//!
//! 本模块只影响展示，不做 JEV → 传统插件字段的翻译。

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// 字段声明：`字段名:「始」...` 或 `字段名:{始}...`（含 `escape` 变体）。
    /// 允许出现在内容开头、换行后或逗号后（与桌面端 FIELD_START_REGEX 同构）。
    static ref FIELD_START: Regex = Regex::new(
        r"(?im)(?:^|\n|,)[ \t]*([^\s,:：「」{}]+)[ \t]*[:：][ \t]*(「始(?:escape)?」|\{始(?:escape)?\})"
    )
    .unwrap();
    /// 字段内容在进入展示转换前会做 HTML 转义，ASCII 单引号通常已是实体。
    static ref QUOTE_ENTITY: Regex = Regex::new(r"(?i)&#0*39;").unwrap();
    /// 能力声明：花括号内容，禁止嵌套花括号与换行。
    static ref CAPABILITY: Regex = Regex::new(r"\{([^{}\r\n]+)\}").unwrap();
    /// 显式工具名：ASCII 单引号、反引号、中文单引号、中文双引号。
    static ref EXPLICIT_NAME: Regex =
        Regex::new(r"'([^'\r\n]+)'|`([^`\r\n]+)`|‘([^’\r\n]+)’|“([^”\r\n]+)”").unwrap();
}

/// JEV 字段的展示元信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JevToolUse {
    /// JEV 字段的原始内容（未 trim 前的值已 trim 首尾空白）。
    pub raw: String,
    /// 卡片上展示的名字（显式工具名优先，其次能力）。
    pub display_name: String,
    /// 引号/反引号内的显式工具名。
    pub explicit_tool_name: String,
    /// 花括号内的能力声明。
    pub capability: String,
}

/// 由起始标记推导结束标记：`「始」` → `「末」`、`{始}` → `{末}`，
/// `escape` 变体分别推导为 `「末ESCAPE」` / `{末ESCAPE}`。
pub fn field_end_marker(start_marker: &str) -> String {
    let use_brace = start_marker.starts_with('{');
    let is_escape = start_marker.to_ascii_lowercase().contains("escape");
    format!(
        "{}末{}{}",
        if use_brace { "{" } else { "「" },
        if is_escape { "ESCAPE" } else { "" },
        if use_brace { "}" } else { "」" }
    )
}

/// 读取工具请求中的一个字段值（字段名大小写不敏感）。
///
/// 顺序扫描字段声明并跳过前序字段载荷，避免把别的字段正文里出现的
/// `JEV:「始」` 误当成真实字段。未闭合时取到文末（流式中间态）。
pub fn read_tool_request_field(content: &str, name: &str) -> Option<String> {
    if content.is_empty() {
        return None;
    }

    let mut cursor = 0usize;
    loop {
        let captures = FIELD_START.captures_at(content, cursor)?;
        let field_name = captures.get(1)?.as_str();
        let marker = captures.get(2)?;
        let value_start = marker.end();
        let end_marker = field_end_marker(marker.as_str());
        let end_regex = Regex::new(&format!("(?i){}", regex::escape(&end_marker))).ok()?;
        let end_match = end_regex.find_at(content, value_start);
        let (value_end, next_cursor) = match end_match {
            Some(found) => (found.start(), Some(found.start() + end_marker.len())),
            None => (content.len(), None),
        };

        if field_name.eq_ignore_ascii_case(name) {
            return Some(content[value_start..value_end].to_string());
        }

        match next_cursor {
            Some(next) if next > cursor => cursor = next,
            _ => return None,
        }
    }
}

/// 解析 JEV 字段内容，得到展示元信息。
///
/// 只要字段存在且有内容就视为 JEV 调用；没有显式名字时 `display_name` 为空。
pub fn parse_jev(value: &str) -> Option<JevToolUse> {
    let raw = value.trim().to_string();
    if raw.is_empty() {
        return None;
    }

    let normalized = QUOTE_ENTITY.replace_all(&raw, "'");
    let capability = CAPABILITY
        .captures(&normalized)
        .and_then(|captures| captures.get(1))
        .map(|matched| matched.as_str().trim().to_string())
        .unwrap_or_default();
    let explicit_tool_name = EXPLICIT_NAME
        .captures(&normalized)
        .and_then(|captures| (1..=4).find_map(|index| captures.get(index)))
        .map(|matched| matched.as_str().trim().to_string())
        .unwrap_or_default();
    let display_name = if explicit_tool_name.is_empty() {
        capability.clone()
    } else {
        explicit_tool_name.clone()
    };

    Some(JevToolUse {
        raw,
        display_name,
        explicit_tool_name,
        capability,
    })
}

/// 从工具请求内容中识别 JEV 调用；不是 JEV 请求时返回 `None`。
pub fn detect_jev_use(content: &str) -> Option<JevToolUse> {
    read_tool_request_field(content, "JEV").and_then(|value| parse_jev(&value))
}

#[cfg(test)]
mod tests {
    use super::{
        detect_jev_use, field_end_marker, parse_jev, read_tool_request_field,
    };

    const TOOL_REQUEST: &str = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」SciCalculator「末」,\nexpression:「始」1+1「末」\n<<<[END_TOOL_REQUEST]>>>";

    #[test]
    fn reads_field_value_between_markers() {
        assert_eq!(
            read_tool_request_field(TOOL_REQUEST, "tool_name").as_deref(),
            Some("SciCalculator")
        );
        assert_eq!(
            read_tool_request_field(TOOL_REQUEST, "expression").as_deref(),
            Some("1+1")
        );
        assert_eq!(read_tool_request_field(TOOL_REQUEST, "JEV"), None);
    }

    #[test]
    fn skips_payload_of_previous_fields() {
        let content = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」Demo「末」,\nnote:「始」正文里提到 JEV:「始」伪字段「末」「末」,\nJEV:「始」真实请求「末」";
        assert_eq!(
            read_tool_request_field(content, "JEV").as_deref(),
            Some("真实请求")
        );
    }

    #[test]
    fn supports_brace_and_escape_markers() {
        assert_eq!(field_end_marker("「始」"), "「末」");
        assert_eq!(field_end_marker("{始}"), "{末}");
        assert_eq!(field_end_marker("「始escape」"), "「末ESCAPE」");
        assert_eq!(field_end_marker("{始escape}"), "{末ESCAPE}");

        let content = "<<<[TOOL_REQUEST]>>>\nJEV:{始}联络 {联网搜索}{末}";
        assert!(read_tool_request_field(content, "JEV").is_some());
    }

    #[test]
    fn parses_explicit_tool_name_from_quotes_and_backticks() {
        assert_eq!(
            parse_jev("联络 'GPT生图'，告诉他画一张猫").unwrap().display_name,
            "GPT生图"
        );
        assert_eq!(
            parse_jev("联络 `GPT生图`").unwrap().display_name,
            "GPT生图"
        );
        assert_eq!(
            parse_jev("使用 ‘临时通讯’ 联系").unwrap().display_name,
            "临时通讯"
        );
        assert_eq!(
            parse_jev("使用 “临时通讯” 联系").unwrap().display_name,
            "临时通讯"
        );
    }

    #[test]
    fn falls_back_to_capability_brace() {
        let parsed = parse_jev("请执行 {联网搜索} 查一下").unwrap();
        assert_eq!(parsed.display_name, "联网搜索");
        assert_eq!(parsed.capability, "联网搜索");
        assert_eq!(parsed.explicit_tool_name, "");
    }

    #[test]
    fn normalizes_escaped_single_quotes() {
        let parsed = parse_jev("联络 &#039;GPT生图&#039;").unwrap();
        assert_eq!(parsed.display_name, "GPT生图");
    }

    #[test]
    fn natural_language_jev_without_name_is_still_jev() {
        let content = "<<<[TOOL_REQUEST]>>>\nJEV:「始」联络[Grok]，告诉他【npm 上 animejs 最新版本是多少？】。「末」\n<<<[END_TOOL_REQUEST]>>>";
        let parsed = detect_jev_use(content).expect("JEV detected");
        assert_eq!(parsed.display_name, "");
        assert!(parsed.raw.starts_with("联络[Grok]"));
    }

    #[test]
    fn plain_tool_request_is_not_jev() {
        assert!(detect_jev_use(TOOL_REQUEST).is_none());
    }
}
