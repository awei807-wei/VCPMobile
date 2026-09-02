use futures_util::TryStreamExt;
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

const SQLITE_BIND_CHUNK: usize = 400;
const MAX_PHASE3_MESSAGES_PER_TOPIC: usize = 10_000;
const MAX_PHASE3_MESSAGES: usize = 100_000;
const MAX_PHASE3_STATE_BYTES: usize = 64 * 1024 * 1024;

pub struct Phase3Message;

#[derive(Debug)]
pub struct TargetedTopicHashState {
    pub owner_type: String,
    pub owner_id: String,
    pub config_hash: String,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct TopicLocalState {
    pub owner_type: String,
    pub owner_id: String,
    pub topic_hash: String,
    pub messages: HashMap<String, String>,
}

#[derive(Default)]
struct Phase3StateBudget {
    messages: usize,
    bytes: usize,
}

impl Phase3StateBudget {
    fn observe_topic(
        &mut self,
        topic_id: &str,
        messages: usize,
        raw_bytes: usize,
    ) -> Result<(), String> {
        if messages > MAX_PHASE3_MESSAGES_PER_TOPIC {
            return Err(format!(
                "Phase 3 topic {topic_id} exceeds the {MAX_PHASE3_MESSAGES_PER_TOPIC}-message limit"
            ));
        }
        self.messages = self
            .messages
            .checked_add(messages)
            .ok_or_else(|| "Phase 3 message count overflow".to_string())?;
        if self.messages > MAX_PHASE3_MESSAGES {
            return Err(format!(
                "Phase 3 state exceeds the {MAX_PHASE3_MESSAGES}-message limit"
            ));
        }
        self.bytes = self
            .bytes
            .checked_add(raw_bytes)
            .ok_or_else(|| "Phase 3 state size overflow".to_string())?;
        if self.bytes > MAX_PHASE3_STATE_BYTES {
            return Err("Phase 3 state exceeds the 64 MiB memory budget".to_string());
        }
        Ok(())
    }
}

fn validate_unique_ids(ids: &[String], label: &str) -> Result<HashSet<String>, String> {
    let expected = ids.iter().cloned().collect::<HashSet<_>>();
    if expected.len() != ids.len() || expected.iter().any(|id| id.is_empty()) {
        return Err(format!("{label} contains empty or duplicate ids"));
    }
    Ok(expected)
}

fn decode_targeted_topic(
    row: sqlx::sqlite::SqliteRow,
    owners: &HashSet<String>,
) -> Result<Option<(String, TargetedTopicHashState)>, String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("Targeted topic id decode failed: {error}"))?;
    if topic_id == "default" {
        return Ok(None);
    }
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Targeted topic {topic_id} owner type decode failed: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Targeted topic {topic_id} owner id decode failed: {error}"))?;
    if !matches!(owner_type.as_str(), "agent" | "group") || !owners.contains(&owner_id) {
        return Err(format!(
            "Targeted topic {topic_id} has invalid owner identity"
        ));
    }
    let state = TargetedTopicHashState {
        owner_type,
        owner_id,
        config_hash: row.try_get("config_hash").map_err(|error| {
            format!("Targeted topic {topic_id} config hash decode failed: {error}")
        })?,
        content_hash: row.try_get("content_hash").map_err(|error| {
            format!("Targeted topic {topic_id} content hash decode failed: {error}")
        })?,
    };
    Ok(Some((topic_id, state)))
}

async fn load_targeted_topic_hashes(
    pool: &SqlitePool,
    owners: &[String],
    expected_owners: &HashSet<String>,
) -> Result<HashMap<String, TargetedTopicHashState>, String> {
    let mut result = HashMap::new();
    for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = owner_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT topic_id, owner_type, owner_id, config_hash, content_hash \
             FROM topics WHERE owner_id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let mut query = sqlx::query(&sql);
        for owner_id in owner_chunk {
            query = query.bind(owner_id);
        }
        for row in query
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?
        {
            let Some((topic_id, state)) = decode_targeted_topic(row, expected_owners)? else {
                continue;
            };
            if result.insert(topic_id.clone(), state).is_some() {
                return Err(format!(
                    "Targeted topic hash query returned duplicate topic {topic_id}"
                ));
            }
        }
    }
    Ok(result)
}

fn decode_topic_local(row: sqlx::sqlite::SqliteRow) -> Result<(String, TopicLocalState), String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("Topic hash id decode failed: {error}"))?;
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
    if !matches!(owner_type.as_str(), "agent" | "group") || owner_id.is_empty() {
        return Err(format!("Topic {topic_id} has invalid owner identity"));
    }
    Ok((
        topic_id.clone(),
        TopicLocalState {
            owner_type,
            owner_id,
            topic_hash: row
                .try_get("content_hash")
                .map_err(|error| format!("Topic {topic_id} content hash decode failed: {error}"))?,
            messages: HashMap::new(),
        },
    ))
}

async fn load_topic_states(
    pool: &SqlitePool,
    topic_ids: &[String],
) -> Result<HashMap<String, TopicLocalState>, String> {
    let mut result = HashMap::new();
    for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = topic_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT topic_id, owner_type, owner_id, content_hash \
             FROM topics WHERE topic_id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let mut query = sqlx::query(&sql);
        for id in topic_chunk {
            query = query.bind(id);
        }
        for row in query
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?
        {
            let (topic_id, state) = decode_topic_local(row)?;
            if result.insert(topic_id.clone(), state).is_some() {
                return Err(format!(
                    "Topic message hash query returned duplicate topic {topic_id}"
                ));
            }
        }
    }
    Ok(result)
}

fn ensure_topic_coverage(
    result: &HashMap<String, TopicLocalState>,
    expected: &HashSet<String>,
) -> Result<(), String> {
    let actual = result.keys().cloned().collect::<HashSet<_>>();
    if actual == *expected {
        return Ok(());
    }
    let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    missing.sort();
    Err(format!(
        "Topic message hash query is missing live topics {missing:?}"
    ))
}

async fn enforce_phase3_budget(pool: &SqlitePool, topic_ids: &[String]) -> Result<(), String> {
    let mut budget = Phase3StateBudget::default();
    for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = topic_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT topic_id, COUNT(*) AS message_count, \
             COALESCE(SUM(LENGTH(CAST(msg_id AS BLOB)) + \
             LENGTH(CAST(content_hash AS BLOB)) + 16), 0) AS state_bytes \
             FROM messages WHERE topic_id IN ({placeholders}) GROUP BY topic_id"
        );
        let mut query = sqlx::query(&sql);
        for id in topic_chunk {
            query = query.bind(id);
        }
        for row in query
            .fetch_all(pool)
            .await
            .map_err(|error| format!("Phase 3 message budget query failed: {error}"))?
        {
            let topic_id: String = row
                .try_get("topic_id")
                .map_err(|error| format!("Phase 3 budget topic id decode failed: {error}"))?;
            let count: i64 = row.try_get("message_count").map_err(|error| {
                format!("Phase 3 message count decode failed for {topic_id}: {error}")
            })?;
            let bytes: i64 = row.try_get("state_bytes").map_err(|error| {
                format!("Phase 3 state size decode failed for {topic_id}: {error}")
            })?;
            budget.observe_topic(
                &topic_id,
                usize::try_from(count)
                    .map_err(|_| format!("Phase 3 message count is invalid for {topic_id}"))?,
                usize::try_from(bytes)
                    .map_err(|_| format!("Phase 3 state size is invalid for {topic_id}"))?,
            )?;
        }
    }
    Ok(())
}

async fn load_message_hashes(
    pool: &SqlitePool,
    topic_ids: &[String],
    result: &mut HashMap<String, TopicLocalState>,
) -> Result<(), String> {
    for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = topic_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT topic_id, msg_id, content_hash, deleted_at \
             FROM messages WHERE topic_id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql);
        for id in topic_chunk {
            query = query.bind(id);
        }
        let mut rows = query.fetch(pool);
        while let Some(row) = rows
            .try_next()
            .await
            .map_err(|error| format!("Phase 3 message hash query failed: {error}"))?
        {
            insert_message_hash(result, row)?;
        }
    }
    Ok(())
}

fn insert_message_hash(
    result: &mut HashMap<String, TopicLocalState>,
    row: sqlx::sqlite::SqliteRow,
) -> Result<(), String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("Message hash topic id decode failed: {error}"))?;
    let msg_id: String = row
        .try_get("msg_id")
        .map_err(|error| format!("Message id decode failed for {topic_id}: {error}"))?;
    let hash = row
        .try_get("content_hash")
        .map_err(|error| format!("Message hash decode failed for {topic_id}/{msg_id}: {error}"))?;
    let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(|error| {
        format!("Message tombstone decode failed for {topic_id}/{msg_id}: {error}")
    })?;
    let state = result
        .get_mut(&topic_id)
        .ok_or_else(|| format!("Message hash query returned an unknown topic {topic_id}"))?;
    let effective_hash = deleted_at.map_or(hash, |_| "DELETED".to_string());
    if state
        .messages
        .insert(msg_id.clone(), effective_hash)
        .is_some()
    {
        return Err(format!(
            "Message hash query returned duplicate message {msg_id} for {topic_id}"
        ));
    }
    Ok(())
}

impl Phase3Message {
    /// V2: 获取指定 owner 下所有 topic 的 config_hash 和 content_hash
    pub async fn get_targeted_topic_hashes(
        pool: &SqlitePool,
        owners: &[String],
    ) -> Result<HashMap<String, TargetedTopicHashState>, String> {
        if owners.is_empty() {
            return Ok(HashMap::new());
        }
        let expected = validate_unique_ids(owners, "Targeted topic owner request")?;
        load_targeted_topic_hashes(pool, owners, &expected).await
    }

    /// 批量获取指定 topic 的本地消息哈希，用于发送给桌面端计算 diff
    pub async fn get_topic_message_hashes(
        pool: &SqlitePool,
        topic_ids: &[String],
    ) -> Result<HashMap<String, TopicLocalState>, String> {
        if topic_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let expected = validate_unique_ids(topic_ids, "Topic message hash request")?;
        let mut result = load_topic_states(pool, topic_ids).await?;
        ensure_topic_coverage(&result, &expected)?;
        enforce_phase3_budget(pool, topic_ids).await?;
        load_message_hashes(pool, topic_ids, &mut result).await?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "phase3_message_tests.rs"]
mod tests;
