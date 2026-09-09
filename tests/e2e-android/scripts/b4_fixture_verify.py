"""Verification helpers for the deterministic B4 SQLite fixture."""

from __future__ import annotations

from pathlib import Path
import sqlite3
from typing import Any

from b4_fixture_data import (
    EMPTY_QUERY,
    QUERY_CASES,
    REQUIRED_CONTENT_BYTES,
    REQUIRED_MESSAGES,
    REQUIRED_TOPICS,
    FixtureError,
    assert_small_fixture_allowed,
    pragma_value,
    semantic_count,
    sidecar_paths,
    table_columns,
)


REQUIRED_TABLES = {
    "agents",
    "settings",
    "topics",
    "messages",
    "messages_fts",
    "active_generations",
    "group_member_tags",
    "recovery_cleanup_outbox",
    "attachment_gc_unlink_outbox",
}


def present_tables(connection: sqlite3.Connection) -> set[str]:
    return {
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
        )
    }


def schema_snapshot(connection: sqlite3.Connection) -> tuple[bool, bool, set[str], set[str]]:
    tables = present_tables(connection)
    schema_valid = REQUIRED_TABLES <= tables
    message_columns = table_columns(connection, "messages")
    topic_columns = table_columns(connection, "topics")
    fts_columns = table_columns(connection, "messages_fts")
    schema_valid = schema_valid and {
        "owner_type",
        "owner_id",
        "topic_id",
        "msg_id",
        "content",
        "deleted_at",
    } <= message_columns
    schema_valid = schema_valid and {
        "owner_type",
        "owner_id",
        "topic_id",
        "msg_count",
        "deleted_at",
    } <= topic_columns
    schema_valid = schema_valid and {
        "owner_type",
        "owner_id",
        "topic_id",
        "msg_id",
        "content",
    } <= fts_columns
    row = connection.execute(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='messages_fts'"
    ).fetchone()
    create_sql = str(row[0]).lower() if row else ""
    tokenizer_valid = (
        "tokenize = 'trigram'" in create_sql
        or 'tokenize = "trigram"' in create_sql
    )
    return schema_valid, tokenizer_valid, message_columns, fts_columns


def live_counts(connection: sqlite3.Connection) -> tuple[int, int, int]:
    topic_count = int(
        connection.execute(
            "SELECT COUNT(*) FROM topics WHERE deleted_at IS NULL"
        ).fetchone()[0]
    )
    message_count = int(
        connection.execute(
            "SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL"
        ).fetchone()[0]
    )
    indexed_count = int(
        connection.execute("SELECT COUNT(*) FROM messages_fts").fetchone()[0]
    )
    return topic_count, message_count, indexed_count


def decoded_content_bytes(
    connection: sqlite3.Connection, message_columns: set[str]
) -> tuple[int, int]:
    if "content" not in message_columns:
        return 0, 0
    total = 0
    errors = 0
    for (content,) in connection.execute(
        "SELECT content FROM messages WHERE deleted_at IS NULL"
    ):
        if not isinstance(content, str):
            errors += 1
            continue
        try:
            total += len(content.encode("utf-8"))
        except (UnicodeError, TypeError):
            errors += 1
    return total, errors


def content_types_text(
    connection: sqlite3.Connection,
    message_columns: set[str],
    fts_columns: set[str],
) -> tuple[bool, bool]:
    """Reject BLOB/other values in every live, deleted, or indexed row."""
    messages_text = "content" in message_columns and all(
        isinstance(content, str)
        for (content,) in connection.execute("SELECT content FROM messages")
    )
    fts_text = "content" in fts_columns and all(
        isinstance(content, str)
        for (content,) in connection.execute("SELECT content FROM messages_fts")
    )
    return messages_text, fts_text


def semantic_checks(connection: sqlite3.Connection, schema_valid: bool) -> list[dict[str, Any]]:
    checks: list[dict[str, Any]] = []
    for case_id, category, query, expectation in QUERY_CASES:
        observed = semantic_count(connection, query) if schema_valid else 0
        checks.append(
            {
                "caseId": case_id,
                "category": category,
                "expectation": expectation,
                "nonEmpty": observed > 0,
                "passed": observed > 0 if expectation == "non-empty" else observed == 0,
            }
        )
    return checks


def check_values(
    page_size: int,
    journal_mode: str,
    quick_check: str,
    schema_valid: bool,
    tokenizer_valid: bool,
    topic_count: int,
    message_count: int,
    indexed_count: int,
    decoded_bytes: int,
    decode_errors: int,
    messages_content_text: bool,
    fts_content_text: bool,
    expected_topics: int,
    expected_messages: int,
    min_bytes: int,
    semantic_valid: bool,
    topic_counts_valid: bool,
    deleted_rows_excluded: bool,
    owner_tuple_isolated: bool,
    sidecars_clean: bool,
) -> dict[str, bool]:
    return {
        "pageSize": page_size == 16384,
        "journalMode": journal_mode == "delete",
        "quickCheck": quick_check == "ok",
        "schema": schema_valid,
        "tokenizer": tokenizer_valid,
        "topicsExact": topic_count == expected_topics,
        "liveTopicRowsExact": topic_count == expected_topics,
        "messagesExact": message_count == expected_messages,
        "ftsExact": indexed_count == message_count,
        "decodedContentMinimum": decoded_bytes >= min_bytes,
        "decodeErrorsZero": decode_errors == 0,
        "messagesContentText": messages_content_text,
        "ftsContentText": fts_content_text,
        "contentTypesText": messages_content_text and fts_content_text,
        "semanticQueries": semantic_valid,
        "topicMessageCounts": topic_counts_valid,
        "deletedRowsExcluded": deleted_rows_excluded,
        "ownerTupleIsolation": owner_tuple_isolated,
        "sidecarsClean": sidecars_clean,
    }


def topic_counts_valid(connection: sqlite3.Connection) -> bool:
    rows = connection.execute(
        """SELECT t.owner_type, t.owner_id, t.topic_id, t.msg_count,
                  COUNT(m.msg_id)
           FROM topics t
           LEFT JOIN messages m
             ON m.owner_type = t.owner_type AND m.owner_id = t.owner_id
            AND m.topic_id = t.topic_id AND m.deleted_at IS NULL
           WHERE t.deleted_at IS NULL
           GROUP BY t.owner_type, t.owner_id, t.topic_id, t.msg_count"""
    )
    return all(int(msg_count) == int(observed) for *_, msg_count, observed in rows)


def deletion_and_identity_checks(connection: sqlite3.Connection) -> tuple[bool, bool]:
    deleted_rows = connection.execute(
        """SELECT m.owner_type, m.owner_id, m.topic_id, m.msg_id
           FROM messages m
           WHERE m.deleted_at IS NOT NULL"""
    ).fetchall()
    deleted_excluded = bool(deleted_rows) and all(
        connection.execute(
            """SELECT COUNT(*) FROM messages_fts
               WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?""",
            row,
        ).fetchone()[0]
        == 0
        for row in deleted_rows
    )
    deleted_topic_excluded = (
        connection.execute(
            "SELECT COUNT(*) FROM messages_fts WHERE topic_id = 'b4-deleted-topic'"
        ).fetchone()[0]
        == 0
    )
    owner_rows = connection.execute(
        """SELECT owner_type, owner_id FROM messages
           WHERE topic_id = 'b4-shared-topic' AND msg_id = 'b4-shared-message'
             AND deleted_at IS NULL
           ORDER BY owner_type, owner_id"""
    ).fetchall()
    owner_fts = connection.execute(
        """SELECT owner_type, owner_id FROM messages_fts
           WHERE topic_id = 'b4-shared-topic' AND msg_id = 'b4-shared-message'
           ORDER BY owner_type, owner_id"""
    ).fetchall()
    isolated = owner_rows == [("agent", "b4-fixture-agent-0001"), ("group", "b4-fixture-group-0001")]
    return deleted_excluded and deleted_topic_excluded, isolated and owner_fts == owner_rows


def open_verified_connection(path: Path) -> sqlite3.Connection:
    """Open a fixture only after rejecting missing files and sidecars."""
    if not path.is_file():
        raise FixtureError("fixture 文件不存在")
    if any(candidate.exists() for candidate in sidecar_paths(path)):
        raise FixtureError("fixture 存在未清理 SQLite sidecar，拒绝验证")
    try:
        connection = sqlite3.connect(path)
        connection.execute("PRAGMA foreign_keys=ON")
        return connection
    except sqlite3.Error as exc:
        raise FixtureError(f"读取 fixture SQLite 失败: {exc}") from exc


def read_database_snapshot(connection: sqlite3.Connection) -> dict[str, Any]:
    """Read all verification inputs while the connection is open."""
    page_size = int(pragma_value(connection, "page_size"))
    journal_mode = str(pragma_value(connection, "journal_mode")).lower()
    quick_check = str(connection.execute("PRAGMA quick_check").fetchone()[0]).lower()
    schema_valid, tokenizer_valid, message_columns, fts_columns = schema_snapshot(connection)
    topic_count, message_count, indexed_count = live_counts(connection)
    decoded_bytes, decode_errors = decoded_content_bytes(connection, message_columns)
    messages_content_text, fts_content_text = content_types_text(
        connection, message_columns, fts_columns
    )
    semantic = semantic_checks(connection, schema_valid)
    topic_counts_ok = topic_counts_valid(connection) if schema_valid else False
    deleted_ok, owner_ok = (
        deletion_and_identity_checks(connection) if schema_valid else (False, False)
    )
    return {
        "page_size": page_size,
        "journal_mode": journal_mode,
        "quick_check": quick_check,
        "schema_valid": schema_valid,
        "tokenizer_valid": tokenizer_valid,
        "topic_count": topic_count,
        "message_count": message_count,
        "indexed_count": indexed_count,
        "decoded_bytes": decoded_bytes,
        "decode_errors": decode_errors,
        "messages_content_text": messages_content_text,
        "fts_content_text": fts_content_text,
        "semantic": semantic,
        "semantic_valid": all(item["passed"] for item in semantic),
        "topic_counts_ok": topic_counts_ok,
        "deleted_ok": deleted_ok,
        "owner_ok": owner_ok,
    }


def build_verification_result(
    snapshot: dict[str, Any],
    expected_topics: int,
    expected_messages: int,
    min_bytes: int,
    sidecars_clean: bool,
) -> dict[str, Any]:
    """Convert a database snapshot into the stable, safe verification DTO."""
    checks = check_values(
        snapshot["page_size"],
        snapshot["journal_mode"],
        snapshot["quick_check"],
        snapshot["schema_valid"],
        snapshot["tokenizer_valid"],
        snapshot["topic_count"],
        snapshot["message_count"],
        snapshot["indexed_count"],
        snapshot["decoded_bytes"],
        snapshot["decode_errors"],
        snapshot["messages_content_text"],
        snapshot["fts_content_text"],
        expected_topics,
        expected_messages,
        min_bytes,
        snapshot["semantic_valid"],
        snapshot["topic_counts_ok"],
        snapshot["deleted_ok"],
        snapshot["owner_ok"],
        sidecars_clean,
    )
    result: dict[str, Any] = {
        "schema": "vcp.android.b4.search-fixture.v1",
        "ok": all(checks.values()),
        "verified": all(checks.values()),
        "pageSize": snapshot["page_size"],
        "journalMode": snapshot["journal_mode"],
        "quickCheck": snapshot["quick_check"],
        "schemaValid": snapshot["schema_valid"],
        "tokenizerValid": snapshot["tokenizer_valid"],
        "topicCount": snapshot["topic_count"],
        "liveTopicRowCount": snapshot["topic_count"],
        "messageCount": snapshot["message_count"],
        "liveCount": snapshot["message_count"],
        "indexedCount": snapshot["indexed_count"],
        "decodedContentBytes": snapshot["decoded_bytes"],
        "decodeErrorCount": snapshot["decode_errors"],
        "messagesContentText": snapshot["messages_content_text"],
        "ftsContentText": snapshot["fts_content_text"],
        "semanticChecks": snapshot["semantic"],
        "semanticValid": snapshot["semantic_valid"],
        "checks": checks,
        "requirements": {
            "topics": expected_topics,
            "liveTopicRows": expected_topics,
            "messages": expected_messages,
            "minDecodedContentBytes": min_bytes,
        },
    }
    if not result["ok"]:
        failed = [name for name, passed in checks.items() if not passed]
        result["error"] = f"B4 fixture verify 失败: {', '.join(failed)}"
    return result


def verify_database(
    path: Path,
    expected_topics: int = REQUIRED_TOPICS,
    expected_messages: int = REQUIRED_MESSAGES,
    min_bytes: int = REQUIRED_CONTENT_BYTES,
    allow_small_fixture: bool = False,
) -> dict[str, Any]:
    """Return a safe verification DTO; malformed fixtures never pass."""
    assert_small_fixture_allowed(expected_topics, expected_messages, min_bytes, allow_small_fixture)
    connection = open_verified_connection(path)
    try:
        snapshot = read_database_snapshot(connection)
        return build_verification_result(
            snapshot,
            expected_topics,
            expected_messages,
            min_bytes,
            not any(candidate.exists() for candidate in sidecar_paths(path)),
        )
    except sqlite3.Error as exc:
        raise FixtureError(f"读取 fixture SQLite 失败: {exc}") from exc
    finally:
        connection.close()
