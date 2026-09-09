use crate::vcp_modules::topic_types::TopicKey;

pub(super) async fn ensure_topic_is_live(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    let topic_is_live: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM topics
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if topic_is_live {
        Ok(())
    } else {
        Err(format!("topic {} is deleted or missing", key.topic_id))
    }
}
