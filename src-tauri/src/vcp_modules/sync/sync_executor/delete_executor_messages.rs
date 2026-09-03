use super::storage::DeleteReceipt;
use crate::vcp_modules::db_write_queue::{ExpectedMessageStates, SNAPSHOT_STALE_MARKER};
use crate::vcp_modules::sync_types::MessageDeleteDecision;
use crate::vcp_modules::sync_types::{MessageDeletedState, MessageLiveState, MessageVersionState};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::collections::BTreeMap;

const MAX_SAFE_TIMESTAMP: i64 = (1_i64 << 53) - 1;

fn validate_topic_key(key: &TopicKey) -> Result<(), String> {
    if key.is_valid() {
        Ok(())
    } else {
        Err("delete requires a valid composite topic identity".to_string())
    }
}

fn validate_tombstones(
    key: &TopicKey,
    tombstones: &[MessageDeleteDecision],
) -> Result<BTreeMap<String, i64>, String> {
    validate_topic_key(key)?;
    if tombstones.is_empty() || tombstones.len() > 10_000 {
        return Err("message delete requires 1..=10000 tombstones".to_string());
    }
    let mut values = BTreeMap::new();
    for tombstone in tombstones {
        if tombstone.msg_id.is_empty()
            || !(0..=MAX_SAFE_TIMESTAMP).contains(&tombstone.deleted_at)
            || values
                .insert(tombstone.msg_id.clone(), tombstone.deleted_at)
                .is_some()
        {
            return Err(
                "message delete requires unique ids and non-negative safe deletedAt values"
                    .to_string(),
            );
        }
    }
    Ok(values)
}

async fn topic_is_live(tx: &mut Transaction<'_, Sqlite>, key: &TopicKey) -> Result<bool, String> {
    let deleted_at: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT deleted_at FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("读取话题状态失败: {error}"))?;
    Ok(matches!(deleted_at, Some(None)))
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

async fn load_matching_message_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &BTreeMap<String, i64>,
) -> Result<Vec<String>, String> {
    let sql = format!(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({})",
        placeholders(ids.len())
    );
    let mut query = sqlx::query_scalar(&sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in ids.keys() {
        query = query.bind(id);
    }
    query
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("读取待删除消息失败: {error}"))
}

async fn validate_message_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &BTreeMap<String, i64>,
    expected: &ExpectedMessageStates,
) -> Result<(), String> {
    if expected.len() != ids.len() || ids.keys().any(|id| !expected.contains_key(id)) {
        return Err(format!(
            "{SNAPSHOT_STALE_MARKER}: delete snapshot does not cover the tombstone batch"
        ));
    }
    for id in ids.keys() {
        let current = load_message_version(tx, key, id).await?;
        if current != expected[id] {
            return Err(format!(
                "{SNAPSHOT_STALE_MARKER}: local message changed after the Phase 3 snapshot"
            ));
        }
    }
    Ok(())
}

async fn load_message_version(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    message_id: &str,
) -> Result<Option<MessageVersionState>, String> {
    let row = sqlx::query_as::<_, (String, i64, Option<i64>)>(
        "SELECT content_hash, updated_at, deleted_at FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("读取消息版本失败: {error}"))?;
    Ok(
        row.map(|(message_hash, updated_at, deleted_at)| match deleted_at {
            Some(deleted_at) => MessageVersionState::Deleted(MessageDeletedState { deleted_at }),
            None => MessageVersionState::Live(MessageLiveState {
                message_hash,
                updated_at,
            }),
        }),
    )
}

async fn load_active_messages_for_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &[String],
) -> Result<Vec<MessageKey>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({}) ORDER BY msg_id",
        placeholders(ids.len())
    );
    let mut query = sqlx::query_scalar(&sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in ids {
        query = query.bind(id);
    }
    query
        .fetch_all(&mut **tx)
        .await
        .map(|values: Vec<String>| {
            values
                .into_iter()
                .map(|msg_id| MessageKey::new(key.clone(), msg_id))
                .collect()
        })
        .map_err(|error| format!("读取待删除消息活跃生成失败: {error}"))
}

async fn delete_message_side_tables(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &[String],
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    for table in ["render_cache", "message_attachments", "active_generations"] {
        let sql = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({})",
            placeholders(ids.len())
        );
        let mut query = sqlx::query(&sql)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for id in ids {
            query = query.bind(id);
        }
        query
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("清理消息 {table} 关系失败: {error}"))?;
    }
    Ok(())
}

async fn write_message_tombstones(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    existing_ids: &[String],
    values: &BTreeMap<String, i64>,
) -> Result<(), String> {
    let rows = existing_ids
        .iter()
        .map(|_| "(?, ?)")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "WITH incoming(msg_id, deleted_at) AS (VALUES {rows})
         UPDATE messages SET deleted_at = MAX(COALESCE(messages.deleted_at, 0),
             (SELECT incoming.deleted_at FROM incoming
              WHERE incoming.msg_id = messages.msg_id))
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN (SELECT msg_id FROM incoming)"
    );
    let mut query = sqlx::query::<Sqlite>(&sql);
    for id in existing_ids {
        query = query.bind(id).bind(
            values
                .get(id)
                .copied()
                .ok_or_else(|| format!("缺少消息 {id} 的墓碑时钟"))?,
        );
    }
    let updated = query
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| format!("写入消息墓碑失败: {error}"))?;
    if updated.rows_affected() != existing_ids.len() as u64 {
        return Err(format!(
            "消息墓碑只更新了 {}/{} 行",
            updated.rows_affected(),
            existing_ids.len()
        ));
    }
    Ok(())
}

async fn refresh_topic_activity(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET
            msg_count = (SELECT COUNT(*) FROM messages
                         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
                           AND deleted_at IS NULL),
            last_message_updated_at = MAX(last_message_updated_at, COALESCE((
                SELECT MAX(CASE WHEN deleted_at IS NULL
                                THEN updated_at ELSE MAX(updated_at, deleted_at) END)
                FROM messages
                WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
            ), 0))
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("刷新话题活动时钟失败: {error}"))?;
    Ok(())
}

pub(super) async fn soft_delete_messages_data(
    pool: &SqlitePool,
    key: &TopicKey,
    tombstones: &[MessageDeleteDecision],
    refresh_activity: bool,
    expected_states: Option<&ExpectedMessageStates>,
) -> Result<DeleteReceipt, String> {
    let values = validate_tombstones(key, tombstones)?;
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    if !topic_is_live(&mut tx, key).await? {
        if expected_states.is_some() {
            return Err(format!(
                "{SNAPSHOT_STALE_MARKER}: parent topic changed after the Phase 3 snapshot"
            ));
        }
        return Ok(DeleteReceipt::default());
    }
    if let Some(expected) = expected_states {
        validate_message_snapshot(&mut tx, key, &values, expected).await?;
    }
    let existing_ids = load_matching_message_ids(&mut tx, key, &values).await?;
    let active_messages = load_active_messages_for_ids(&mut tx, key, &existing_ids).await?;
    if !existing_ids.is_empty() {
        write_message_tombstones(&mut tx, key, &existing_ids, &values).await?;
        delete_message_side_tables(&mut tx, key, &existing_ids).await?;
    }
    if refresh_activity {
        refresh_topic_activity(&mut tx, key).await?;
        crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic_for_key(&mut tx, key)
            .await?;
    }
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(DeleteReceipt { active_messages })
}
