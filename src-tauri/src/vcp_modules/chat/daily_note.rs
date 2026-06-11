//! DailyNote 共享字段解析。
//!
//! 静态解析（`content_parser`）与流式解析（`stream_block_parser`）共用同一套
//! 解析逻辑，保证两条路径对同一样例输出一致字段。
//! 语义对齐 VCPChat `messageRenderer.js` 的 DailyNote create/update/legacy 渲染：
//! - tool_name 为 DailyNote 且 command=update（或缺省但同时有 target+replace）→ update
//! - tool_name 为 DailyNote 且 command=create（或缺省但有 Content）→ create
//! - `<<<DailyNoteStart>>>` 老式块 → legacy
//! 字段值兼容 `「始」…「末」` / `「始exp」…「末exp」` / `{始}…{末}` 标记形式与普通 `Key:` 行。

use lazy_static::lazy_static;
use regex::Regex;

/// 解析后的 DailyNote 字段集合。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DailyNoteDetails {
    /// "create" | "update" | "legacy"
    pub mode: String,
    /// "maid" | "valet"
    pub agent_type: String,
    /// "Maid" | "Valet"
    pub agent_label: String,
    pub agent_name: String,
    pub date: String,
    pub content: String,
    pub file_name: Option<String>,
    pub folder: Option<String>,
    pub tag: Option<String>,
    pub target: Option<String>,
    pub replace: Option<String>,
}

lazy_static! {
    static ref MARK_START: Regex = Regex::new(r"(?i)[「{]始(?:escape|exp)?[」}]").unwrap();
    static ref MARK_END_PLAIN: Regex = Regex::new(r"[「{]末[」}]").unwrap();
    static ref MARK_END_ESCAPE: Regex = Regex::new(r"(?i)[「{]末(?:escape|exp)[」}]").unwrap();

    static ref LABEL_TOOL_NAME: Regex = Regex::new(r"(?i)\btool_name\s*:").unwrap();
    static ref LABEL_COMMAND: Regex = Regex::new(r"(?i)\bcommand\s*:").unwrap();
    static ref LABEL_MAID: Regex = Regex::new(r"(?i)\b(?:maidname|maid)\s*:").unwrap();
    static ref LABEL_VALET: Regex = Regex::new(r"(?i)\b(?:valetname|valet)\s*:").unwrap();
    static ref LABEL_DATE: Regex = Regex::new(r"(?i)\bdate\s*:").unwrap();
    static ref LABEL_FILE_NAME: Regex = Regex::new(r"(?i)\bfile_?name\s*:").unwrap();
    static ref LABEL_FOLDER: Regex = Regex::new(r"(?i)\bfolder\s*:").unwrap();
    static ref LABEL_TAG: Regex = Regex::new(r"(?i)\btag\s*:").unwrap();
    static ref LABEL_TARGET: Regex = Regex::new(r"(?i)\btarget\s*:").unwrap();
    static ref LABEL_REPLACE: Regex = Regex::new(r"(?i)\breplace\s*:").unwrap();
    static ref LABEL_CONTENT: Regex = Regex::new(r"(?i)\bcontent\s*:").unwrap();

    static ref XML_TOOL_NAME: Regex = Regex::new(r"(?i)<tool_name>([\s\S]*?)</tool_name>").unwrap();
}

/// 提取 `Label:「始」…「末」` 标记形式的字段值。
/// 字段名与起始标记之间只允许空白；缺少结束标记时取剩余全文（流式容错）。
fn extract_marked_field(source: &str, label: &Regex) -> Option<String> {
    let label_m = label.find(source)?;
    let after = &source[label_m.end()..];
    let start_m = MARK_START.find(after)?;
    if !after[..start_m.start()].trim().is_empty() {
        return None;
    }
    let marker_lower = start_m.as_str().to_lowercase();
    let is_escape = marker_lower.contains("escape") || marker_lower.contains("exp");
    let rest = &after[start_m.end()..];
    let end_re: &Regex = if is_escape {
        &MARK_END_ESCAPE
    } else {
        &MARK_END_PLAIN
    };
    let value = match end_re.find(rest) {
        Some(end_m) => &rest[..end_m.start()],
        None => rest,
    };
    Some(value.trim().to_string())
}

/// 提取普通 `Label: 单行值` 形式的字段值（去掉行尾逗号）。
fn extract_plain_line(source: &str, label: &Regex) -> Option<String> {
    let label_m = label.find(source)?;
    let line = source[label_m.end()..].lines().next().unwrap_or("");
    let value = line.trim().trim_end_matches(',').trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 标记形式优先，普通单行形式兜底。
fn extract_field(source: &str, label: &Regex) -> Option<String> {
    extract_marked_field(source, label).or_else(|| extract_plain_line(source, label))
}

/// Maid/Valet 代理信息：valet 字段存在时优先。
fn extract_agent(source: &str) -> (String, String, String) {
    if let Some(valet) = extract_field(source, &LABEL_VALET) {
        return (valet, "valet".to_string(), "Valet".to_string());
    }
    let maid = extract_field(source, &LABEL_MAID).unwrap_or_default();
    (maid, "maid".to_string(), "Maid".to_string())
}

/// 解析 TOOL_REQUEST 内容；非 DailyNote create/update 时返回 None（回退普通工具块）。
pub(crate) fn parse_daily_note_tool(content: &str) -> Option<DailyNoteDetails> {
    let tool_name = extract_field(content, &LABEL_TOOL_NAME).or_else(|| {
        XML_TOOL_NAME
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
    })?;
    if !tool_name.trim().eq_ignore_ascii_case("dailynote") {
        return None;
    }

    let command = extract_field(content, &LABEL_COMMAND)
        .map(|v| v.trim().to_lowercase())
        .unwrap_or_default();
    let target = extract_field(content, &LABEL_TARGET);
    let replace = extract_field(content, &LABEL_REPLACE);
    let note_content = extract_field(content, &LABEL_CONTENT);

    let is_update =
        command == "update" || (command.is_empty() && target.is_some() && replace.is_some());
    let is_create =
        !is_update && (command == "create" || (command.is_empty() && note_content.is_some()));
    if !is_update && !is_create {
        return None;
    }

    let (agent_name, agent_type, agent_label) = extract_agent(content);
    let folder = extract_field(content, &LABEL_FOLDER);

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
        date: extract_field(content, &LABEL_DATE).unwrap_or_default(),
        content: note_content.unwrap_or_else(|| "[日记内容解析失败]".to_string()),
        file_name: extract_field(content, &LABEL_FILE_NAME),
        folder,
        tag: extract_field(content, &LABEL_TAG),
        target: None,
        replace: None,
    })
}

/// 解析老式 `<<<DailyNoteStart>>>…<<<DailyNoteEnd>>>` 块内容。
pub(crate) fn parse_daily_note_legacy(content: &str) -> DailyNoteDetails {
    let maid = extract_field(content, &LABEL_MAID).unwrap_or_default();
    let date = extract_field(content, &LABEL_DATE).unwrap_or_default();
    // Content 标记形式优先；普通形式取 `Content:` 之后剩余全文；都缺失时整块作为正文
    let note_content = extract_marked_field(content, &LABEL_CONTENT)
        .or_else(|| {
            LABEL_CONTENT
                .find(content)
                .map(|m| content[m.end()..].trim().to_string())
        })
        .unwrap_or_else(|| content.trim().to_string());

    DailyNoteDetails {
        mode: "legacy".to_string(),
        agent_type: "maid".to_string(),
        agent_label: "Maid".to_string(),
        agent_name: maid,
        date,
        content: note_content,
        file_name: None,
        folder: None,
        tag: None,
        target: None,
        replace: None,
    }
}

/// 供流式解析做哈希指纹的字段串联（与静态路径无关，仅保证字段变化触发重渲染）。
pub(crate) fn fingerprint(details: &DailyNoteDetails) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
        details.mode,
        details.agent_name,
        details.date,
        details.content,
        details.file_name.as_deref().unwrap_or(""),
        details.folder.as_deref().unwrap_or(""),
        details.tag.as_deref().unwrap_or(""),
        details.target.as_deref().unwrap_or(""),
        details.replace.as_deref().unwrap_or(""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE_MARKED: &str = "maid:「始」小克「末」,\ntool_name:「始」DailyNote「末」,\ncommand:「始」create「末」,\nDate:「始」2026.6.11「末」,\nfileName:「始」周三随记「末」,\nfolder:「始」小克日记「末」,\nTag:「始」日常「末」,\nContent:「始」今天调试了响铃权限。\n\n继续修复更新检查。「末」";

    const UPDATE_MARKED: &str = "valet:「始」管家「末」,\ntool_name:「始」DailyNote「末」,\ncommand:「始」update「末」,\nfolder:「始」管家日志「末」,\ntarget:「始」旧的段落「末」,\nreplace:「始」新的段落「末」";

    #[test]
    fn create_marked_fields() {
        let d = parse_daily_note_tool(CREATE_MARKED).expect("should detect create");
        assert_eq!(d.mode, "create");
        assert_eq!(d.agent_name, "小克");
        assert_eq!(d.agent_type, "maid");
        assert_eq!(d.agent_label, "Maid");
        assert_eq!(d.date, "2026.6.11");
        assert_eq!(d.file_name.as_deref(), Some("周三随记"));
        assert_eq!(d.folder.as_deref(), Some("小克日记"));
        assert_eq!(d.tag.as_deref(), Some("日常"));
        assert!(d.content.contains("响铃权限"));
        assert!(d.content.contains("更新检查"));
        assert!(d.target.is_none());
    }

    #[test]
    fn update_marked_fields_with_valet() {
        let d = parse_daily_note_tool(UPDATE_MARKED).expect("should detect update");
        assert_eq!(d.mode, "update");
        assert_eq!(d.agent_name, "管家");
        assert_eq!(d.agent_type, "valet");
        assert_eq!(d.agent_label, "Valet");
        assert_eq!(d.folder.as_deref(), Some("管家日志"));
        assert_eq!(d.target.as_deref(), Some("旧的段落"));
        assert_eq!(d.replace.as_deref(), Some("新的段落"));
    }

    #[test]
    fn update_inferred_without_command() {
        let text = "tool_name:「始」DailyNote「末」,\ntarget:「始」A「末」,\nreplace:「始」B「末」";
        let d = parse_daily_note_tool(text).expect("target+replace implies update");
        assert_eq!(d.mode, "update");
    }

    #[test]
    fn create_inferred_without_command() {
        let text = "maid:「始」小克「末」,\ntool_name:「始」DailyNote「末」,\nContent:「始」正文「末」";
        let d = parse_daily_note_tool(text).expect("content implies create");
        assert_eq!(d.mode, "create");
        assert_eq!(d.content, "正文");
    }

    #[test]
    fn plain_key_value_compat() {
        let text = "tool_name: DailyNote,\ncommand: create,\nMaid: 小克\nDate: 2026.6.11\nContent:「始」正文「末」";
        let d = parse_daily_note_tool(text).expect("plain keys should parse");
        assert_eq!(d.mode, "create");
        assert_eq!(d.agent_name, "小克");
        assert_eq!(d.date, "2026.6.11");
    }

    #[test]
    fn escape_markers_do_not_stop_at_plain_end() {
        let text = "tool_name:「始」DailyNote「末」,\ncommand:「始」create「末」,\nContent:「始exp」正文里有「末」假标记「末exp」";
        let d = parse_daily_note_tool(text).expect("escape-marked content");
        assert_eq!(d.content, "正文里有「末」假标记");
    }

    #[test]
    fn unterminated_content_takes_rest() {
        let text = "tool_name:「始」DailyNote「末」,\ncommand:「始」create「末」,\nContent:「始」流式未闭合正文";
        let d = parse_daily_note_tool(text).expect("unterminated content tolerated");
        assert_eq!(d.content, "流式未闭合正文");
    }

    #[test]
    fn non_daily_note_tool_returns_none() {
        let text = "tool_name:「始」SciCalculator「末」,\nexpression:「始」1+1「末」";
        assert!(parse_daily_note_tool(text).is_none());
    }

    #[test]
    fn daily_note_without_create_or_update_returns_none() {
        let text = "tool_name:「始」DailyNote「末」,\ncommand:「始」delete「末」";
        assert!(parse_daily_note_tool(text).is_none());
    }

    #[test]
    fn command_update_does_not_leak_into_date() {
        // `update` 一词内含 `date`，\b 边界保证 Date 字段不被误读
        let text = "tool_name:「始」DailyNote「末」,\ncommand:「始」update「末」,\ntarget:「始」A「末」,\nreplace:「始」B「末」";
        let d = parse_daily_note_tool(text).expect("update detected");
        assert_eq!(d.date, "");
    }

    #[test]
    fn legacy_block_plain_fields() {
        let text = "Maid: 小克\nDate: 2026.6.11\nContent: 今天的日记正文\n第二行";
        let d = parse_daily_note_legacy(text);
        assert_eq!(d.mode, "legacy");
        assert_eq!(d.agent_name, "小克");
        assert_eq!(d.date, "2026.6.11");
        assert!(d.content.starts_with("今天的日记正文"));
        assert!(d.content.contains("第二行"));
    }

    #[test]
    fn legacy_block_without_content_label_uses_full_text() {
        let text = "只有一段没有任何字段标签的正文";
        let d = parse_daily_note_legacy(text);
        assert_eq!(d.content, text);
        assert_eq!(d.agent_name, "");
    }
}
