use chrono::Local;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use tauri::State;

use crate::vcp_modules::db_manager::DbState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TarvenRule {
    pub id: String,
    pub name: String,
    pub rule_type: String,
    pub is_enabled: bool,
    pub content: String,
    pub scope: String,
    pub wrap: bool,
    pub role: Option<String>,
    pub depth: Option<i32>,
    pub position: Option<String>,
    pub sort_order: i32,
}

pub async fn fetch_active_rules(
    pool: &Pool<Sqlite>,
    scope: &str,
) -> Result<Vec<TarvenRule>, String> {
    let rows = sqlx::query(
        "SELECT id, name, rule_type, is_enabled, content, scope, wrap, role, depth, position, sort_order
         FROM tarven_rules
         WHERE is_enabled = 1 AND (scope = 'global' OR scope = ?)
         ORDER BY sort_order ASC",
    )
    .bind(scope)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to fetch active rules: {}", e))?;
    Ok(rows.into_iter().map(rule_from_row).collect())
}

fn rule_from_row(row: sqlx::sqlite::SqliteRow) -> TarvenRule {
    TarvenRule {
        id: row.get("id"),
        name: row.get("name"),
        rule_type: row.get("rule_type"),
        is_enabled: row.get::<i32, _>("is_enabled") != 0,
        content: row.get("content"),
        scope: row.get("scope"),
        wrap: row.get::<i32, _>("wrap") != 0,
        role: row.get("role"),
        depth: row.get("depth"),
        position: row.get("position"),
        sort_order: row.get("sort_order"),
    }
}

#[tauri::command]
pub async fn get_tarven_rules(db_state: State<'_, DbState>) -> Result<Vec<TarvenRule>, String> {
    let rows = sqlx::query(
        "SELECT id, name, rule_type, is_enabled, content, scope, wrap, role, depth, position, sort_order
         FROM tarven_rules
         ORDER BY sort_order ASC",
    )
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| format!("Database error: {}", e))?;
    Ok(rows.into_iter().map(rule_from_row).collect())
}

#[tauri::command]
pub async fn save_tarven_rule(
    db_state: State<'_, DbState>,
    rule: TarvenRule,
) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO tarven_rules (id, name, rule_type, is_enabled, content, scope, wrap, role, depth, position, sort_order, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name, rule_type = excluded.rule_type,
            is_enabled = excluded.is_enabled, content = excluded.content,
            scope = excluded.scope, wrap = excluded.wrap, role = excluded.role,
            depth = excluded.depth, position = excluded.position,
            sort_order = excluded.sort_order, updated_at = excluded.updated_at",
    )
    .bind(rule.id)
    .bind(rule.name)
    .bind(rule.rule_type)
    .bind(if rule.is_enabled { 1 } else { 0 })
    .bind(rule.content)
    .bind(rule.scope)
    .bind(if rule.wrap { 1 } else { 0 })
    .bind(rule.role)
    .bind(rule.depth)
    .bind(rule.position)
    .bind(rule.sort_order)
    .bind(now)
    .bind(now)
    .execute(&db_state.pool)
    .await
    .map_err(|e| format!("Failed to save rule: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn delete_tarven_rule(db_state: State<'_, DbState>, id: String) -> Result<(), String> {
    if matches!(id.as_str(), "system_meta_injection" | "time_anchoring_v2") {
        return Err("系统内置高级注入规则禁止被删除".to_string());
    }
    sqlx::query("DELETE FROM tarven_rules WHERE id = ?")
        .bind(id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| format!("Failed to delete rule: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn toggle_rule_enabled(
    db_state: State<'_, DbState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    sqlx::query("UPDATE tarven_rules SET is_enabled = ?, updated_at = ? WHERE id = ?")
        .bind(if enabled { 1 } else { 0 })
        .bind(now)
        .bind(id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| format!("Failed to toggle rule: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn reorder_rules(
    db_state: State<'_, DbState>,
    rule_ids: Vec<String>,
) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    for (index, id) in rule_ids.iter().enumerate() {
        sqlx::query("UPDATE tarven_rules SET sort_order = ?, updated_at = ? WHERE id = ?")
            .bind(index as i32)
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Failed to update sort order for {}: {}", id, e))?;
    }
    tx.commit().await.map_err(|e| e.to_string())
}

pub async fn sync_system_preset_rules(pool: &Pool<Sqlite>) -> Result<(), String> {
    for preset in system_presets() {
        sync_system_preset(pool, preset).await?;
    }
    Ok(())
}

struct SystemPreset<'a> {
    id: &'a str,
    name: &'a str,
    rule_type: &'a str,
    enabled: i32,
    content: &'a str,
}

fn system_presets() -> [SystemPreset<'static>; 2] {
    [
        SystemPreset {
            id: "system_meta_injection",
            name: "系统元数据注入",
            rule_type: "system_meta_injection",
            enabled: 1,
            content: "包含当前系统时间、运行环境及话题创建时间元数据注入系统提示词。",
        },
        SystemPreset {
            id: "time_anchoring_v2",
            name: "消息时间线感知 V2",
            rule_type: "time_anchoring_v2",
            enabled: 0,
            content: "为上下文中每条消息注入伪系统发送时间戳，使大模型具备精确的时间线感知，防止其对物理时间产生幻觉。",
        },
    ]
}

async fn sync_system_preset(pool: &Pool<Sqlite>, preset: SystemPreset<'_>) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    let exists: Option<(i32,)> = sqlx::query_as("SELECT is_enabled FROM tarven_rules WHERE id = ?")
        .bind(preset.id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("Failed to query tarven_rules existence: {}", e))?;
    if exists.is_some() {
        update_system_preset(pool, preset, now).await
    } else {
        insert_system_preset(pool, preset, now).await
    }
}

async fn update_system_preset(
    pool: &Pool<Sqlite>,
    preset: SystemPreset<'_>,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE tarven_rules
         SET name = ?, rule_type = ?, content = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(preset.name)
    .bind(preset.rule_type)
    .bind(preset.content)
    .bind(now)
    .bind(preset.id)
    .execute(pool)
    .await
    .map_err(|e| format!("Failed to update system preset rule: {}", e))?;
    Ok(())
}

async fn insert_system_preset(
    pool: &Pool<Sqlite>,
    preset: SystemPreset<'_>,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO tarven_rules (id, name, rule_type, is_enabled, content, scope, wrap, sort_order, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'global', 0, -100, ?, ?)",
    )
    .bind(preset.id)
    .bind(preset.name)
    .bind(preset.rule_type)
    .bind(preset.enabled)
    .bind(preset.content)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| format!("Failed to insert system preset rule: {}", e))?;
    Ok(())
}
