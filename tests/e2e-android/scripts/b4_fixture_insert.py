"""Deterministic business-row insertion for the B4 fixture database."""

from __future__ import annotations

from typing import Any
import sqlite3

from b4_fixture_data import (
    DEFAULT_AGENT_ID,
    DEFAULT_GROUP_ID,
    DEFAULT_OWNER_ID,
    DEFAULT_OWNER_TYPE,
    SHARED_MESSAGE_ID,
    SHARED_TOPIC_ID,
    FixtureError,
    content_for,
    content_target,
    stable_hash,
    topic_message_counts,
)


def agent_values(timestamp: int) -> tuple[Any, ...]:
    return (
        DEFAULT_AGENT_ID,
        "B4 fixture agent",
        "Deterministic B4 search fixture.",
        "",
        "fixture-model",
        0.0,
        4096,
        1024,
        1,
        0,
        stable_hash("b4-fixture-agent-config"),
        stable_hash("b4-fixture-agent-content"),
        timestamp,
        None,
    )


def topic_row(topic_number: int, count: int, timestamp: int) -> tuple[Any, ...]:
    return topic_row_for_owner(
        f"b4-fixture-topic-{topic_number:06d}",
        count,
        timestamp,
        DEFAULT_OWNER_TYPE,
        DEFAULT_OWNER_ID,
        f"B4 fixture topic {topic_number:04d}",
    )


def message_rows(
    topic_number: int, count: int, start: int, target_bytes: int, timestamp: int
) -> tuple[list[tuple[Any, ...]], list[tuple[Any, ...]], int]:
    return message_rows_for_owner(
        f"b4-fixture-topic-{topic_number:06d}",
        count,
        start,
        target_bytes,
        timestamp,
        DEFAULT_OWNER_TYPE,
        DEFAULT_OWNER_ID,
        DEFAULT_AGENT_ID,
    )


def message_rows_for_owner(
    topic_id: str,
    count: int,
    start: int,
    target_bytes: int,
    timestamp: int,
    owner_type: str,
    owner_id: str,
    agent_id: str,
    first_message_id: str | None = None,
    group_id: str | None = None,
) -> tuple[list[tuple[Any, ...]], list[tuple[Any, ...]], int]:
    topic_time = timestamp + len(topic_id)
    messages: list[tuple[Any, ...]] = []
    fts_rows: list[tuple[Any, ...]] = []
    current = start
    for offset in range(count):
        current += 1
        msg_id = (
            first_message_id
            if offset == 0 and first_message_id
            else f"b4-fixture-message-{current:06d}"
        )
        content = content_for(current, target_bytes)
        message_time = topic_time + offset
        identity = (owner_type, owner_id, topic_id, msg_id)
        messages.append(
            identity
            + (
                "assistant",
                "B4 fixture",
                agent_id,
                content,
                message_time,
                1 if group_id else 0,
                group_id,
                "stop",
                stable_hash(content),
                message_time,
                message_time,
                None,
            )
        )
        fts_rows.append(identity + (content,))
    return messages, fts_rows, current


def insert_fixture(
    connection: sqlite3.Connection, topics: int, messages: int, min_bytes: int
) -> None:
    """Insert all deterministic live and deleted rows."""
    timestamp = 1_725_000_000_000
    insert_agent(connection, timestamp)
    insert_group(connection, timestamp)
    connection.execute(
        "INSERT INTO settings(key, value, updated_at) VALUES (?, ?, ?)",
        ("b4_fixture_version", "v1", timestamp),
    )
    if topics < 2:
        raise FixtureError("fixture 至少需要 2 个 topic 以覆盖跨 owner identity")
    topic_rows, message_rows_all, fts_rows_all = build_live_rows(
        topics, messages, min_bytes, timestamp
    )
    insert_topics(connection, topic_rows)
    insert_messages(connection, message_rows_all)
    insert_fts(connection, fts_rows_all)
    insert_deleted_rows(connection, timestamp)


def insert_agent(connection: sqlite3.Connection, timestamp: int) -> None:
    connection.execute(
        """INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model,
            temperature, context_token_limit, max_output_tokens, stream_output,
            use_temperature, config_hash, content_hash, updated_at, deleted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        agent_values(timestamp),
    )


def insert_group(connection: sqlite3.Connection, timestamp: int) -> None:
    connection.execute(
        """INSERT INTO groups (
            group_id, name, mode, group_prompt, config_hash, content_hash,
            created_at, updated_at, deleted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        (
            DEFAULT_GROUP_ID,
            "B4 fixture group",
            "sequential",
            "Deterministic B4 group fixture.",
            stable_hash("b4-fixture-group-config"),
            stable_hash("b4-fixture-group-content"),
            timestamp,
            timestamp,
            None,
        ),
    )


def build_live_rows(
    topics: int, messages: int, min_bytes: int, timestamp: int
) -> tuple[list[tuple[Any, ...]], list[tuple[Any, ...]], list[tuple[Any, ...]]]:
    counts = list(topic_message_counts(topics, messages))
    target_bytes = content_target(messages, min_bytes)
    topic_rows: list[tuple[Any, ...]] = []
    message_rows_all: list[tuple[Any, ...]] = []
    fts_rows_all: list[tuple[Any, ...]] = []
    current = 0
    for topic_number, count in enumerate(counts[:-2], start=1):
        topic_rows.append(topic_row(topic_number, count, timestamp))
        rows, fts_rows, current = message_rows(
            topic_number, count, current, target_bytes, timestamp
        )
        message_rows_all.extend(rows)
        fts_rows_all.extend(fts_rows)
    current = append_shared_rows(
        topic_rows,
        message_rows_all,
        fts_rows_all,
        counts[-2],
        current,
        target_bytes,
        timestamp,
        DEFAULT_OWNER_TYPE,
        DEFAULT_OWNER_ID,
        "B4 shared agent topic",
    )
    current = append_shared_rows(
        topic_rows,
        message_rows_all,
        fts_rows_all,
        counts[-1],
        current,
        target_bytes,
        timestamp,
        "group",
        DEFAULT_GROUP_ID,
        "B4 shared group topic",
        DEFAULT_GROUP_ID,
    )
    if current != messages:
        raise FixtureError(f"fixture message distribution failed: {current} != {messages}")
    return topic_rows, message_rows_all, fts_rows_all


def append_shared_rows(
    topic_rows: list[tuple[Any, ...]],
    message_rows_all: list[tuple[Any, ...]],
    fts_rows_all: list[tuple[Any, ...]],
    count: int,
    current: int,
    target_bytes: int,
    timestamp: int,
    owner_type: str,
    owner_id: str,
    title: str,
    group_id: str | None = None,
) -> int:
    topic_rows.append(
        topic_row_for_owner(
            SHARED_TOPIC_ID, count, timestamp, owner_type, owner_id, title
        )
    )
    rows, fts_rows, current = message_rows_for_owner(
        SHARED_TOPIC_ID,
        count,
        current,
        target_bytes,
        timestamp,
        owner_type,
        owner_id,
        DEFAULT_AGENT_ID,
        SHARED_MESSAGE_ID,
        group_id,
    )
    message_rows_all.extend(rows)
    fts_rows_all.extend(fts_rows)
    return current


def topic_row_for_owner(
    topic_id: str,
    count: int,
    timestamp: int,
    owner_type: str,
    owner_id: str,
    title: str,
) -> tuple[Any, ...]:
    topic_time = timestamp + len(topic_id)
    return (
        owner_type,
        owner_id,
        topic_id,
        title,
        topic_time,
        topic_time,
        topic_time + count,
        1,
        0,
        0,
        count,
        stable_hash(f"{owner_type}:{owner_id}:{topic_id}"),
        stable_hash(f"{owner_type}:{owner_id}:{topic_id}:{count}"),
        None,
    )


def insert_deleted_rows(connection: sqlite3.Connection, timestamp: int) -> None:
    deleted_at = timestamp + 9_000_000
    connection.execute(
        """INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at,
            last_message_updated_at, locked, unread, unread_count, msg_count,
            config_hash, content_hash, deleted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        topic_row_for_owner(
            "b4-deleted-topic", 0, timestamp, DEFAULT_OWNER_TYPE,
            DEFAULT_OWNER_ID, "B4 deleted topic",
        )[:-1] + (deleted_at,),
    )
    insert_deleted_message(
        connection,
        "b4-deleted-topic",
        "b4-deleted-topic-message",
        "b4-deleted-only-content",
        deleted_at,
    )
    insert_deleted_message(
        connection,
        "b4-fixture-topic-000001",
        "b4-deleted-live-message",
        "b4-deleted-live-only-content",
        deleted_at,
    )


def insert_deleted_message(
    connection: sqlite3.Connection,
    topic_id: str,
    msg_id: str,
    content: str,
    deleted_at: int,
) -> None:
    connection.execute(
        """INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, name, agent_id,
            content, timestamp, is_group_message, group_id, finish_reason,
            content_hash, created_at, updated_at, deleted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        (
            DEFAULT_OWNER_TYPE,
            DEFAULT_OWNER_ID,
            topic_id,
            msg_id,
            "assistant",
            "B4 fixture",
            DEFAULT_AGENT_ID,
            content,
            deleted_at,
            0,
            None,
            "stop",
            stable_hash(content),
            deleted_at,
            deleted_at,
            deleted_at,
        ),
    )


def insert_topics(connection: sqlite3.Connection, rows: list[tuple[Any, ...]]) -> None:
    connection.executemany(
        """INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at,
            last_message_updated_at, locked, unread, unread_count, msg_count,
            config_hash, content_hash, deleted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        rows,
    )


def insert_messages(connection: sqlite3.Connection, rows: list[tuple[Any, ...]]) -> None:
    connection.executemany(
        """INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, name, agent_id,
            content, timestamp, is_group_message, group_id, finish_reason,
            content_hash, created_at, updated_at, deleted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        rows,
    )


def insert_fts(connection: sqlite3.Connection, rows: list[tuple[Any, ...]]) -> None:
    connection.executemany(
        """INSERT INTO messages_fts (
            owner_type, owner_id, topic_id, msg_id, content
        ) VALUES (?, ?, ?, ?, ?)""",
        rows,
    )
