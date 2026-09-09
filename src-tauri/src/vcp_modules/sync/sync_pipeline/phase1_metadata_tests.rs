use super::{validate_targeted_owners, Phase1Metadata};
use crate::vcp_modules::sync_types::{
    AvatarManifestState, ManifestRequest, OwnerManifestState, TopicManifestState,
};
use crate::vcp_modules::topic_types::OwnerKey;

fn hash(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

#[test]
fn targeted_owner_keys_reject_duplicates_and_unknown_namespaces() {
    let duplicate = [
        OwnerKey::new("agent", "agent-a"),
        OwnerKey::new("agent", "agent-a"),
    ];
    assert!(validate_targeted_owners(&duplicate).is_err());
    assert!(validate_targeted_owners(&[OwnerKey::new("user", "user-a")]).is_err());
    assert!(validate_targeted_owners(&[OwnerKey::new("group", "group-a")]).is_ok());
}

#[tokio::test]
async fn owner_manifest_combines_agent_and_group_states_with_tombstones() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE agents (
            agent_id TEXT, config_hash TEXT, content_hash TEXT,
            updated_at INTEGER, deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT, config_hash TEXT, content_hash TEXT,
            updated_at INTEGER, deleted_at INTEGER
         );
         INSERT INTO agents VALUES
            ('agent-a', ?, ?, 10, NULL);
         INSERT INTO groups VALUES
            ('group-a', ?, ?, 11, 9);",
    )
    .bind(hash('a'))
    .bind(hash('b'))
    .bind(hash('c'))
    .bind(hash('d'))
    .execute(&pool)
    .await
    .expect("create owner fixture");

    let ManifestRequest::Owner { items } = Phase1Metadata::build_owner_manifest(&pool)
        .await
        .expect("build owner manifest")
    else {
        panic!("owner builder returned another manifest type");
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], OwnerManifestState::Live(value) if value.owner_id == "agent-a"));
    assert!(
        matches!(&items[1], OwnerManifestState::Deleted(value) if value.owner_id == "group-a" && value.deleted_at == 9)
    );
}

#[tokio::test]
async fn targeted_topic_manifest_keeps_same_topic_id_in_distinct_owner_namespaces() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE topics (
            topic_id TEXT, config_hash TEXT, content_hash TEXT,
            updated_at INTEGER, owner_type TEXT, owner_id TEXT,
            deleted_at INTEGER
         );
         INSERT INTO topics VALUES
            ('shared', ?, ?, 10, 'agent', 'agent-a', NULL),
            ('shared', ?, ?, 11, 'group', 'group-a', 9);",
    )
    .bind(hash('a'))
    .bind(hash('b'))
    .bind(hash('c'))
    .bind(hash('d'))
    .execute(&pool)
    .await
    .expect("create topic fixture");

    let owners = [
        OwnerKey::new("agent", "agent-a"),
        OwnerKey::new("group", "group-a"),
    ];
    let ManifestRequest::Topic {
        items,
        targeted_owners,
    } = Phase1Metadata::build_targeted_topic_manifest(&pool, &owners)
        .await
        .expect("build topic manifest")
    else {
        panic!("topic builder returned another manifest type");
    };
    assert_eq!(targeted_owners, owners);
    assert_eq!(items.len(), 2);
    assert!(
        matches!(&items[0], TopicManifestState::Live(value) if value.owner_id == "agent-a" && value.topic_id == "shared")
    );
    assert!(
        matches!(&items[1], TopicManifestState::Deleted(value) if value.owner_id == "group-a" && value.topic_id == "shared")
    );
}

#[tokio::test]
async fn avatar_manifest_rejects_live_avatar_without_live_parent() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE avatars (
            owner_id TEXT, owner_type TEXT, avatar_hash TEXT,
            updated_at INTEGER, deleted_at INTEGER
         );
         CREATE TABLE agents (agent_id TEXT, deleted_at INTEGER);
         CREATE TABLE groups (group_id TEXT, deleted_at INTEGER);
         INSERT INTO agents VALUES ('agent-live', NULL);
         INSERT INTO avatars VALUES
            ('agent-live', 'agent', ?, 10, NULL),
            ('agent-missing', 'agent', ?, 11, NULL),
            ('user_avatar', 'user', ?, 12, NULL),
            ('agent-missing', 'agent', ?, 13, 9);",
    )
    .bind(hash('a'))
    .bind(hash('b'))
    .bind(hash('c'))
    .bind(hash('d'))
    .execute(&pool)
    .await
    .expect("create avatar fixture");

    assert!(Phase1Metadata::build_avatar_manifest(&pool).await.is_err());
}

#[tokio::test]
async fn avatar_manifest_preserves_tombstones_without_live_hashes() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE avatars (
            owner_id TEXT, owner_type TEXT, avatar_hash TEXT,
            updated_at INTEGER, deleted_at INTEGER
         );
         CREATE TABLE agents (agent_id TEXT, deleted_at INTEGER);
         CREATE TABLE groups (group_id TEXT, deleted_at INTEGER);
         INSERT INTO agents VALUES ('agent-live', NULL);
         INSERT INTO avatars VALUES
            ('agent-live', 'agent', ?, 10, NULL),
            ('agent-missing', 'agent', 'not-a-hash', 11, 9),
            ('user_avatar', 'user', ?, 12, NULL);",
    )
    .bind(hash('a'))
    .bind(hash('c'))
    .execute(&pool)
    .await
    .expect("create avatar tombstone fixture");

    let ManifestRequest::Avatar { items } = Phase1Metadata::build_avatar_manifest(&pool)
        .await
        .expect("build avatar manifest")
    else {
        panic!("avatar builder returned another manifest type");
    };
    assert_eq!(items.len(), 3);
    assert!(
        matches!(&items[0], AvatarManifestState::Live(value) if value.owner_id == "agent-live")
    );
    assert!(
        matches!(&items[1], AvatarManifestState::Deleted(value) if value.owner_id == "agent-missing" && value.deleted_at == 9)
    );
    assert!(
        matches!(&items[2], AvatarManifestState::Live(value) if value.owner_id == "user_avatar")
    );
}

#[tokio::test]
async fn invalid_live_hash_is_rejected_before_serialization() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE agents (
            agent_id TEXT, config_hash TEXT, content_hash TEXT,
            updated_at INTEGER, deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT, config_hash TEXT, content_hash TEXT,
            updated_at INTEGER, deleted_at INTEGER
         );
         INSERT INTO agents VALUES ('agent-a', 'not-a-hash', '', 10, NULL);",
    )
    .execute(&pool)
    .await
    .expect("create invalid fixture");
    assert!(Phase1Metadata::build_owner_manifest(&pool).await.is_err());
}
