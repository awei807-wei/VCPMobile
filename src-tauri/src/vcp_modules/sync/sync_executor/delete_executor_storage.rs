use crate::vcp_modules::sync_types::{OwnerType, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, OwnerKey, TopicKey};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

#[derive(Debug, Default)]
pub(super) struct DeleteReceipt {
    pub(super) active_messages: Vec<MessageKey>,
}

fn validate_owner_key(key: &OwnerKey) -> Result<OwnerType, String> {
    if !key.is_valid() {
        return Err("delete requires a valid agent/group owner identity".to_string());
    }
    OwnerType::try_from(key.owner_type.as_str())
        .map_err(|_| format!("unsupported owner type {}", key.owner_type))
}

fn validate_topic_key(key: &TopicKey) -> Result<(), String> {
    if key.is_valid() {
        Ok(())
    } else {
        Err("delete requires a valid composite topic identity".to_string())
    }
}

fn owner_table(owner_type: OwnerType) -> (&'static str, &'static str) {
    match owner_type {
        OwnerType::Agent => ("agents", "agent_id"),
        OwnerType::Group => ("groups", "group_id"),
    }
}

async fn owner_deleted_at(
    tx: &mut Transaction<'_, Sqlite>,
    key: &OwnerKey,
) -> Result<Option<Option<i64>>, String> {
    let owner_type = validate_owner_key(key)?;
    let (table, id_column) = owner_table(owner_type);
    let sql = format!("SELECT deleted_at FROM {table} WHERE {id_column} = ?");
    sqlx::query_scalar(&sql)
        .bind(&key.owner_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| format!("读取 {} 所有者墓碑失败: {error}", key.owner_type))
}

async fn owner_is_live(tx: &mut Transaction<'_, Sqlite>, key: &OwnerKey) -> Result<bool, String> {
    Ok(matches!(owner_deleted_at(tx, key).await?, Some(None)))
}

async fn load_active_messages_for_owner(
    tx: &mut Transaction<'_, Sqlite>,
    key: &OwnerKey,
) -> Result<Vec<MessageKey>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? ORDER BY topic_id, msg_id",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("读取所有者活跃生成失败: {error}"))?;
    rows.into_iter()
        .map(|row| {
            let topic_id: String = row.try_get("topic_id").map_err(|error| error.to_string())?;
            let msg_id: String = row.try_get("msg_id").map_err(|error| error.to_string())?;
            Ok(MessageKey::new(
                TopicKey::new(key.owner_type.clone(), key.owner_id.clone(), topic_id),
                msg_id,
            ))
        })
        .collect()
}

async fn delete_owner_side_tables(
    tx: &mut Transaction<'_, Sqlite>,
    key: &OwnerKey,
) -> Result<(), String> {
    for table in ["render_cache", "message_attachments", "active_generations"] {
        let sql = format!("DELETE FROM {table} WHERE owner_type = ? AND owner_id = ?");
        sqlx::query(&sql)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("清理所有者 {table} 关系失败: {error}"))?;
    }
    clear_owner_unread_receipts(tx, key).await?;
    if key.owner_type == "group" {
        sqlx::query("DELETE FROM group_member_tags WHERE group_id = ?")
            .bind(&key.owner_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("清理群组成员标签失败: {error}"))?;
    }
    Ok(())
}

async fn clear_owner_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &OwnerKey,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'message_unread_receipts'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn bubble_owner_hash(tx: &mut Transaction<'_, Sqlite>, key: &OwnerKey) -> Result<(), String> {
    match key.owner_type.as_str() {
        "agent" => {
            crate::vcp_modules::sync_hash::HashAggregator::bubble_agent_hash(tx, &key.owner_id)
                .await
        }
        "group" => {
            crate::vcp_modules::sync_hash::HashAggregator::bubble_group_hash(tx, &key.owner_id)
                .await
        }
        other => Err(format!("unsupported owner type {other}")),
    }
}

async fn update_owner_tombstone(
    tx: &mut Transaction<'_, Sqlite>,
    key: &OwnerKey,
    deleted_at: i64,
) -> Result<(), String> {
    let owner_type = validate_owner_key(key)?;
    let (table, id_column) = owner_table(owner_type);
    let sql = format!(
        "UPDATE {table} SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE {id_column} = ?"
    );
    let result = sqlx::query(&sql)
        .bind(deleted_at)
        .bind(&key.owner_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| format!("写入所有者墓碑失败: {error}"))?;
    if result.rows_affected() != 1 {
        return Err(format!(
            "{} {} 在删除期间消失",
            key.owner_type, key.owner_id
        ));
    }
    Ok(())
}

async fn cascade_owner_tombstone(
    tx: &mut Transaction<'_, Sqlite>,
    key: &OwnerKey,
    deleted_at: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ?",
    )
    .bind(deleted_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("级联话题墓碑失败: {error}"))?;
    sqlx::query::<Sqlite>(
        "UPDATE messages SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ?",
    )
    .bind(deleted_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("级联消息墓碑失败: {error}"))?;
    delete_owner_side_tables(tx, key).await
}

pub(super) async fn soft_delete_owner_data(
    pool: &SqlitePool,
    key: &OwnerKey,
    deleted_at: i64,
) -> Result<DeleteReceipt, String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    let current = owner_deleted_at(&mut tx, key).await?;
    let Some(current_deleted_at) = current else {
        return Ok(DeleteReceipt::default());
    };
    let active_messages = load_active_messages_for_owner(&mut tx, key).await?;
    cascade_owner_tombstone(&mut tx, key, deleted_at).await?;
    if current_deleted_at.is_none() {
        // The owner must still be live while its aggregate hash is recomputed.
        bubble_owner_hash(&mut tx, key).await?;
    }
    update_owner_tombstone(&mut tx, key, deleted_at).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(DeleteReceipt { active_messages })
}

async fn topic_deleted_at(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<Option<Option<i64>>, String> {
    validate_topic_key(key)?;
    sqlx::query_scalar(
        "SELECT deleted_at FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("读取话题墓碑失败: {error}"))
}

async fn load_active_messages_for_topic(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<Vec<MessageKey>, String> {
    let rows = sqlx::query(
        "SELECT msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? ORDER BY msg_id",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("读取话题活跃生成失败: {error}"))?;
    rows.into_iter()
        .map(|row| {
            let msg_id: String = row.try_get("msg_id").map_err(|error| error.to_string())?;
            Ok(MessageKey::new(key.clone(), msg_id))
        })
        .collect()
}

async fn delete_topic_side_tables(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    for table in ["render_cache", "message_attachments", "active_generations"] {
        let sql = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
        );
        sqlx::query(&sql)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("清理话题 {table} 关系失败: {error}"))?;
    }
    clear_topic_unread_receipts(tx, key).await?;
    Ok(())
}

async fn clear_topic_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'message_unread_receipts'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn insert_topic_tombstone(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    deleted_at: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at,
            config_hash, deleted_at
         ) VALUES (?, ?, ?, '', 0, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
            deleted_at = MAX(COALESCE(topics.deleted_at, 0), excluded.deleted_at)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(deleted_at)
    .bind(SYNC_TOMBSTONE_HASH)
    .bind(deleted_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("写入话题墓碑失败: {error}"))?;
    Ok(())
}

pub(super) async fn soft_delete_topic_data(
    pool: &SqlitePool,
    key: &TopicKey,
    deleted_at: i64,
) -> Result<DeleteReceipt, String> {
    validate_topic_key(key)?;
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    let current = topic_deleted_at(&mut tx, key).await?;
    if current.is_none() {
        insert_topic_tombstone(&mut tx, key, deleted_at).await?;
        if owner_is_live(&mut tx, &key.owner_key()).await? {
            bubble_owner_hash(&mut tx, &key.owner_key()).await?;
        }
        tx.commit().await.map_err(|error| error.to_string())?;
        return Ok(DeleteReceipt::default());
    }
    let Some(current_deleted_at) = current else {
        unreachable!("checked topic row state");
    };
    let active_messages = load_active_messages_for_topic(&mut tx, key).await?;
    sqlx::query::<Sqlite>(
        "UPDATE messages SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(deleted_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| format!("级联话题消息墓碑失败: {error}"))?;
    delete_topic_side_tables(&mut tx, key).await?;
    sqlx::query::<Sqlite>(
        "UPDATE topics SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(deleted_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| format!("写入话题墓碑失败: {error}"))?;
    if current_deleted_at.is_none() {
        bubble_owner_hash(&mut tx, &key.owner_key()).await?;
    }
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(DeleteReceipt { active_messages })
}
