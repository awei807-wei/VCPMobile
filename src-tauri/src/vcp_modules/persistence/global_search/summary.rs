pub(crate) const SUMMARY_LIMIT: usize = 240;

pub(crate) fn summarize(content: &str, terms: &[String]) -> String {
    let plain = strip_markup_and_controls(content);
    let total_chars = plain.chars().count();
    if total_chars <= SUMMARY_LIMIT {
        return plain;
    }

    let first_hit = terms
        .iter()
        .filter(|term| !term.is_empty())
        .filter_map(|term| plain.find(term))
        .min()
        .map(|byte_offset| plain[..byte_offset].chars().count());
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
}
