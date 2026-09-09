use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::Row;
use std::collections::HashMap;
use tauri::{ipc::Channel, State};

/// 批量获取所有 owner 的未读计数，替代前端的 N+1 查询。
#[tauri::command]
pub async fn get_unread_counts(
    db_state: State<'_, DbState>,
) -> Result<HashMap<String, i32>, String> {
    let start_time = std::time::Instant::now();
    let result = load_unread_counts(&db_state.pool).await?;
    log::info!(
        "[Profile] get_unread_counts finished. Total: {}ms",
        start_time.elapsed().as_millis()
    );
    Ok(result)
}

pub(crate) async fn load_unread_counts(
    pool: &sqlx::SqlitePool,
) -> Result<HashMap<String, i32>, String> {
    let rows = sqlx::query(
        "SELECT owner_type, owner_id,
                CAST(COALESCE(SUM(CASE WHEN unread = 1 THEN unread_count ELSE 0 END), 0) AS INTEGER) as total_count,
                MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END) as has_unread
         FROM topics
         WHERE deleted_at IS NULL
         GROUP BY owner_type, owner_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut result = HashMap::new();
    for row in rows {
        let owner_type: String = row.get("owner_type");
        let owner_id: String = row.get("owner_id");
        let total_count: i64 = row.get("total_count");
        let has_unread: i32 = row.get("has_unread");
        let value = if total_count > 0 {
            total_count as i32
        } else if has_unread != 0 {
            -1
        } else {
            0
        };
        if value != 0 {
            result.insert(unread_owner_key(&owner_type, &owner_id), value);
        }
    }

    Ok(result)
}

pub(crate) fn unread_owner_key(owner_type: &str, owner_id: &str) -> String {
    format!("{}:{}", owner_type, encode_uri_component(owner_id))
}

/// Keep the owner aggregation key byte-for-byte compatible with the frontend's
/// `encodeURIComponent` contract. `urlencoding::encode` follows RFC 3986 and
/// escapes `!`, `'`, `(`, `)` and `*`, while JavaScript leaves those bytes
/// unescaped.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(*byte as char),
            byte => {
                use std::fmt::Write;
                write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::unread_owner_key;

    #[test]
    fn owner_unread_key_matches_frontend_encode_uri_component_contract() {
        let cases = [
            ("owner!x", "owner!x"),
            ("owner with space", "owner%20with%20space"),
            ("owner%value", "owner%25value"),
            ("owner/path", "owner%2Fpath"),
            ("所有者/😀", "%E6%89%80%E6%9C%89%E8%80%85%2F%F0%9F%98%80"),
        ];
        for (owner_id, encoded_id) in cases {
            assert_eq!(
                unread_owner_key("agent", owner_id),
                format!("agent:{encoded_id}")
            );
        }
    }

    #[test]
    fn owner_unread_key_keeps_agent_and_group_namespaces_separate() {
        assert_ne!(
            unread_owner_key("agent", "same/id"),
            unread_owner_key("group", "same/id")
        );
    }
}

#[tauri::command]
pub async fn get_topics(
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
) -> Result<Vec<Topic>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL
         ORDER BY created_at DESC",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|row| topic_from_row(&row, &owner_type, &owner_id))
        .collect())
}

#[tauri::command]
pub async fn get_topics_streamed(
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    on_chunk: Channel<Vec<Topic>>,
) -> Result<(), String> {
    let mut rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL
         ORDER BY created_at DESC",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .fetch(&db_state.pool);

    use futures_util::StreamExt;
    let mut chunk = Vec::new();
    while let Some(row_result) = rows.next().await {
        let row = row_result.map_err(|e| e.to_string())?;
        chunk.push(topic_from_row(&row, &owner_type, &owner_id));
        if chunk.len() >= 15 {
            on_chunk.send(chunk.clone()).map_err(|e| e.to_string())?;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        on_chunk.send(chunk).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn topic_from_row(row: &sqlx::sqlite::SqliteRow, owner_type: &str, owner_id: &str) -> Topic {
    let topic_key = TopicKey::new(owner_type, owner_id, row.get::<String, _>("topic_id"));
    Topic {
        id: topic_key.topic_id.clone(),
        name: row.get("title"),
        created_at: row.get("created_at"),
        locked: row.get::<i32, _>("locked") != 0,
        unread: row.get::<i32, _>("unread") != 0,
        unread_count: row.get("unread_count"),
        msg_count: row.get("msg_count"),
        owner_id: topic_key.owner_id,
        owner_type: topic_key.owner_type,
    }
}
