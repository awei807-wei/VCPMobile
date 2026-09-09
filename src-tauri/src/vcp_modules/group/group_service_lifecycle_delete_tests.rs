use super::*;

async fn assert_group_delete_rows(pool: &SqlitePool) -> Option<i64> {
    let deleted_at: Option<i64> =
        sqlx::query_scalar("SELECT deleted_at FROM groups WHERE group_id = 'group-a'")
            .fetch_one(pool)
            .await
            .expect("读取 Group 墓碑失败");
    assert!(deleted_at.is_some());
    let topic_deleted_at: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM topics
         WHERE owner_type = 'group' AND owner_id = 'group-a' AND topic_id = 'topic-a'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 Group 话题墓碑失败");
    let message_deleted_at: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'group-a'
           AND topic_id = 'topic-a' AND msg_id = 'msg-a'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 Group 消息墓碑失败");
    assert!(topic_deleted_at.is_some());
    assert!(message_deleted_at.is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM active_generations
             WHERE owner_type = 'group' AND owner_id = 'group-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM group_member_tags WHERE group_id = 'group-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM message_attachments WHERE owner_id = 'group-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        0
    );
    deleted_at
}

async fn assert_group_generation_preserved(pool: &SqlitePool) {
    let generation: (String, String, String, String, i64) = sqlx::query_as(
        "SELECT owner_type, owner_id, topic_id, msg_id, created_at
         FROM active_generations
         WHERE owner_type = 'group' AND owner_id = 'group-a'
           AND topic_id = 'topic-a' AND msg_id = 'msg-a'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        generation,
        (
            "group".to_string(),
            "group-a".to_string(),
            "topic-a".to_string(),
            "msg-a".to_string(),
            1,
        )
    );
}

#[tokio::test]
async fn direct_delete_is_atomic_and_fail_closed() {
    let app = test_app(test_pool(true).await);
    let pool = &app.state::<DbState>().pool;
    delete_group(app.handle().clone(), app.state(), "group-a".to_string())
        .await
        .expect("删除存活 Group 应成功");
    let deleted_at = assert_group_delete_rows(pool).await;

    let duplicate = delete_group(app.handle().clone(), app.state(), "group-a".to_string()).await;
    assert!(duplicate.is_err(), "重复删除必须 fail-closed");
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM groups WHERE group_id = 'group-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        deleted_at
    );
    let missing = delete_group(app.handle().clone(), app.state(), "missing".to_string()).await;
    assert!(missing.is_err(), "缺失 Group 删除必须 fail-closed");
}

#[tokio::test]
async fn direct_delete_rolls_back_before_relation_cleanup() {
    let app = test_app(test_pool(false).await);
    let error = delete_group(app.handle().clone(), app.state(), "group-a".to_string())
        .await
        .expect_err("附件关系表缺失时删除必须回滚");
    assert!(error.contains("message_attachments"));
    assert_group_generation_preserved(&app.state::<DbState>().pool).await;
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM groups WHERE group_id = 'group-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM topics
             WHERE owner_type = 'group' AND owner_id = 'group-a' AND topic_id = 'topic-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM messages
             WHERE owner_type = 'group' AND owner_id = 'group-a'
               AND topic_id = 'topic-a' AND msg_id = 'msg-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM group_member_tags WHERE group_id = 'group-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn save_and_delete_barrier_cannot_revive_group() {
    let app = test_app(test_pool(true).await);
    let state = app.state::<GroupManagerState>();
    let owner_lock = state.acquire_lock("group-a").await;
    let owner_guard = owner_lock.lock().await;
    let delete_app: &'static tauri::AppHandle<tauri::test::MockRuntime> =
        Box::leak(Box::new(app.handle().clone()));
    let save_app = delete_app.clone();
    let delete_task = tokio::spawn(async move {
        delete_group(
            delete_app.clone(),
            delete_app.state(),
            "group-a".to_string(),
        )
        .await
    });
    let save_task = tokio::spawn(async move {
        save_group_config(save_app.clone(), save_app.state(), stale_group()).await
    });
    tokio::task::yield_now().await;
    assert!(!delete_task.is_finished() && !save_task.is_finished());
    drop(owner_guard);
    let _ = delete_task.await.expect("删除任务 panic");
    let _ = save_task.await.expect("保存任务 panic");
    assert!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM groups WHERE group_id = 'group-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap()
        .is_some(),
        "save/delete barrier 后 Group 不得复活"
    );
}
