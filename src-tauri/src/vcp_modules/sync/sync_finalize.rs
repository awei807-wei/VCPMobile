use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::SyncPipeline;
use crate::vcp_modules::sync_service::emit_sync_log;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

pub struct SyncFinalizer;

struct TopicBubbleMeta {
    owner_id: String,
    owner_type: String,
    title: String,
    created_at: i64,
    locked: bool,
    unread: bool,
}

#[derive(Debug)]
struct FinalizationStats {
    bubbled_topics: usize,
    affected_agents: usize,
    affected_groups: usize,
}

const SQLITE_BIND_CHUNK: usize = 400;

fn decode_topic_meta(row: sqlx::sqlite::SqliteRow) -> Result<(String, TopicBubbleMeta), String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("解码同步收尾 topic_id 失败: {error}"))?;
    let meta = TopicBubbleMeta {
        owner_id: row
            .try_get("owner_id")
            .map_err(|error| format!("解码同步收尾 owner_id 失败: {error}"))?,
        owner_type: row
            .try_get("owner_type")
            .map_err(|error| format!("解码同步收尾 owner_type 失败: {error}"))?,
        title: row
            .try_get("title")
            .map_err(|error| format!("解码同步收尾 title 失败: {error}"))?,
        created_at: row
            .try_get("created_at")
            .map_err(|error| format!("解码同步收尾 created_at 失败: {error}"))?,
        locked: row
            .try_get::<i64, _>("locked")
            .map_err(|error| format!("解码同步收尾 locked 失败: {error}"))?
            != 0,
        unread: row
            .try_get::<i64, _>("unread")
            .map_err(|error| format!("解码同步收尾 unread 失败: {error}"))?
            != 0,
    };
    Ok((topic_id, meta))
}

async fn load_topic_metadata(
    tx: &mut Transaction<'_, Sqlite>,
    topic_ids: &[&String],
) -> Result<HashMap<String, TopicBubbleMeta>, String> {
    let mut metadata = HashMap::new();
    for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = topic_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let query_sql = format!(
            "SELECT topic_id, owner_id, owner_type, title, created_at, locked, unread
             FROM topics WHERE deleted_at IS NULL AND topic_id IN ({placeholders})"
        );
        let mut query = sqlx::query(&query_sql);
        for topic_id in topic_chunk {
            query = query.bind(*topic_id);
        }
        let rows = query
            .fetch_all(&mut **tx)
            .await
            .map_err(|error| format!("读取同步收尾话题元数据失败: {error}"))?;
        for row in rows {
            let (topic_id, meta) = decode_topic_meta(row)?;
            if metadata.insert(topic_id.clone(), meta).is_some() {
                return Err(format!("同步收尾话题元数据重复: {topic_id}"));
            }
        }
    }
    Ok(metadata)
}

fn ensure_metadata_complete(
    modified_topics: &HashSet<String>,
    metadata: &HashMap<String, TopicBubbleMeta>,
) -> Result<(), String> {
    let actual_topics = metadata.keys().cloned().collect::<HashSet<_>>();
    if actual_topics != *modified_topics {
        let mut missing = modified_topics
            .difference(&actual_topics)
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        return Err(format!("同步收尾缺少 live 话题元数据: {missing:?}"));
    }
    Ok(())
}

async fn refresh_message_counts(
    tx: &mut Transaction<'_, Sqlite>,
    topic_ids: &[&String],
) -> Result<(), String> {
    let updated_at = chrono::Utc::now().timestamp_millis();
    for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = topic_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let update_sql = format!(
            "UPDATE topics SET
                msg_count = (SELECT COUNT(*) FROM messages
                             WHERE messages.topic_id = topics.topic_id AND deleted_at IS NULL),
                updated_at = ?
             WHERE deleted_at IS NULL AND topic_id IN ({placeholders})"
        );
        let mut update = sqlx::query(&update_sql).bind(updated_at);
        for topic_id in topic_chunk {
            update = update.bind(*topic_id);
        }
        let result = update
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("更新同步收尾消息计数失败: {error}"))?;
        if result.rows_affected() != topic_chunk.len() as u64 {
            return Err(format!(
                "同步收尾消息计数仅更新 {}/{} 个话题",
                result.rows_affected(),
                topic_chunk.len()
            ));
        }
    }
    Ok(())
}

async fn bubble_topics(
    tx: &mut Transaction<'_, Sqlite>,
    metadata: &HashMap<String, TopicBubbleMeta>,
) -> Result<(HashSet<String>, HashSet<String>), String> {
    let mut affected_agents = HashSet::new();
    let mut affected_groups = HashSet::new();
    for (topic_id, meta) in metadata {
        HashAggregator::bubble_topic_hash_with_meta(
            tx,
            topic_id,
            &meta.owner_type,
            &meta.title,
            meta.created_at,
            meta.locked,
            meta.unread,
        )
        .await
        .map_err(|error| format!("冒泡同步话题哈希失败 ({topic_id}): {error}"))?;
        match meta.owner_type.as_str() {
            "agent" => {
                affected_agents.insert(meta.owner_id.clone());
            }
            "group" => {
                affected_groups.insert(meta.owner_id.clone());
            }
            other => return Err(format!("同步话题 {topic_id} 的 owner_type 非法: {other}")),
        }
    }
    Ok((affected_agents, affected_groups))
}

async fn bubble_owners(
    tx: &mut Transaction<'_, Sqlite>,
    affected_agents: &HashSet<String>,
    affected_groups: &HashSet<String>,
) -> Result<(), String> {
    for agent_id in affected_agents {
        HashAggregator::bubble_agent_hash(tx, agent_id)
            .await
            .map_err(|error| format!("冒泡同步 Agent 哈希失败 ({agent_id}): {error}"))?;
    }
    for group_id in affected_groups {
        HashAggregator::bubble_group_hash(tx, group_id)
            .await
            .map_err(|error| format!("冒泡同步 Group 哈希失败 ({group_id}): {error}"))?;
    }
    Ok(())
}

async fn finalize_modified_topics(
    pool: &sqlx::SqlitePool,
    modified_topics: &HashSet<String>,
) -> Result<FinalizationStats, String> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("开启同步收尾事务失败: {error}"))?;
    let topic_ids = modified_topics.iter().collect::<Vec<_>>();
    let metadata = load_topic_metadata(&mut tx, &topic_ids).await?;
    ensure_metadata_complete(modified_topics, &metadata)?;
    refresh_message_counts(&mut tx, &topic_ids).await?;
    let (affected_agents, affected_groups) = bubble_topics(&mut tx, &metadata).await?;
    bubble_owners(&mut tx, &affected_agents, &affected_groups).await?;
    tx.commit()
        .await
        .map_err(|error| format!("提交同步收尾事务失败: {error}"))?;
    Ok(FinalizationStats {
        bubbled_topics: metadata.len(),
        affected_agents: affected_agents.len(),
        affected_groups: affected_groups.len(),
    })
}

pub fn invalidate_sync_entity_caches(app_handle: &AppHandle) {
    if let Some(state) =
        app_handle.try_state::<crate::vcp_modules::agent_service::AgentConfigState>()
    {
        state.caches.clear();
    }
    if let Some(state) =
        app_handle.try_state::<crate::vcp_modules::group_service::GroupManagerState>()
    {
        state.caches.clear();
    }
}

async fn finalize_and_report(
    app_handle: &AppHandle,
    db: &DbState,
    logger: &Arc<Mutex<SyncLogger>>,
    modified_topics: &HashSet<String>,
) -> Result<(), String> {
    let started_at = std::time::Instant::now();
    log::info!(
        "[SyncFinalizer] Finalizing {} modified topics",
        modified_topics.len()
    );
    emit_sync_log(
        app_handle,
        "info",
        &format!("正在校验 {} 个话题的一致性...", modified_topics.len()),
    );
    let stats = finalize_modified_topics(&db.pool, modified_topics)
        .await
        .inspect_err(|error| {
            if let Ok(mut sync_logger) = logger.lock() {
                sync_logger.log(LogLevel::Error, "finalize", error);
            }
            emit_sync_log(app_handle, "error", error);
        })?;
    let success = format!(
        "[SyncFinalizer] 一致性校验成功！耗时: {:?}. 冒泡话题: {}, 级联智能体: {}, 级联群组: {}.",
        started_at.elapsed(),
        stats.bubbled_topics,
        stats.affected_agents,
        stats.affected_groups
    );
    log::info!("{success}");
    emit_sync_log(app_handle, "success", &success);
    Ok(())
}

impl SyncFinalizer {
    pub async fn execute(
        app_handle: &AppHandle,
        db: &DbState,
        write_queue: &DbWriteQueue,
        pipeline: &SyncPipeline,
        logger: &Arc<Mutex<SyncLogger>>,
        modified_topics: HashSet<String>,
    ) -> Result<(), String> {
        write_queue
            .flush()
            .await
            .map_err(|error| format!("同步写队列落盘失败: {error}"))?;

        if !modified_topics.is_empty() {
            finalize_and_report(app_handle, db, logger, &modified_topics).await?;
        }

        invalidate_sync_entity_caches(app_handle);
        pipeline
            .on_messages_done()
            .await
            .map_err(|error| format!("推进同步收尾状态失败: {error}"))?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "sync_finalize_tests.rs"]
mod tests;
