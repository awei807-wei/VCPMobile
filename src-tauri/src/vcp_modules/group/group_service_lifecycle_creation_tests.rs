use super::*;
use crate::vcp_modules::sync_dto::GroupTopicSyncDTO;
use crate::vcp_modules::sync_types::compute_merkle_root;
use sqlx::Row;

#[tokio::test]
async fn create_group_bubble_failure_rolls_back_group_and_initial_topic() {
    let app = test_app(test_pool(false).await);
    sqlx::query(
        "CREATE TRIGGER fail_group_bubble
         BEFORE UPDATE ON groups
         BEGIN SELECT RAISE(ABORT, 'injected group bubble failure'); END",
    )
    .execute(&app.state::<DbState>().pool)
    .await
    .unwrap();

    let result = create_group(app.handle().clone(), app.state(), "新增群组".to_string()).await;
    let error = result.expect_err("group bubble failure should abort creation");
    assert!(error.contains("injected group bubble failure"), "{error}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM groups")
            .fetch_one(&app.state::<DbState>().pool)
            .await
            .unwrap(),
        1,
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics")
            .fetch_one(&app.state::<DbState>().pool)
            .await
            .unwrap(),
        1,
    );
}

#[tokio::test]
async fn create_group_persists_default_config_for_restart_and_hash() {
    let app = test_app(test_pool(false).await);
    let created = create_group(
        app.handle().clone(),
        app.state(),
        "默认字段群组".to_string(),
    )
    .await
    .expect("创建 Group 应成功");
    let loaded = read_group_config(app.handle().clone(), app.state(), created.id.clone())
        .await
        .expect("重新读取应能从数据库读取 Group");
    assert_loaded_group_matches_created(&loaded, &created);
    let group_content_hash =
        assert_group_row_persisted(&app.state::<DbState>().pool, &created, &loaded).await;
    assert_initial_group_topic_persisted(
        &app.state::<DbState>().pool,
        &created,
        &loaded,
        &group_content_hash,
    )
    .await;
    assert_group_starts_without_members(&app.state::<DbState>().pool, &created.id).await;
}

fn assert_loaded_group_matches_created(loaded: &GroupConfig, created: &GroupConfig) {
    assert_eq!(loaded.id, created.id);
    assert_eq!(loaded.name, created.name);
    assert_eq!(
        loaded.avatar_calculated_color,
        created.avatar_calculated_color
    );
    assert_eq!(loaded.members, created.members);
    assert_eq!(loaded.mode, created.mode);
    assert_eq!(loaded.member_tags, created.member_tags);
    assert_eq!(loaded.group_prompt, created.group_prompt);
    assert_eq!(loaded.invite_prompt, created.invite_prompt);
    assert_eq!(loaded.use_unified_model, created.use_unified_model);
    assert_eq!(loaded.unified_model, created.unified_model);
    assert_eq!(loaded.tag_match_mode, created.tag_match_mode);
    assert_eq!(loaded.created_at, created.created_at);
    assert_eq!(created.topics.len(), 1);
    assert_eq!(loaded.topics.len(), created.topics.len());
    assert_eq!(loaded.topics[0].id, created.topics[0].id);
    assert_eq!(loaded.topics[0].name, created.topics[0].name);
    assert_eq!(loaded.topics[0].created_at, created.topics[0].created_at);
    assert_eq!(loaded.topics[0].locked, created.topics[0].locked);
    assert_eq!(loaded.topics[0].unread, created.topics[0].unread);
    assert_eq!(
        loaded.topics[0].unread_count,
        created.topics[0].unread_count
    );
    assert_eq!(loaded.topics[0].msg_count, created.topics[0].msg_count);
    assert_eq!(loaded.topics[0].owner_id, created.topics[0].owner_id);
    assert_eq!(loaded.topics[0].owner_type, created.topics[0].owner_type);
}

async fn assert_group_row_persisted(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    created: &GroupConfig,
    loaded: &GroupConfig,
) -> String {
    let row = sqlx::query(
        "SELECT name, mode, group_prompt, invite_prompt, use_unified_model,
                unified_model, tag_match_mode, created_at, config_hash, content_hash
         FROM groups WHERE group_id = ?",
    )
    .bind(&created.id)
    .fetch_one(pool)
    .await
    .expect("读取 Group 持久化字段失败");
    assert_eq!(row.get::<String, _>("name"), created.name);
    assert_eq!(row.get::<String, _>("mode"), created.mode);
    assert_eq!(
        row.get::<Option<String>, _>("group_prompt"),
        created.group_prompt
    );
    assert_eq!(
        row.get::<Option<String>, _>("invite_prompt"),
        created.invite_prompt
    );
    assert_eq!(
        row.get::<i64, _>("use_unified_model") != 0,
        created.use_unified_model
    );
    assert_eq!(
        row.get::<Option<String>, _>("unified_model"),
        created.unified_model
    );
    assert_eq!(
        row.get::<Option<String>, _>("tag_match_mode"),
        created.tag_match_mode
    );
    assert_eq!(row.get::<i64, _>("created_at"), created.created_at);
    let expected_hash = HashAggregator::compute_group_config_hash(&GroupSyncDTO::from(loaded));
    assert_eq!(
        expected_hash,
        HashAggregator::compute_group_config_hash(&GroupSyncDTO::from(created))
    );
    assert_eq!(row.get::<String, _>("config_hash"), expected_hash);
    row.get("content_hash")
}

async fn assert_initial_group_topic_persisted(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    created: &GroupConfig,
    loaded: &GroupConfig,
    actual_group_content_hash: &str,
) {
    let topic_row = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count,
                owner_id, owner_type, config_hash, content_hash
         FROM topics WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(&created.id)
    .fetch_one(pool)
    .await
    .expect("读取初始 Group 话题失败");
    let topic = &loaded.topics[0];
    let expected_topic_config_hash =
        HashAggregator::compute_group_topic_metadata_hash(&GroupTopicSyncDTO::from(topic));
    let expected_topic_content_hash = compute_merkle_root(Vec::new());
    let expected_group_content_hash =
        compute_merkle_root(vec![HashAggregator::compute_topic_leaf_hash(
            &topic.id,
            &expected_topic_config_hash,
            &expected_topic_content_hash,
        )]);
    assert_eq!(topic_row.get::<String, _>("topic_id"), topic.id);
    assert_eq!(topic_row.get::<String, _>("title"), topic.name);
    assert_eq!(topic_row.get::<i64, _>("created_at"), topic.created_at);
    assert_eq!(topic_row.get::<i64, _>("locked") != 0, topic.locked);
    assert_eq!(topic_row.get::<i64, _>("unread") != 0, topic.unread);
    assert_eq!(
        topic_row.get::<i64, _>("unread_count"),
        i64::from(topic.unread_count)
    );
    assert_eq!(
        topic_row.get::<i64, _>("msg_count"),
        i64::from(topic.msg_count)
    );
    assert_eq!(topic_row.get::<String, _>("owner_id"), topic.owner_id);
    assert_eq!(topic_row.get::<String, _>("owner_type"), topic.owner_type);
    assert_eq!(
        topic_row.get::<String, _>("config_hash"),
        expected_topic_config_hash
    );
    assert_eq!(
        topic_row.get::<String, _>("content_hash"),
        expected_topic_content_hash
    );
    assert_eq!(actual_group_content_hash, expected_group_content_hash);
}

async fn assert_group_starts_without_members(pool: &sqlx::Pool<sqlx::Sqlite>, group_id: &str) {
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_members WHERE group_id = ?",)
            .bind(group_id)
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_member_tags WHERE group_id = ?",)
            .bind(group_id)
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
}
