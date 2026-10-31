use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

use crate::vcp_modules::chat::daily_note::DailyNoteDetails;
use crate::vcp_modules::pre_renderer::MarkdownNode;

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "markdown")]
    Markdown {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "tool-use")]
    ToolUse {
        tool_name: String,
        content: String,
        is_complete: bool,
        /// 是否 JEV（自然语言）工具请求，展示为 JEVToolUse 卡片。
        #[serde(default)]
        is_jev: bool,
        /// JEV 卡片上的展示名（引号内工具名优先，其次 {能力}）。
        #[serde(default)]
        jev_display_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "tool-result")]
    ToolResult {
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
        /// 是否已被用户标记为“不注入上下文”。
        #[serde(default)]
        hidden: bool,
        /// 原始正文的 FNV-1a 哈希，供前端在原文中精确定位该块。
        #[serde(default)]
        raw_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "diary")]
    Diary {
        #[serde(flatten)]
        fields: Box<DiaryContentBlock>,
    },
    #[serde(rename = "thought")]
    Thought {
        theme: String,
        content: String,
        is_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        nodes: Option<Vec<MarkdownNode>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "button-click")]
    ButtonClick {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "html-preview")]
    HtmlPreview {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        highlighted_content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "role-divider")]
    RoleDivider {
        role: String,
        is_end: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "style")]
    Style {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
    #[serde(rename = "tool-call-summary")]
    ToolCallSummary {
        items: Vec<ToolCallSummaryItem>,
        raw_content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct DiaryContentBlock {
    pub maid: String,
    pub date: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<MarkdownNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_nodes: Option<Vec<MarkdownNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_nodes: Option<Vec<MarkdownNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct ToolCallSummaryItem {
    pub tool_name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct ToolResultDetail {
    pub key: String,
    pub value: String,
}

impl ContentBlock {
    pub fn markdown(content: Option<String>, nodes: Option<Vec<MarkdownNode>>) -> Self {
        Self::Markdown {
            content,
            nodes,
            hash: None,
        }
    }

    pub fn tool_use(
        tool_name: String,
        content: String,
        is_complete: bool,
        jev: Option<crate::vcp_modules::chat::jev_tool_use::JevToolUse>,
    ) -> Self {
        let (is_jev, jev_display_name) = match jev {
            Some(jev) => (true, jev.display_name),
            None => (false, String::new()),
        };
        Self::ToolUse {
            tool_name,
            content,
            is_complete,
            is_jev,
            jev_display_name,
            hash: None,
        }
    }

    pub fn tool_result(
        tool_name: String,
        status: String,
        details: Vec<ToolResultDetail>,
        footer: String,
        hidden: bool,
        raw_hash: String,
    ) -> Self {
        Self::ToolResult {
            tool_name,
            status,
            details,
            footer,
            hidden,
            raw_hash,
            hash: None,
        }
    }

    pub(crate) fn daily_note(details: DailyNoteDetails) -> Self {
        let nodes = if details.content.trim().is_empty() {
            None
        } else {
            Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                &details.content,
            ))
        };
        let target_nodes = details
            .target
            .as_ref()
            .filter(|v| !v.trim().is_empty())
            .map(|v| crate::vcp_modules::pre_renderer::parse_markdown_to_ast(v));
        let replace_nodes = details
            .replace
            .as_ref()
            .filter(|v| !v.trim().is_empty())
            .map(|v| crate::vcp_modules::pre_renderer::parse_markdown_to_ast(v));

        Self::Diary {
            fields: Box::new(DiaryContentBlock {
                maid: details.agent_name,
                date: details.date,
                content: details.content,
                nodes,
                mode: Some(details.mode),
                agent_type: Some(details.agent_type),
                agent_label: Some(details.agent_label),
                file_name: details.file_name,
                folder: details.folder,
                tag: details.tag,
                target: details.target,
                replace: details.replace,
                target_nodes,
                replace_nodes,
                hash: None,
            }),
        }
    }

    pub fn thought(
        theme: String,
        content: String,
        is_complete: bool,
        nodes: Option<Vec<MarkdownNode>>,
    ) -> Self {
        Self::Thought {
            theme,
            content,
            is_complete,
            nodes,
            hash: None,
        }
    }

    #[allow(dead_code)]
    pub fn button_click(content: String) -> Self {
        Self::ButtonClick {
            content,
            hash: None,
        }
    }

    pub fn html_preview(content: String) -> Self {
        // 在流结束后沉淀或全量重新编译时，调用专属 HTML classed 高亮预渲染，生成不含 style 的 DOM
        let highlighted_content =
            crate::vcp_modules::chat::pre_renderer::code_highlighter::highlight_html_block(
                &content,
            );
        Self::HtmlPreview {
            content,
            highlighted_content,
            hash: None,
        }
    }

    pub fn role_divider(role: String, is_end: bool) -> Self {
        Self::RoleDivider {
            role,
            is_end,
            hash: None,
        }
    }

    pub fn style(content: String) -> Self {
        Self::Style {
            content,
            hash: None,
        }
    }

    pub fn tool_call_summary(items: Vec<ToolCallSummaryItem>, raw_content: String) -> Self {
        Self::ToolCallSummary {
            items,
            raw_content,
            hash: None,
        }
    }

    pub fn compute_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    pub fn set_hash(&mut self, h: u64) {
        match self {
            ContentBlock::Markdown { hash, .. } => *hash = Some(h),
            ContentBlock::ToolUse { hash, .. } => *hash = Some(h),
            ContentBlock::ToolResult { hash, .. } => *hash = Some(h),
            ContentBlock::Diary { fields } => fields.hash = Some(h),
            ContentBlock::Thought { hash, .. } => *hash = Some(h),
            ContentBlock::ButtonClick { hash, .. } => *hash = Some(h),
            ContentBlock::HtmlPreview { hash, .. } => *hash = Some(h),
            ContentBlock::RoleDivider { hash, .. } => *hash = Some(h),
            ContentBlock::Style { hash, .. } => *hash = Some(h),
            ContentBlock::ToolCallSummary { hash, .. } => *hash = Some(h),
        }
    }

    pub fn compute_hashes_recursively(&mut self) {
        let h = self.compute_hash();
        self.set_hash(h);
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum BlockType {
    Tool,
    Thought,
    Think,
    ToolResult,
    Diary,
    HtmlFence,
    HtmlDoc,
    HtmlContainer,
    Style,
    RoleDivider,
    CodeFence,
    ToolCallSummary,
}

lazy_static! {
    // 核心修复：为所有 VCP 块的起始标记强制增加行首锚定符 `(?im)^[ \t]*`
    // 这将彻底消除因正文提及 `<<<[TOOL_REQUEST]>>>` 等内联代码而引发的 AST 错误截断
    pub(crate) static ref TOOL_START: Regex = Regex::new(r"(?im)^[ \t]*<<<\[TOOL_REQUEST\]>>>").unwrap();
    pub(crate) static ref TOOL_END: Regex = Regex::new(r"(?im)^[ \t]*<<<\[END_TOOL_REQUEST\]>>>").unwrap();
    pub(crate) static ref TOOL_NAME: Regex = Regex::new(r"<tool_name>([\s\S]*?)</tool_name>|tool_name:\s*「始(?:exp)?」([^「」]*)「末(?:exp)?」").unwrap();

    pub(crate) static ref THOUGHT_START: Regex = Regex::new(r"(?im)^[ \t]*\[--- VCP元思考链(?::\s*([^\]]*?))?\s*---\]").unwrap();
    pub(crate) static ref THOUGHT_END: Regex = Regex::new(r"(?im)^[ \t]*\[--- 元思考链结束 ---\]").unwrap();

    pub(crate) static ref THINK_START: Regex = Regex::new(r"(?i)<think(?:ing)?>").unwrap();
    pub(crate) static ref THINK_END: Regex = Regex::new(r"(?i)</think(?:ing)?>").unwrap();

    pub(crate) static ref TOOL_RESULT_START: Regex = Regex::new(r"(?im)^[ \t]*\[\[VCP调用结果信息汇总:").unwrap();
    pub(crate) static ref TOOL_RESULT_END: Regex = Regex::new(r"(?im)^[ \t]*VCP调用结果结束\]\]").unwrap();

    pub(crate) static ref DIARY_START: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteStart>>>").unwrap();
    pub(crate) static ref DIARY_END: Regex = Regex::new(r"(?im)^[ \t]*<<<DailyNoteEnd>>>").unwrap();

    pub(crate) static ref BUTTON_CLICK: Regex = Regex::new(r"\[\[点击按钮:(.*?)\]\]").unwrap();

    pub(crate) static ref KV_REGEX: Regex = Regex::new(r"^-\s*([^:]+):\s*(.*)").unwrap();

    pub(crate) static ref HTML_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*`{3,}html[ \t]*\r?$").unwrap();
    pub(crate) static ref HTML_FENCE_END: Regex = Regex::new(r"(?im)^[ \t]*```[ \t]*\r?$").unwrap();

    // 修复：强行增加行首锚定符 ^，防止正文中的内联 `<!DOCTYPE html>` 触发解析截断
    pub(crate) static ref HTML_DOC_START: Regex = Regex::new(r"(?im)^[ \t]*(?:<!doctype html>|<html[\s>])").unwrap();
    pub(crate) static ref HTML_DOC_END: Regex = Regex::new(r"(?i)</html>").unwrap();

    pub(crate) static ref HTML_CONTAINER_OPEN_RE: Regex =
        Regex::new(r"(?im)^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>").unwrap();

    pub(crate) static ref ROLE_DIVIDER: Regex = Regex::new(r"(?im)^[ \t]*<<<\[(END_)?ROLE_DIVIDE_(SYSTEM|ASSISTANT|USER)\]>>>").unwrap();
    pub(crate) static ref STYLE_TAG_START: Regex = Regex::new(r"(?im)^[ \t]*<style\b[^>]*>?").unwrap();
    pub(crate) static ref STYLE_TAG_END: Regex = Regex::new(r"(?i)</style>").unwrap();
    pub(crate) static ref TOOL_CALL_SUMMARY_START: Regex = Regex::new(r"(?im)^[ \t]*\[本轮工具调用摘要:\]").unwrap();
    pub(crate) static ref TOOL_CALL_SUMMARY_END: Regex = Regex::new(r"(?im)^[ \t]*\[本轮工具调用摘要结束\]").unwrap();

    pub(crate) static ref HTML_TAG_BLOCK_RE: Regex =
        Regex::new(r"(?im)^[ \t]*<(?:style\b[^>]*>?|html[\s>]|!doctype\s+html|/?(?:div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>)").unwrap();

    pub(crate) static ref GENERIC_CODE_FENCE_START: Regex = Regex::new(r"(?im)^[ \t]*`{3,}[a-zA-Z0-9-]*[ \t]*\r?$").unwrap();
    pub(crate) static ref GENERIC_CODE_FENCE_END: Regex = Regex::new(r"(?im)^[ \t]*```[ \t]*\r?$").unwrap();


    static ref LIST_REGEX: Regex = Regex::new(r"^[ \t]*([-*]|\d+\.)[ \t]+").unwrap();
    static ref HTML_TAG_REGEX: Regex = Regex::new(r"(?i)^[ \t]*</?[a-zA-Z][a-zA-Z0-9]*[\s>/]").unwrap();
}

/// 检测字符是否为自然语言的起始字符（CJK / 日文 / 韩文 / 标点）。
///
/// 覆盖以下 Unicode 区块：
///   U+2E80..U+9FFF  CJK Radicals → Unified Ideographs（大部分东亚文字）
///   U+AC00..U+D7AF  Hangul Syllables（韩文）
///   U+F900..U+FAFF  CJK Compatibility Ideographs
///   U+FE30..U+FE4F  CJK Compatibility Forms
///   U+FF01..U+FF60  Fullwidth Forms（全角标点+字母）
///   U+FFE0..U+FFE6  Fullwidth Signs
///   若干常用 Curly Quotes / Em-Dash / Ellipsis
#[inline]
fn is_natural_language_line_start(c: char) -> bool {
    ('\u{2E80}'..='\u{9FFF}').contains(&c)
        || ('\u{AC00}'..='\u{D7AF}').contains(&c)
        || ('\u{F900}'..='\u{FAFF}').contains(&c)
        || ('\u{FE30}'..='\u{FE4F}').contains(&c)
        || ('\u{FF01}'..='\u{FF60}').contains(&c)
        || ('\u{FFE0}'..='\u{FFE6}').contains(&c)
        || matches!(
            c,
            '\u{201C}' | '\u{201D}' | // " "
            '\u{2018}' | '\u{2019}' | // ' '
            '\u{2026}' | // …
            '\u{2014}' // —
        )
}

#[inline]
fn is_vcp_marker(s: &str) -> bool {
    s.starts_with("<<<")
        || s.starts_with("[---")
        || (s.len() >= 5 && s.is_char_boundary(5) && s[..5].eq_ignore_ascii_case("[[vcp"))
        || (s.len() >= 6 && s.is_char_boundary(6) && s[..6].eq_ignore_ascii_case("<think"))
        || (s.len() >= 7 && s.is_char_boundary(7) && s[..7].eq_ignore_ascii_case("</think"))
}

pub fn de_indent_misinterpreted_code_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());

    // 预先检测所有代码围栏的行索引范围
    let lines: Vec<&str> = text.lines().collect();
    let num_lines = lines.len();
    let mut is_inside_fence = vec![false; num_lines];
    let mut temp_in_fence = false;

    for i in 0..num_lines {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("```") {
            temp_in_fence = !temp_in_fence;
            is_inside_fence[i] = true; // 围栏行本身也算作围栏内
        } else if temp_in_fence {
            is_inside_fence[i] = true;
        }
    }

    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }

        // 如果是代码围栏内部的行，绝对不进行任何去缩进清洗，原样保留
        if is_inside_fence[i] {
            result.push_str(line);
            continue;
        }

        let trimmed = line.trim_start();

        let has_indentation = line.len() > trimmed.len();
        if has_indentation {
            if LIST_REGEX.is_match(line) {
                result.push_str(line);
            } else if (trimmed.starts_with('<') && HTML_TAG_REGEX.is_match(trimmed))
                || trimmed
                    .chars()
                    .next()
                    .is_some_and(is_natural_language_line_start)
                || is_vcp_marker(trimmed)
                || trimmed.starts_with("<!--")
            {
                result.push_str(trimmed);
            } else {
                result.push_str(line);
            }
        } else {
            result.push_str(line);
        }
    }

    result
}

pub(crate) fn find_matching_fence_end(
    search_area: &str,
    start_marker_text: &str,
) -> (Option<usize>, Option<usize>, bool) {
    let trimmed = start_marker_text.trim_start();
    let fence_char = match trimmed.chars().next() {
        Some(c) if c == '`' => c,
        _ => return (None, None, false),
    };
    let count = trimmed.chars().take_while(|&c| c == fence_char).count();
    if count < 3 {
        return (None, None, false);
    }

    let regex_str = format!(r"(?m)^[ \t]{{0,3}}\`{{{},}}[ \t]*\r?$", count);

    if let Ok(re) = Regex::new(&regex_str) {
        if let Some(m) = re.find(search_area) {
            return (Some(m.start()), Some(m.end()), true);
        }
    }

    (None, None, false)
}

/// 核心解析函数：将原始 Markdown 文本解析为 AST 块数组
pub fn parse_content(raw_text: &str) -> Vec<ContentBlock> {
    let deindented_text = de_indent_misinterpreted_code_blocks(raw_text);
    let text = deindented_text.as_str();

    let mut blocks = Vec::new();
    let mut current_pos = 0;

    // 预编译主匹配正则（包含所有特种块起始标记，利用捕获组编号识别类型）
    // 1: TOOL, 2: THOUGHT, 3: THINK, 4: TOOL_RESULT, 5: DIARY, 6: HTML_FENCE, 7: HTML_DOC, 8: ROLE_DIVIDER, 9: STYLE, 10: CODE_FENCE, 11: HTML_CONTAINER
    lazy_static! {
        static ref MASTER_START: Regex = Regex::new(concat!(
            r"(?im)",
            r"(^[ \t]*<<<\[TOOL_REQUEST\]>>>)|",                       // 1
            r"(^[ \t]*\[--- VCP元思考链(?::\s*[^\]]*?)?\s*---\])|",    // 2
            r"(<think(?:ing)?>)|",                                     // 3
            r"(^[ \t]*\[\[VCP调用结果信息汇总:)|",                     // 4
            r"(^[ \t]*<<<DailyNoteStart>>>)|",                         // 5
            r"(^[ \t]*`{3,}html[ \t]*$)|",                            // 6
            r"(^[ \t]*(?:<!doctype html>|<html[\s>]))|",               // 7
            r"(^[ \t]*<<<\[(?:END_)?ROLE_DIVIDE_(?:SYSTEM|ASSISTANT|USER)\]>>>)|", // 8
            r"(^[ \t]*<style\b[^>]*>)|",                                      // 9
            r"(^[ \t]*`{3,}[a-zA-Z0-9-]*[ \t]*$)|",                    // 10
            r"(^[ \t]*<(div|section|article|header|footer|main|aside|figure|figcaption)\b[^>]*>)|", // 11
            r"(^[ \t]*\[本轮工具调用摘要:\])"                          // 13
        )).unwrap();
    }

    while current_pos < text.len() {
        let remaining = &text[current_pos..];

        if let Some(caps) = MASTER_START.captures(remaining) {
            let m = caps.get(0).unwrap();
            let start_idx = m.start();
            let end_idx = m.end();

            // 1. 将起始标记之前的文本作为 Markdown 块推入
            if start_idx > 0 {
                let md_text = &remaining[..start_idx];
                if md_text.contains("[[点击按钮:") {
                    blocks.extend(parse_inline_blocks(md_text));
                } else {
                    blocks.push(ContentBlock::markdown(
                        None,
                        Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                            md_text,
                        )),
                    ));
                }
            }

            // 识别匹配到的块类型
            let mut container_tag = String::new();
            let block_type = if caps.get(1).is_some() {
                BlockType::Tool
            } else if caps.get(2).is_some() {
                BlockType::Thought
            } else if caps.get(3).is_some() {
                BlockType::Think
            } else if caps.get(4).is_some() {
                BlockType::ToolResult
            } else if caps.get(5).is_some() {
                BlockType::Diary
            } else if caps.get(6).is_some() {
                BlockType::HtmlFence
            } else if caps.get(7).is_some() {
                BlockType::HtmlDoc
            } else if caps.get(8).is_some() {
                BlockType::RoleDivider
            } else if caps.get(9).is_some() {
                BlockType::Style
            } else if caps.get(10).is_some() {
                BlockType::CodeFence
            } else if caps.get(13).is_some() {
                BlockType::ToolCallSummary
            } else {
                container_tag = caps.get(12).unwrap().as_str().to_lowercase();
                BlockType::HtmlContainer
            };

            // 2. 寻找对应的结束标记
            let content_start = end_idx;
            let search_area = &remaining[content_start..];
            let start_marker_text = &remaining[start_idx..end_idx];

            let (end_marker_start, end_marker_end, is_complete) = match block_type {
                BlockType::Tool => TOOL_END.find(search_area).map_or((None, None, false), |m| {
                    (Some(m.start()), Some(m.end()), true)
                }),
                BlockType::Thought => THOUGHT_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::Think => THINK_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::ToolResult => TOOL_RESULT_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::Diary => DIARY_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::ToolCallSummary => TOOL_CALL_SUMMARY_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::HtmlFence => {
                    find_matching_fence_end(search_area, start_marker_text)
                }
                BlockType::HtmlDoc => HTML_DOC_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::HtmlContainer => crate::vcp_modules::chat::pre_renderer::markdown_parser::find_matching_close_tag(remaining, content_start, &container_tag)
                    .map_or((None, None, false), |(s, e)| {
                        (Some(s - content_start), Some(e - content_start), true)
                    }),
                BlockType::RoleDivider => (Some(0), Some(0), true),
                BlockType::Style => STYLE_TAG_END
                    .find(search_area)
                    .map_or((None, None, false), |m| {
                        (Some(m.start()), Some(m.end()), true)
                    }),
                BlockType::CodeFence => {
                    find_matching_fence_end(search_area, start_marker_text)
                }
            };

            // 容错处理：未闭合的块（流式中断）降级为普通 Markdown
            if !is_complete
                && !matches!(
                    block_type,
                    BlockType::HtmlFence
                        | BlockType::HtmlDoc
                        | BlockType::HtmlContainer
                        | BlockType::CodeFence
                        | BlockType::RoleDivider
                )
            {
                let marker_text = &remaining[start_idx..end_idx];
                blocks.push(ContentBlock::markdown(
                    None,
                    Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                        marker_text,
                    )),
                ));
                current_pos += end_idx;
                continue;
            }

            let inner_content = if let Some(end_start) = end_marker_start {
                &search_area[..end_start]
            } else {
                search_area
            };

            // 3. 解析具体的块内容
            let block = match block_type {
                BlockType::Tool => {
                    if let Some(details) =
                        crate::vcp_modules::chat::daily_note::parse_daily_note_tool(inner_content)
                    {
                        ContentBlock::daily_note(details)
                    } else {
                        let tool_name = extract_tool_name(inner_content);
                        // JEV 是工具请求里的自然语言字段，没有 tool_name，
                        // 识别出来后交给展示层渲染成 JEVToolUse 卡片。
                        let jev =
                            crate::vcp_modules::chat::jev_tool_use::detect_jev_use(inner_content);
                        ContentBlock::tool_use(
                            tool_name,
                            inner_content.to_string(),
                            is_complete,
                            jev,
                        )
                    }
                }
                BlockType::Thought => {
                    let start_marker_text = &remaining[start_idx..end_idx];
                    let theme = THOUGHT_START
                        .captures(start_marker_text)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().trim().replace("\"", ""))
                        .unwrap_or_else(|| "元思考链".to_string());

                    let nodes =
                        crate::vcp_modules::pre_renderer::parse_markdown_to_ast(inner_content);
                    ContentBlock::thought(
                        theme,
                        inner_content.to_string(),
                        is_complete,
                        Some(nodes),
                    )
                }
                BlockType::Think => {
                    let nodes =
                        crate::vcp_modules::pre_renderer::parse_markdown_to_ast(inner_content);
                    ContentBlock::thought(
                        "思维链".to_string(),
                        inner_content.to_string(),
                        is_complete,
                        Some(nodes),
                    )
                }
                BlockType::ToolResult => {
                    // 工具结果可以被用户手动标记为“隐藏”（不注入 AI 上下文）。
                    // 标记写在起始标记之后，这里先记录状态与原文哈希，再把标记剥掉，
                    // 保证工具字段解析与展示不受影响。
                    let hidden = crate::vcp_modules::chat::tool_result_visibility::is_tool_result_hidden(inner_content);
                    let raw_hash = crate::vcp_modules::chat::tool_result_visibility::tool_result_body_hash(inner_content);
                    let body = crate::vcp_modules::chat::tool_result_visibility::strip_hidden_marker(inner_content);
                    let (tool_name, status, details, footer) = parse_tool_result(&body);
                    ContentBlock::tool_result(tool_name, status, details, footer, hidden, raw_hash)
                }
                BlockType::Diary => ContentBlock::daily_note(
                    crate::vcp_modules::chat::daily_note::parse_daily_note_legacy(inner_content),
                ),
                BlockType::ToolCallSummary => {
                    let items = parse_tool_call_summary(inner_content);
                    ContentBlock::tool_call_summary(items, inner_content.to_string())
                }
                BlockType::HtmlFence => ContentBlock::html_preview(inner_content.to_string()),
                BlockType::HtmlDoc => {
                    let mut full_html = String::new();
                    full_html.push_str(&remaining[start_idx..end_idx]);
                    full_html.push_str(inner_content);
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                            full_html.push_str(&search_area[s..e]);
                        }
                    }
                    ContentBlock::html_preview(full_html)
                }
                BlockType::HtmlContainer => {
                    let open_tag = &remaining[start_idx..end_idx];
                    let deindented_inner = crate::vcp_modules::chat::pre_renderer::markdown_parser::trim_common_leading_indent(inner_content);
                    let mut nodes = vec![crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                        open_tag.to_string(),
                    )];
                    nodes.extend(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                        &deindented_inner,
                    ));
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                            let close_tag = &search_area[s..e];
                            nodes.push(crate::vcp_modules::pre_renderer::MarkdownNode::raw_html(
                                close_tag.to_string(),
                            ));
                        }
                    }
                    ContentBlock::markdown(None, Some(nodes))
                }
                BlockType::RoleDivider => {
                    let marker_text = &remaining[start_idx..end_idx];
                    if let Some(caps) = ROLE_DIVIDER.captures(marker_text) {
                        let is_end = caps.get(1).is_some();
                        let role = caps
                            .get(2)
                            .map(|m| m.as_str().to_lowercase())
                            .unwrap_or_default();
                        ContentBlock::role_divider(role, is_end)
                    } else {
                        ContentBlock::markdown(
                            None,
                            Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                                marker_text,
                            )),
                        )
                    }
                }
                BlockType::Style => ContentBlock::style(inner_content.to_string()),
                BlockType::CodeFence => {
                    let mut full_fence = String::new();
                    full_fence.push_str(&remaining[start_idx..end_idx]);
                    full_fence.push_str(inner_content);
                    if is_complete {
                        if let (Some(s), Some(e)) = (end_marker_start, end_marker_end) {
                            full_fence.push_str(&search_area[s..e]);
                        }
                    }
                    ContentBlock::markdown(
                        None,
                        Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                            &full_fence,
                        )),
                    )
                }
            };

            blocks.push(block);

            // 4. 更新游标
            if let Some(end_end) = end_marker_end {
                current_pos += content_start + end_end;
            } else {
                break;
            }
        } else {
            // 没有找到任何特种块，剩余部分全部作为 Markdown 处理
            if remaining.contains("[[点击按钮:") {
                blocks.extend(parse_inline_blocks(remaining));
            } else {
                blocks.push(ContentBlock::markdown(
                    None,
                    Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                        remaining,
                    )),
                ));
            }
            break;
        }
    }

    // 计算全量块的稳定哈希指纹
    for block in &mut blocks {
        block.compute_hashes_recursively();
    }

    blocks
}

/// 解析内联块（如按钮点击）
fn parse_inline_blocks(text: &str) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let mut last_end = 0;

    for cap in BUTTON_CLICK.captures_iter(text) {
        let Some(m) = cap.get(0) else { continue };
        let Some(button_content) = cap.get(1) else {
            continue;
        };
        if m.start() > last_end {
            blocks.push(ContentBlock::markdown(
                None,
                Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                    &text[last_end..m.start()],
                )),
            ));
        }
        blocks.push(ContentBlock::button_click(
            button_content.as_str().trim().to_string(),
        ));
        last_end = m.end();
    }

    if last_end < text.len() {
        blocks.push(ContentBlock::markdown(
            None,
            Some(crate::vcp_modules::pre_renderer::parse_markdown_to_ast(
                &text[last_end..],
            )),
        ));
    }

    blocks
}

fn extract_tool_name(content: &str) -> String {
    if let Some(caps) = TOOL_NAME.captures(content) {
        if let Some(m) = caps.get(1).or_else(|| caps.get(2)) {
            let s = m.as_str().trim();
            let mut name = if s.contains('「') {
                s.replace("「始」", "")
                    .replace("「末」", "")
                    .replace("「始exp」", "")
                    .replace("「末exp」", "")
            } else {
                s.to_string()
            };
            if name.ends_with(',') {
                name.pop();
            }
            return name.trim().to_string();
        }
    }
    "Processing...".to_string()
}

fn parse_tool_result(content: &str) -> (String, String, Vec<ToolResultDetail>, String) {
    let mut tool_name = "Unknown Tool".to_string();
    let mut status = "Unknown Status".to_string();
    let mut details = Vec::new();
    let mut footer = String::new();

    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let captures = if trimmed.starts_with('-') {
            KV_REGEX.captures(trimmed)
        } else {
            None
        };

        if let Some(captures) = captures {
            if let Some(key) = current_key.take() {
                let val = current_value.trim().to_string();
                if key == "工具名称" {
                    tool_name = val;
                } else if key == "执行状态" {
                    status = val;
                } else {
                    details.push(ToolResultDetail { key, value: val });
                }
            }
            if let (Some(key_match), Some(val_match)) = (captures.get(1), captures.get(2)) {
                current_key = Some(key_match.as_str().trim().to_string());
                current_value = val_match.as_str().trim().to_string();
            } else {
                current_value = String::new();
            }
        } else if current_key.is_some() {
            if !current_value.is_empty() {
                current_value.push('\n');
            }
            current_value.push_str(line);
        } else if !trimmed.is_empty() {
            if !footer.is_empty() {
                footer.push('\n');
            }
            footer.push_str(line);
        }
    }

    if let Some(key) = current_key {
        let val = current_value.trim().to_string();
        if key == "工具名称" {
            tool_name = val;
        } else if key == "执行状态" {
            status = val;
        } else {
            details.push(ToolResultDetail { key, value: val });
        }
    }

    (tool_name, status, details, footer)
}

pub(crate) fn parse_tool_call_summary(content: &str) -> Vec<ToolCallSummaryItem> {
    let mut items = Vec::new();
    for entry in content.split(['；', ';', '。']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let status = if entry.contains("拒绝")
            || entry.contains("被拒")
            || entry.contains("denied")
            || entry.contains("rejected")
            || entry.contains("refused")
        {
            "rejected"
        } else if entry.contains("失败")
            || entry.contains("错误")
            || entry.contains("异常")
            || entry.contains("error")
            || entry.contains("failed")
        {
            "failure"
        } else if entry.contains("超时") || entry.contains("timeout") {
            "timeout"
        } else if entry.contains("成功")
            || entry.contains("完成")
            || entry.contains("success")
            || entry.contains("succeeded")
            || entry.contains("ok")
        {
            "success"
        } else if entry.contains("取消") || entry.contains("中止") || entry.contains("cancel") {
            "cancelled"
        } else if entry.contains("跳过") || entry.contains("skip") {
            "skipped"
        } else {
            "unknown"
        };

        let tool_name = if let Some(idx) = entry.find("调用") {
            entry[..idx].trim().to_string()
        } else {
            entry.to_string()
        };

        items.push(ToolCallSummaryItem {
            tool_name,
            status: status.to_string(),
        });
    }
    items
}

pub fn is_html_tag_block(text: &str) -> bool {
    HTML_TAG_BLOCK_RE.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jev_tool_request_is_flagged_as_jev_card() {
        let raw = "<<<[TOOL_REQUEST]>>>\nJEV:「始」联络 'GPT生图'，画一张猫「末」\n<<<[END_TOOL_REQUEST]>>>";
        let blocks = parse_content(raw);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolUse {
                tool_name,
                is_jev,
                jev_display_name,
                ..
            } => {
                assert!(is_jev);
                assert_eq!(jev_display_name, "GPT生图");
                // JEV 没有 tool_name 字段，沿用兜底名
                assert_eq!(tool_name, "Processing...");
            }
            _ => panic!("Expected ToolUse block, got {:?}", blocks[0]),
        }
    }

    #[test]
    fn test_plain_tool_request_is_not_jev_card() {
        let raw = "<<<[TOOL_REQUEST]>>>\ntool_name:「始」SciCalculator「末」,\nexpression:「始」1+1「末」\n<<<[END_TOOL_REQUEST]>>>";
        let blocks = parse_content(raw);
        match &blocks[0] {
            ContentBlock::ToolUse {
                is_jev,
                jev_display_name,
                tool_name,
                ..
            } => {
                assert!(!is_jev);
                assert!(jev_display_name.is_empty());
                assert_eq!(tool_name, "SciCalculator");
            }
            _ => panic!("Expected ToolUse block, got {:?}", blocks[0]),
        }
    }

    #[test]
    fn test_tool_result_hidden_marker_is_parsed_and_stripped() {
        let raw = "[[VCP调用结果信息汇总: [HIDDEN]\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n- 返回内容: 结果\nVCP调用结果结束]]";
        let blocks = parse_content(raw);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult {
                tool_name,
                status,
                footer,
                hidden,
                raw_hash,
                ..
            } => {
                assert!(hidden);
                assert_eq!(tool_name, "WebSearch");
                assert_eq!(status, "✅ 成功");
                // 隐藏标记不能泄漏到展示字段里
                assert!(!footer.contains("[HIDDEN]"));
                // 哈希基于原文（包含标记），供前端精确回指
                assert_eq!(
                    raw_hash,
                    &crate::vcp_modules::chat::tool_result_visibility::tool_result_body_hash(
                        " [HIDDEN]\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n- 返回内容: 结果\n"
                    )
                );
            }
            _ => panic!("Expected ToolResult block, got {:?}", blocks[0]),
        }
    }

    #[test]
    fn test_tool_result_without_hidden_marker_is_visible() {
        let raw = "[[VCP调用结果信息汇总:\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n- 返回内容: 结果\nVCP调用结果结束]]";
        let blocks = parse_content(raw);
        match &blocks[0] {
            ContentBlock::ToolResult { hidden, .. } => assert!(!hidden),
            _ => panic!("Expected ToolResult block, got {:?}", blocks[0]),
        }
    }

    #[test]
    fn test_parse_content_style_blocks() {
        // 1. 正常的独立行 <style> 应该被正确解析为 Style 块
        let raw_style = "<style>\nbody { color: red; }\n</style>";
        let blocks = parse_content(raw_style);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Style { content, .. } => {
                assert_eq!(content.trim(), "body { color: red; }");
            }
            _ => panic!("Expected Style block, got {:?}", blocks[0]),
        }

        // 2. 行内代码包裹的 `<style>` 应该被保留在 Markdown 中，而不是被提取为 Style 块
        let raw_inline = "在 HTML 中，`<style>body {}</style>` 用于定义样式。";
        let blocks = parse_content(raw_inline);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Markdown { .. } => {}
            _ => panic!("Expected Markdown block, got {:?}", blocks[0]),
        }
    }

    #[test]
    fn test_pre_txt_16_parsing() {
        let text = "### 16. 代码块内包含围栏\n\n````markdown\n```python\n# This is code inside markdown inside code\nprint(\"nested\")\n```\n````";
        let blocks = parse_content(text);
        println!("BLOCKS: {:#?}", blocks);
        assert_eq!(blocks.len(), 2);

        // 第一个块应该是 Heading
        if let ContentBlock::Markdown { nodes, .. } = &blocks[0] {
            let nodes = nodes.as_ref().unwrap();
            assert_eq!(nodes.len(), 1);
            assert!(matches!(
                nodes[0],
                crate::vcp_modules::pre_renderer::MarkdownNode::Heading { .. }
            ));
        } else {
            panic!("Expected Heading block");
        }

        // 第二个块应该是 CodeBlock
        if let ContentBlock::Markdown { nodes, .. } = &blocks[1] {
            let nodes = nodes.as_ref().unwrap();
            assert_eq!(nodes.len(), 1);
            let has_nested_code = nodes.iter().any(|node| {
                if let crate::vcp_modules::pre_renderer::MarkdownNode::CodeBlock {
                    lang,
                    code,
                    ..
                } = node
                {
                    lang.as_deref() == Some("markdown") && code.contains("```python")
                } else {
                    false
                }
            });
            assert!(
                has_nested_code,
                "Expected to find a nested CodeBlock with lang=markdown and containing inner code"
            );
        } else {
            panic!(
                "Expected Markdown block with nested code, got {:?}",
                blocks[1]
            );
        }
    }
}
