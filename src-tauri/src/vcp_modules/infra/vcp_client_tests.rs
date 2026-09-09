use super::active::delete_active_generation_for_key;
use super::*;
use tauri::Manager;

#[test]
fn 核心状态缺失时返回可重试错误() {
    let error = require_core_state::<()>(None).expect_err("核心状态缺失时必须拒绝请求");

    assert_eq!(error, CORE_NOT_READY_ERROR);
}

#[test]
fn 缺少正文时生产校验返回明确错误() {
    assert_eq!(
        require_full_content(&serde_json::json!({"status": "completed"})),
        Err("响应缺少 fullContent".to_string())
    );
    assert_eq!(
        require_full_content(&serde_json::json!({"fullContent": "正文"})),
        Ok("正文")
    );
}

#[test]
fn 流事件serde序列化携带正整数请求纪元() {
    let event = StreamEvent::thinking(
        "message-a".to_string(),
        Some(serde_json::json!({
            "ownerType": "agent",
            "agentId": "owner-a",
            "topicId": "topic-a"
        })),
        7,
    );
    let serialized = serde_json::to_value(event).expect("流事件序列化失败");
    assert_eq!(serialized["type"], "thinking");
    assert_eq!(serialized["generation"], 7);
    assert!(serialized.get("requestEpoch").is_none());
}

#[test]
fn 群聊回合取消和清理按完整群组话题身份隔离() {
    let app = tauri::test::mock_app();
    app.manage(CancelledGroupTurns::default());
    let first_key = group_turn_key("group-a", "shared-topic").unwrap();
    let second_key = group_turn_key("group-b", "shared-topic").unwrap();

    interruptGroupTurn(
        app.state(),
        "group-a".to_string(),
        "shared-topic".to_string(),
    )
    .expect("应取消第一个群组回合");
    let cancelled_turns = app.state::<CancelledGroupTurns>();
    assert!(cancelled_turns.is_cancelled(&first_key));
    assert!(!cancelled_turns.is_cancelled(&second_key));

    cancelled_turns.clear(&second_key);
    assert!(cancelled_turns.is_cancelled(&first_key));

    interruptGroupTurn(
        app.state(),
        "group-b".to_string(),
        "shared-topic".to_string(),
    )
    .expect("应独立取消第二个群组回合");
    cancelled_turns.clear(&first_key);
    assert!(!cancelled_turns.is_cancelled(&first_key));
    assert!(cancelled_turns.is_cancelled(&second_key));
}

#[test]
fn 群聊回合取消拒绝不完整身份() {
    let app = tauri::test::mock_app();
    app.manage(CancelledGroupTurns::default());

    let error = interruptGroupTurn(app.state(), String::new(), "shared-topic".to_string())
        .expect_err("缺少群组标识时必须拒绝 topic-only 取消");

    assert!(error.contains("groupId"));
}

#[tokio::test]
async fn 活动生成清理严格按归属隔离() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建内存数据库");
    sqlx::query(
        "CREATE TABLE active_generations (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(&pool)
    .await
    .expect("创建活动生成表");

    let agent_key =
        registry::message_key_from_parts("owner-a", "agent", "shared-topic", "same-msg").unwrap();
    let group_key =
        registry::message_key_from_parts("owner-g", "group", "shared-topic", "same-msg").unwrap();
    for (key, created_at) in [(&agent_key, 10_i64), (&group_key, 20_i64)] {
        sqlx::query(
            "INSERT INTO active_generations
             (owner_type, owner_id, topic_id, msg_id, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&key.topic.owner_type)
        .bind(&key.topic.owner_id)
        .bind(&key.topic.topic_id)
        .bind(&key.msg_id)
        .bind(created_at)
        .execute(&pool)
        .await
        .expect("写入活动生成行");
    }

    let legacy_error = recovery::resolve_legacy_generation_key(&pool, "same-msg")
        .await
        .expect_err("旧版恢复遇到重复消息 ID 时必须拒绝");
    assert!(legacy_error.contains("ambiguous"));

    delete_active_generation_for_key(&pool, &agent_key)
        .await
        .expect("按归属删除活动生成");
    let remaining: (String, String, String, String) =
        sqlx::query_as("SELECT owner_type, owner_id, topic_id, msg_id FROM active_generations")
            .fetch_one(&pool)
            .await
            .expect("读取剩余活动生成");
    assert_eq!(
        remaining,
        (
            "group".to_string(),
            "owner-g".to_string(),
            "shared-topic".to_string(),
            "same-msg".to_string()
        )
    );
    delete_active_generation_for_key(&pool, &group_key)
        .await
        .expect("删除第二条活动生成");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
        .fetch_one(&pool)
        .await
        .expect("读取活动生成数量");
    assert_eq!(count, 0);
}
