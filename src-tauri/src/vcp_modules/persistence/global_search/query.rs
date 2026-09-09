use sha2::{Digest, Sha256};

use super::{cursor, FtsSearchFilter};

pub(crate) const MAX_QUERY_CHARS: usize = 128;
pub(crate) const MAX_QUERY_BYTES: usize = 512;
pub(crate) const DEFAULT_LIMIT: i64 = 50;
pub(crate) const MAX_LIMIT: i64 = 100;
pub(crate) const MIN_TRIGRAM_CHARS: usize = 3;
const MAX_FILTER_VALUE_CHARS: usize = 256;
const MAX_FILTER_VALUE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SortMode {
    Time,
    Rank,
}

impl SortMode {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("time") {
            "time" | "timestamp" => Ok(Self::Time),
            "rank" => Ok(Self::Rank),
            other => Err(format!("不支持的搜索排序方式: {other}")),
        }
    }
}

#[derive(Debug)]
pub(crate) struct SearchPlan {
    pub(crate) normalized_query: String,
    pub(crate) long_terms: Vec<String>,
    pub(crate) short_terms: Vec<String>,
    pub(crate) sort: SortMode,
    pub(crate) limit: i64,
    pub(crate) fingerprint: String,
    pub(crate) cursor: Option<cursor::CursorPayload>,
}

impl SearchPlan {
    pub(crate) fn from_filter(filter: &FtsSearchFilter) -> Result<Self, String> {
        validate_query(&filter.query)?;
        validate_filters(filter)?;
        let normalized_query = filter.query.trim().to_string();
        let (long_terms, short_terms) = split_terms(&normalized_query);
        let requested_sort = SortMode::parse(filter.sort.as_deref())?;
        let sort = requested_sort.fallback_for_short_query(&long_terms);
        let limit = validate_limit(filter.limit)?;
        let fingerprint = fingerprint(filter, &normalized_query, requested_sort);
        let cursor = filter
            .cursor
            .as_deref()
            .map(cursor::decode_cursor)
            .transpose()?;
        validate_cursor(
            cursor.as_ref(),
            &fingerprint,
            sort,
            long_terms.is_empty() && short_terms.is_empty(),
        )?;

        Ok(Self {
            normalized_query,
            long_terms,
            short_terms,
            sort,
            limit,
            fingerprint,
            cursor,
        })
    }

    pub(crate) fn has_terms(&self) -> bool {
        !self.long_terms.is_empty() || !self.short_terms.is_empty()
    }

    pub(crate) fn fts_match_query(&self) -> Option<String> {
        if self.long_terms.is_empty() {
            None
        } else {
            Some(
                self.long_terms
                    .iter()
                    .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" AND "),
            )
        }
    }
}

impl SortMode {
    fn fallback_for_short_query(self, long_terms: &[String]) -> Self {
        if matches!(self, Self::Rank) && long_terms.is_empty() {
            Self::Time
        } else {
            self
        }
    }
}

fn validate_query(query: &str) -> Result<(), String> {
    if query.len() > MAX_QUERY_BYTES || query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!(
            "搜索关键词超过 {} 个 Unicode 字符或 {} 字节",
            MAX_QUERY_CHARS, MAX_QUERY_BYTES
        ));
    }
    if query
        .chars()
        .any(|character| character == '\0' || character.is_control())
    {
        return Err("搜索关键词含有禁止的控制字符".to_string());
    }
    Ok(())
}

fn validate_filters(filter: &FtsSearchFilter) -> Result<(), String> {
    if filter.owner_id.is_some() && filter.owner_type.is_none() {
        return Err("搜索 ownerId 必须同时提供 ownerType".to_string());
    }
    if filter.topic_id.is_some() && filter.owner_id.is_none() {
        return Err("搜索 topicId 必须同时提供完整归属身份".to_string());
    }
    if filter
        .owner_type
        .as_deref()
        .is_some_and(|owner_type| !matches!(owner_type, "agent" | "group"))
    {
        return Err("搜索 ownerType 只能是 agent 或 group".to_string());
    }
    validate_filter_value("ownerId", filter.owner_id.as_deref())?;
    validate_filter_value("ownerType", filter.owner_type.as_deref())?;
    validate_filter_value("topicId", filter.topic_id.as_deref())?;
    validate_filter_value("speakerAgentId", filter.speaker_agent_id.as_deref())?;
    validate_filter_value("role", filter.role.as_deref())?;
    validate_time_range(filter.start_time, filter.end_time)?;
    Ok(())
}

fn validate_limit(limit: Option<i64>) -> Result<i64, String> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    (1..=MAX_LIMIT)
        .contains(&limit)
        .then_some(limit)
        .ok_or_else(|| format!("搜索分页条数必须在 1 到 {MAX_LIMIT} 之间"))
}

fn validate_time_range(start_time: Option<i64>, end_time: Option<i64>) -> Result<(), String> {
    if start_time.is_some_and(|value| value < 0) || end_time.is_some_and(|value| value < 0) {
        return Err("搜索时间范围不能为负数".to_string());
    }
    if matches!((start_time, end_time), (Some(start), Some(end)) if start > end) {
        return Err("搜索起始时间不能晚于结束时间".to_string());
    }
    Ok(())
}

fn validate_filter_value(field: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() {
        return Err(format!("搜索筛选字段 {field} 不能为空"));
    }
    if value.len() > MAX_FILTER_VALUE_BYTES || value.chars().count() > MAX_FILTER_VALUE_CHARS {
        return Err(format!(
            "搜索筛选字段 {field} 超过 {} 个 Unicode 字符或 {} 字节",
            MAX_FILTER_VALUE_CHARS, MAX_FILTER_VALUE_BYTES
        ));
    }
    if value
        .chars()
        .any(|character| character == '\0' || character.is_control())
    {
        return Err(format!("搜索筛选字段 {field} 含有禁止的控制字符"));
    }
    Ok(())
}

fn split_terms(query: &str) -> (Vec<String>, Vec<String>) {
    let mut long_terms = Vec::new();
    let mut short_terms = Vec::new();
    for term in query.split_whitespace().filter(|term| !term.is_empty()) {
        if term.chars().count() >= MIN_TRIGRAM_CHARS {
            long_terms.push(term.to_string());
        } else {
            short_terms.push(term.to_string());
        }
    }
    (long_terms, short_terms)
}

fn fingerprint(filter: &FtsSearchFilter, normalized_query: &str, sort: SortMode) -> String {
    let input = serde_json::json!({
        "query": normalized_query,
        "topicId": filter.topic_id,
        "ownerId": filter.owner_id,
        "ownerType": filter.owner_type,
        "speakerAgentId": filter.speaker_agent_id,
        "role": filter.role,
        "startTime": filter.start_time,
        "endTime": filter.end_time,
        "sort": cursor::sort_name(sort),
    });
    hex::encode(Sha256::digest(
        serde_json::to_vec(&input).unwrap_or_default(),
    ))
}

fn validate_cursor(
    payload: Option<&cursor::CursorPayload>,
    fingerprint: &str,
    sort: SortMode,
    empty_query: bool,
) -> Result<(), String> {
    let Some(payload) = payload else {
        return Ok(());
    };
    if empty_query
        || payload.fingerprint != fingerprint
        || payload.sort != cursor::sort_name(sort)
        || payload.owner_type.is_empty()
        || payload.owner_id.is_empty()
        || payload.topic_id.is_empty()
        || payload.msg_id.is_empty()
    {
        return Err("搜索游标与当前查询不匹配".to_string());
    }
    match sort {
        SortMode::Time if payload.rank_bits.is_some() => {
            Err("搜索游标与时间排序不匹配".to_string())
        }
        SortMode::Rank if payload.rank_bits.is_none() => {
            Err("相关度搜索游标缺少排序值".to_string())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(query: &str) -> FtsSearchFilter {
        FtsSearchFilter {
            query: query.to_string(),
            topic_id: None,
            owner_id: None,
            owner_type: None,
            speaker_agent_id: None,
            role: None,
            start_time: None,
            end_time: None,
            limit: None,
            sort: None,
            cursor: None,
        }
    }

    #[test]
    fn 关键词按统一码标量数量分流并转义全文检索引号() {
        let short = SearchPlan::from_filter(&filter("😀\u{301}")).expect("短关键词应有效");
        assert!(short.long_terms.is_empty());
        assert_eq!(short.short_terms, vec!["😀\u{301}"]);

        let mixed = SearchPlan::from_filter(&filter("二字 long\"term")).expect("混合关键词应有效");
        assert_eq!(mixed.short_terms, vec!["二字"]);
        assert_eq!(mixed.fts_match_query().as_deref(), Some("\"long\"\"term\""));
    }

    #[test]
    fn 拒绝控制字符和超长关键词但接受首尾空白() {
        assert!(SearchPlan::from_filter(&filter("  long term  ")).is_ok());
        assert!(SearchPlan::from_filter(&filter("bad\0term")).is_err());
        assert!(SearchPlan::from_filter(&filter("bad\u{001f}term")).is_err());
        assert!(SearchPlan::from_filter(&filter(&"a".repeat(129))).is_err());
        assert!(SearchPlan::from_filter(&filter(&"😀".repeat(129))).is_err());
    }

    #[test]
    fn 归属筛选区分归属类型和完整话题身份() {
        let mut request = filter("needle");
        request.owner_type = Some("agent".to_string());
        assert!(SearchPlan::from_filter(&request).is_ok());
        request.topic_id = Some("topic".to_string());
        assert!(SearchPlan::from_filter(&request).is_err());
        request.owner_id = Some("owner".to_string());
        assert!(SearchPlan::from_filter(&request).is_ok());
        request.owner_type = None;
        assert!(SearchPlan::from_filter(&request).is_err());
    }

    #[test]
    fn 短词相关度排序退化为稳定时间排序() {
        let mut request = filter("二字");
        request.sort = Some("rank".to_string());
        let plan = SearchPlan::from_filter(&request).expect("短词相关度查询应有效");
        assert_eq!(plan.sort, SortMode::Time);
    }

    #[test]
    fn 用户筛选字段受长度和控制字符限制() {
        let mut request = filter("needle");
        request.owner_type = Some("agent".to_string());
        request.owner_id = Some("a".repeat(257));
        assert!(SearchPlan::from_filter(&request).is_err());
        request.owner_id = Some("owner\0id".to_string());
        assert!(SearchPlan::from_filter(&request).is_err());
        request.owner_id = Some("owner".to_string());
        request.topic_id = Some("topic".to_string());
        request.role = Some("assistant".to_string());
        request.speaker_agent_id = Some("speaker".to_string());
        assert!(SearchPlan::from_filter(&request).is_ok());
    }

    #[test]
    fn 分页条数和时间范围拒绝越界值() {
        let mut request = filter("needle");
        request.limit = Some(0);
        assert!(SearchPlan::from_filter(&request).is_err());
        request.limit = Some(MAX_LIMIT + 1);
        assert!(SearchPlan::from_filter(&request).is_err());
        request.limit = Some(DEFAULT_LIMIT);
        request.start_time = Some(2);
        request.end_time = Some(1);
        assert!(SearchPlan::from_filter(&request).is_err());
        request.start_time = Some(-1);
        request.end_time = None;
        assert!(SearchPlan::from_filter(&request).is_err());
    }
}
