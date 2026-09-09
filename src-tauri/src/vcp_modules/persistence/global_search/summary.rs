pub(crate) const SUMMARY_LIMIT: usize = 240;

pub(crate) fn summarize(content: &str, terms: &[String]) -> String {
    let plain = strip_markup_and_controls(content);
    let total_chars = plain.chars().count();
    if total_chars <= SUMMARY_LIMIT {
        return plain;
    }

    let first_hit = first_case_insensitive_hit(&plain, terms);
    let center = first_hit.unwrap_or(0);
    let has_prefix = center > 0;
    let has_suffix = center + SUMMARY_LIMIT < total_chars;
    let marker_count = usize::from(has_prefix) + usize::from(has_suffix);
    let window_size = SUMMARY_LIMIT.saturating_sub(marker_count);
    let mut start = center.saturating_sub(window_size / 2);
    if start + window_size > total_chars {
        start = total_chars.saturating_sub(window_size);
    }
    let end = start + window_size;
    let window: String = plain.chars().skip(start).take(window_size).collect();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        window,
        if end < total_chars { "…" } else { "" }
    )
}

fn first_case_insensitive_hit(content: &str, terms: &[String]) -> Option<usize> {
    let mut folded_content = String::with_capacity(content.len());
    let mut original_char_positions = Vec::with_capacity(content.chars().count());
    for (original_char_position, character) in content.chars().enumerate() {
        for folded_character in character.to_lowercase() {
            folded_content.push(folded_character);
            original_char_positions.push(original_char_position);
        }
    }

    terms
        .iter()
        .filter(|term| !term.is_empty())
        .filter_map(|term| {
            let folded_term: String = term.chars().flat_map(char::to_lowercase).collect();
            let byte_offset = folded_content.find(&folded_term)?;
            let folded_char_position = folded_content[..byte_offset].chars().count();
            original_char_positions.get(folded_char_position).copied()
        })
        .min()
}

fn strip_markup_and_controls(content: &str) -> String {
    let mut output = String::with_capacity(content.len().min(SUMMARY_LIMIT * 4));
    let mut cursor = 0;

    while cursor < content.len() {
        let remaining = &content[cursor..];
        let character = remaining
            .chars()
            .next()
            .expect("cursor must stay on a character boundary");
        if character != '<' {
            push_visible_character(&mut output, character);
            cursor += character.len_utf8();
            continue;
        }

        match complete_markup_fragment_len(remaining) {
            MarkupScan::Complete(fragment_len) => {
                let fragment = &remaining[..fragment_len];
                if !crate::vcp_modules::chat::pre_renderer::markdown_parser::is_supported_message_html(
                    fragment,
                ) {
                    push_visible_text(&mut output, fragment);
                }
                cursor += fragment_len;
            }
            MarkupScan::Interrupted => {
                output.push('<');
                cursor += 1;
            }
            MarkupScan::Unclosed => {
                push_visible_text(&mut output, remaining);
                break;
            }
        }
    }

    output
}

enum MarkupScan {
    Complete(usize),
    Interrupted,
    Unclosed,
}

fn complete_markup_fragment_len(content: &str) -> MarkupScan {
    if content.starts_with("<!--") {
        return content
            .find("-->")
            .map(|end| MarkupScan::Complete(end + 3))
            .unwrap_or(MarkupScan::Unclosed);
    }

    let mut quote = None;
    for (index, character) in content.char_indices().skip(1) {
        match (quote, character) {
            (Some(expected), actual) if actual == expected => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '>') => return MarkupScan::Complete(index + 1),
            (None, '<') => return MarkupScan::Interrupted,
            _ => {}
        }
    }

    MarkupScan::Unclosed
}

fn push_visible_text(output: &mut String, content: &str) {
    for character in content.chars() {
        push_visible_character(output, character);
    }
}

fn push_visible_character(output: &mut String, character: char) {
    if character.is_control() {
        if matches!(character, '\n' | '\r' | '\t') {
            output.push(' ');
        }
    } else {
        output.push(character);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 摘要保持纯文本并限制统一码字符数量() {
        let content = format!("prefix <mark>needle</mark> {}", "中".repeat(400));
        let summary = summarize(&content, &["needle".to_string()]);
        assert!(summary.chars().count() <= SUMMARY_LIMIT);
        assert!(!summary.contains('<'));
        assert!(!summary.contains('>'));
        assert!(summary.contains("needle"));
    }

    #[test]
    fn 摘要按不区分大小写的命中位置居中() {
        let content = format!("{}Apple{}", "前".repeat(300), "后".repeat(300));
        let summary = summarize(&content, &["apple".to_string()]);

        assert!(summary.chars().count() <= SUMMARY_LIMIT);
        assert!(summary.contains("Apple"));
        assert!(summary.starts_with('…'));
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn 摘要保留未闭合的尖括号文本() {
        assert_eq!(strip_markup_and_controls("use <variable"), "use <variable");
        assert_eq!(
            strip_markup_and_controls("prefix <mark needle"),
            "prefix <mark needle"
        );
    }

    #[test]
    fn 摘要保留泛型与不受支持的标签() {
        let content = "Vec<T> and Result<T, E> <reason>visible</reason>";

        assert_eq!(strip_markup_and_controls(content), content);
    }

    #[test]
    fn 摘要只移除完整且受支持的标签() {
        let content =
            "<mark>needle</mark><kbd title=\"1 > 0\">Ctrl</kbd><br>tail<!-- hidden > text -->";

        assert_eq!(strip_markup_and_controls(content), "needleCtrltail");
    }

    #[test]
    fn 摘要在畸形片段后仍能识别后续支持标签() {
        let content = "use <variable then <mark>needle</mark>";

        assert_eq!(
            strip_markup_and_controls(content),
            "use <variable then needle"
        );
    }

    #[test]
    fn 摘要继续规范化控制字符() {
        assert_eq!(
            strip_markup_and_controls("line\nnext\ttab\rreturn\u{0000}tail"),
            "line next tab returntail"
        );
    }
}
