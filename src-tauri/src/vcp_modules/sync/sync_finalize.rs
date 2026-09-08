use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::SyncPipeline;
use crate::vcp_modules::sync_service::emit_sync_log;
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;

pub struct SyncFinalizer;

struct TopicBubbleMeta {
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

const SQLITE_BIND_CHUNK: usize = 300;

fn decode_topic_meta(row: sqlx::sqlite::SqliteRow) -> Result<(TopicKey, TopicBubbleMeta), String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("解码同步收尾 topic_id 失败: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("解码同步收尾 owner_id 失败: {error}"))?;
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("解码同步收尾 owner_type 失败: {error}"))?;
    let key = TopicKey::new(owner_type, owner_id, topic_id);
    if !key.is_valid() {
        return Err(format!("同步收尾话题身份非法: {:?}", key));
    }
    let meta = TopicBubbleMeta {
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
    Ok((key, meta))
}

async fn load_topic_metadata(
    tx: &mut Transaction<'_, Sqlite>,
    topic_keys: &[&TopicKey],
) -> Result<HashMap<TopicKey, TopicBubbleMeta>, String> {
    let mut metadata = HashMap::new();
    for chunk in topic_keys.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?, ?, ?)")
            .collect::<Vec<_>>()
            .join(",");
        let query_sql = format!(
            "SELECT topic_id, owner_id, owner_type, title, created_at, locked, unread
             FROM topics WHERE deleted_at IS NULL
               AND (owner_type, owner_id, topic_id) IN ({placeholders})"
        );
        let mut query = sqlx::query(&query_sql);
        for key in chunk {
            query = query
                .bind(&key.owner_type)
                .bind(&key.owner_id)
                .bind(&key.topic_id);
        }
        let rows = query
            .fetch_all(&mut **tx)
            .await
            .map_err(|error| format!("读取同步收尾话题元数据失败: {error}"))?;
        for row in rows {
            let (key, meta) = decode_topic_meta(row)?;
            if metadata.insert(key.clone(), meta).is_some() {
                return Err(format!("同步收尾话题元数据重复: {:?}", key));
            }
        }
    }
    Ok(metadata)
}

fn ensure_metadata_complete(
    modified_topics: &HashSet<TopicKey>,
    metadata: &HashMap<TopicKey, TopicBubbleMeta>,
) -> Result<(), String> {
    let actual_topics = metadata.keys().cloned().collect::<HashSet<_>>();
    if actual_topics == *modified_topics {
        return Ok(());
    }
    let mut missing = modified_topics
        .difference(&actual_topics)
        .cloned()
        .collect::<Vec<_>>();
    missing.sort();
    Err(format!("同步收尾缺少 live 话题元数据: {missing:?}"))
}

async fn refresh_message_state(
    tx: &mut Transaction<'_, Sqlite>,
    topic_keys: &[&TopicKey],
) -> Result<(), String> {
    for chunk in topic_keys.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = chunk
            .iter()
            .map(|_| "(?, ?, ?)")
            .collect::<Vec<_>>()
            .join(",");
        let query_sql = format!(
            "UPDATE topics SET
                msg_count = (SELECT COUNT(*) FROM messages
                             WHERE messages.owner_type = topics.owner_type
                               AND messages.owner_id = topics.owner_id
                               AND messages.topic_id = topics.topic_id
                               AND messages.deleted_at IS NULL),
                last_message_updated_at = MAX(last_message_updated_at, COALESCE((
                    SELECT MAX(CASE WHEN deleted_at IS NULL
                                    THEN updated_at ELSE MAX(updated_at, deleted_at) END)
                    FROM messages
                    WHERE messages.owner_type = topics.owner_type
                      AND messages.owner_id = topics.owner_id
                      AND messages.topic_id = topics.topic_id
                ), 0))
             WHERE deleted_at IS NULL
               AND (owner_type, owner_id, topic_id) IN ({placeholders})"
        );
        let mut query = sqlx::query(&query_sql);
        for key in chunk {
            query = query
                .bind(&key.owner_type)
                .bind(&key.owner_id)
                .bind(&key.topic_id);
        }
        query
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("刷新同步收尾消息计数和活动时钟失败: {error}"))?;
    }
    Ok(())
}

async fn bubble_topics(
    tx: &mut Transaction<'_, Sqlite>,
    metadata: &HashMap<TopicKey, TopicBubbleMeta>,
) -> Result<HashSet<OwnerKey>, String> {
    let mut owners = HashSet::new();
    for (key, meta) in metadata {
        HashAggregator::bubble_topic_hash_with_meta_for_key(
            tx,
            key,
            &meta.title,
            meta.created_at,
            meta.locked,
            meta.unread,
        )
        .await
        .map_err(|error| format!("冒泡同步话题哈希失败 ({}): {error}", key.topic_id))?;
        owners.insert(key.owner_key());
    }
    Ok(owners)
}

async fn bubble_owners(
    tx: &mut Transaction<'_, Sqlite>,
    owners: &HashSet<OwnerKey>,
) -> Result<(usize, usize), String> {
    let mut agents = HashSet::new();
    let mut groups = HashSet::new();
    for owner in owners {
        match owner.owner_type.as_str() {
            "agent" => {
                agents.insert(owner.owner_id.clone());
            }
            "group" => {
                groups.insert(owner.owner_id.clone());
            }
            other => return Err(format!("同步收尾 owner_type 非法: {other}")),
        }
    }
    for owner_id in &agents {
        HashAggregator::bubble_agent_hash(tx, owner_id)
            .await
            .map_err(|error| format!("冒泡同步 Agent 哈希失败 ({owner_id}): {error}"))?;
    }
    for owner_id in &groups {
        HashAggregator::bubble_group_hash(tx, owner_id)
            .await
            .map_err(|error| format!("冒泡同步 Group 哈希失败 ({owner_id}): {error}"))?;
    }
    Ok((agents.len(), groups.len()))
}

async fn finalize_modified_topics(
    pool: &sqlx::SqlitePool,
    modified_topics: &HashSet<TopicKey>,
) -> Result<FinalizationStats, String> {
    if modified_topics.is_empty() {
        return Ok(FinalizationStats {
            bubbled_topics: 0,
            affected_agents: 0,
            affected_groups: 0,
        });
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("开启同步收尾事务失败: {error}"))?;
    let topic_keys = modified_topics.iter().collect::<Vec<_>>();
    let metadata = load_topic_metadata(&mut tx, &topic_keys).await?;
    ensure_metadata_complete(modified_topics, &metadata)?;
    refresh_message_state(&mut tx, &topic_keys).await?;
    let owners = bubble_topics(&mut tx, &metadata).await?;
    let (affected_agents, affected_groups) = bubble_owners(&mut tx, &owners).await?;
    tx.commit()
        .await
        .map_err(|error| format!("提交同步收尾事务失败: {error}"))?;
    Ok(FinalizationStats {
        bubbled_topics: metadata.len(),
        affected_agents,
        affected_groups,
    })
}

async fn finalize_and_report(
    app_handle: &AppHandle,
    db: &DbState,
    logger: &Arc<Mutex<SyncLogger>>,
    modified_topics: &HashSet<TopicKey>,
) -> Result<(), String> {
    let started_at = std::time::Instant::now();
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
    pub(crate) async fn reconcile_after_interruption(
        db: &DbState,
        modified_topics: &HashSet<TopicKey>,
    ) -> Result<(), String> {
        if modified_topics.is_empty() {
            return Ok(());
        }
        let stats = finalize_modified_topics(&db.pool, modified_topics).await?;
        log::info!(
            "[SyncFinalizer] Reconciled interrupted attempt: topics={}, agents={}, groups={}",
            stats.bubbled_topics,
            stats.affected_agents,
            stats.affected_groups
        );
        Ok(())
    }

    pub async fn execute(
        app_handle: &AppHandle,
        db: &DbState,
        write_queue: &DbWriteQueue,
        pipeline: &SyncPipeline,
        logger: &Arc<Mutex<SyncLogger>>,
        modified_topics: HashSet<TopicKey>,
    ) -> Result<(), String> {
        if let Err(error) = write_queue.flush().await {
            return Err(format!("同步写队列落盘失败: {error}"));
        }
        let finalize_result = if modified_topics.is_empty() {
            Ok(())
        } else {
            finalize_and_report(app_handle, db, logger, &modified_topics).await
        };
        finalize_result?;
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
