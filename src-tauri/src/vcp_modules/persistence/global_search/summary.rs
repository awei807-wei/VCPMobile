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
    let mut in_tag = false;
    let mut tag_candidate = false;
    for character in content.chars() {
        if in_tag {
            if character == '>' {
                in_tag = false;
                tag_candidate = false;
            }
            continue;
        }
        if character == '<' {
            tag_candidate = true;
            continue;
        }
        if tag_candidate {
            if character.is_ascii_alphabetic() || matches!(character, '/' | '!' | '?' | ':') {
                in_tag = true;
                tag_candidate = false;
                continue;
            }
            output.push('<');
            tag_candidate = false;
        }
        if character.is_control() {
            if matches!(character, '\n' | '\r' | '\t') {
                output.push(' ');
            }
        } else {
            output.push(character);
        }
    }
    if tag_candidate {
        output.push('<');
    }
    output
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
}
