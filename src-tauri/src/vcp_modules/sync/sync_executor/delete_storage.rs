use crate::vcp_modules::sync_hash::HashAggregator;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

#[derive(Clone, Copy)]
pub(super) struct OwnerDeleteSpec<'a> {
    pub(super) table: &'a str,
    pub(super) id_column: &'a str,
    pub(super) owner_type: &'a str,
}

async fn bubble_owner_hash(
    tx: &mut Transaction<'_, Sqlite>,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    match owner_type {
        "agent" => HashAggregator::bubble_agent_hash(tx, owner_id).await,
        "group" => HashAggregator::bubble_group_hash(tx, owner_id).await,
        other => Err(format!("Unsupported topic owner_type {other}")),
    }
}

async fn owner_is_live(
    tx: &mut Transaction<'_, Sqlite>,
    spec: OwnerDeleteSpec<'_>,
    owner_id: &str,
) -> Result<bool, String> {
    let sql = format!(
        "SELECT deleted_at FROM {} WHERE {} = ?",
        spec.table, spec.id_column
    );
    let current: Option<Option<i64>> = sqlx::query_scalar(&sql)
        .bind(owner_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    Ok(matches!(current, Some(None)))
}

async fn apply_owner_tombstone(
    tx: &mut Transaction<'_, Sqlite>,
    spec: OwnerDeleteSpec<'_>,
    owner_id: &str,
    deleted_at: i64,
) -> Result<(), String> {
    let update_sql = format!(
        "UPDATE {} SET deleted_at = ? WHERE {} = ? AND deleted_at IS NULL",
        spec.table, spec.id_column
    );
    let result = sqlx::query(&update_sql)
        .bind(deleted_at)
        .bind(owner_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    if result.rows_affected() != 1 {
        return Err(format!(
            "{} {owner_id} disappeared during delete",
            spec.owner_type
        ));
    }
    sqlx::query(
        "UPDATE topics SET deleted_at = ? \
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL",
    )
    .bind(deleted_at)
    .bind(owner_id)
    .bind(spec.owner_type)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE messages SET deleted_at = ? \
         WHERE topic_id IN ( \
           SELECT topic_id FROM topics WHERE owner_id = ? AND owner_type = ? \
         ) AND deleted_at IS NULL",
    )
    .bind(deleted_at)
    .bind(owner_id)
    .bind(spec.owner_type)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM active_generations WHERE owner_id = ? AND owner_type = ?")
        .bind(owner_id)
        .bind(spec.owner_type)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn soft_delete_owner_data(
    pool: &SqlitePool,
    spec: OwnerDeleteSpec<'_>,
    owner_id: &str,
    deleted_at: i64,
) -> Result<Vec<String>, String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    if !owner_is_live(&mut tx, spec, owner_id).await? {
        return Ok(Vec::new());
    }
    let active_ids: Vec<String> = sqlx::query_scalar(
        "SELECT msg_id FROM active_generations WHERE owner_id = ? AND owner_type = ?",
    )
    .bind(owner_id)
    .bind(spec.owner_type)
    .fetch_all(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    apply_owner_tombstone(&mut tx, spec, owner_id, deleted_at).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(active_ids)
}

async fn load_live_topic_parent(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
) -> Result<Option<(String, String)>, String> {
    let Some(row) =
        sqlx::query("SELECT owner_id, owner_type, deleted_at FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let deleted_at: Option<i64> = row
        .try_get("deleted_at")
        .map_err(|error| format!("Topic {topic_id} tombstone decode failed: {error}"))?;
    if deleted_at.is_some() {
        return Ok(None);
    }
    let owner_id = row
        .try_get("owner_id")
        .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
    let owner_type = row
        .try_get("owner_type")
        .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;
    Ok(Some((owner_id, owner_type)))
}

async fn apply_topic_tombstone(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    deleted_at: i64,
) -> Result<Vec<String>, String> {
    let active_ids = sqlx::query_scalar("SELECT msg_id FROM active_generations WHERE topic_id = ?")
        .bind(topic_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    let updated =
        sqlx::query("UPDATE topics SET deleted_at = ? WHERE topic_id = ? AND deleted_at IS NULL")
            .bind(deleted_at)
            .bind(topic_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| error.to_string())?;
    if updated.rows_affected() != 1 {
        return Err(format!("Topic {topic_id} disappeared during delete"));
    }
    sqlx::query("UPDATE messages SET deleted_at = ? WHERE topic_id = ? AND deleted_at IS NULL")
        .bind(deleted_at)
        .bind(topic_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM active_generations WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    Ok(active_ids)
}

pub(super) async fn soft_delete_topic_data(
    pool: &SqlitePool,
    topic_id: &str,
    deleted_at: i64,
) -> Result<Vec<String>, String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    let Some((owner_id, owner_type)) = load_live_topic_parent(&mut tx, topic_id).await? else {
        return Ok(Vec::new());
    };
    let active_ids = apply_topic_tombstone(&mut tx, topic_id, deleted_at).await?;
    bubble_owner_hash(&mut tx, &owner_type, &owner_id).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(active_ids)
}

async fn load_live_message_parent(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    message_id: &str,
) -> Result<Option<(String, String)>, String> {
    let parent = sqlx::query_as("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    let Some(parent) = parent else {
        return Ok(None);
    };
    let current: Option<Option<i64>> =
        sqlx::query_scalar("SELECT deleted_at FROM messages WHERE topic_id = ? AND msg_id = ?")
            .bind(topic_id)
            .bind(message_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| error.to_string())?;
    Ok(matches!(current, Some(None)).then_some(parent))
}

async fn apply_message_tombstone(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    message_id: &str,
    deleted_at: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE messages SET deleted_at = ? \
         WHERE topic_id = ? AND msg_id = ? AND deleted_at IS NULL",
    )
    .bind(deleted_at)
    .bind(topic_id)
    .bind(message_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    for table in ["render_cache", "message_attachments", "active_generations"] {
        let sql = format!("DELETE FROM {table} WHERE topic_id = ? AND msg_id = ?");
        sqlx::query(&sql)
            .bind(topic_id)
            .bind(message_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) async fn soft_delete_message_data(
    pool: &SqlitePool,
    topic_id: &str,
    message_id: &str,
    deleted_at: i64,
) -> Result<bool, String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    let Some((_owner_id, _owner_type)) =
        load_live_message_parent(&mut tx, topic_id, message_id).await?
    else {
        return Ok(false);
    };
    apply_message_tombstone(&mut tx, topic_id, message_id, deleted_at).await?;
    HashAggregator::bubble_from_topic(&mut tx, topic_id).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(test)]
#[path = "delete_storage_tests.rs"]
mod tests;
