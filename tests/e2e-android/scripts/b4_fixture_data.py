"""Shared constants and migration-safe helpers for the B4 fixture."""

from __future__ import annotations

import hashlib
import math
from pathlib import Path
import sqlite3
from typing import Any, Iterable


SCHEMA = "vcp.android.b4.search-fixture.v1"
REQUIRED_TOPICS = 1_800
REQUIRED_MESSAGES = 50_000
REQUIRED_CONTENT_BYTES = 100 * 1024 * 1024
DEFAULT_OWNER_TYPE = "agent"
DEFAULT_OWNER_ID = "b4-fixture-agent-0001"
DEFAULT_AGENT_ID = DEFAULT_OWNER_ID
DEFAULT_GROUP_ID = "b4-fixture-group-0001"
SHARED_TOPIC_ID = "b4-shared-topic"
SHARED_MESSAGE_ID = "b4-shared-message"
EMPTY_QUERY = "b4-no-such-token-9f3e7c"
QUERY_CASES = (
    ("default-zh-1", "中文1字", "全", "non-empty"),
    ("default-zh-2", "中文2字", "全局", "non-empty"),
    ("default-zh-3", "中文3+字", "三个字", "non-empty"),
    ("default-en", "英文", "English", "non-empty"),
    ("default-special", "特殊字符", "a.b", "non-empty"),
    ("default-empty", "确定空结果", EMPTY_QUERY, "empty"),
)
FILLER = "fixture-padding-0123456789abcdef "
MIGRATION_DIR = Path(__file__).resolve().parents[3] / "src-tauri" / "migrations"


class FixtureError(RuntimeError):
    """An expected, fail-closed fixture operation error."""


def positive_int(value: str, flag: str) -> int:
    try:
        parsed = int(value, 10)
    except (TypeError, ValueError) as exc:
        raise FixtureError(f"{flag} 必须是正整数") from exc
    if parsed < 1:
        raise FixtureError(f"{flag} 必须是正整数")
    return parsed


def non_negative_int(value: str, flag: str) -> int:
    try:
        parsed = int(value, 10)
    except (TypeError, ValueError) as exc:
        raise FixtureError(f"{flag} 必须是非负整数") from exc
    if parsed < 0:
        raise FixtureError(f"{flag} 必须是非负整数")
    return parsed


def normalize_path(value: str, flag: str = "--input") -> Path:
    if not isinstance(value, str) or not value.strip():
        raise FixtureError(f"{flag} 不能为空")
    path = Path(value).expanduser()
    if path.exists() and path.is_dir():
        raise FixtureError(f"{flag} 不能指向目录")
    if path.name in ("", ".", "..") or path == Path(path.anchor or "."):
        raise FixtureError(f"{flag} 不是安全的文件路径")
    return path


def sidecar_paths(path: Path) -> tuple[Path, ...]:
    return tuple(
        path.with_name(path.name + suffix)
        for suffix in ("-wal", "-shm", "-journal")
    )


def assert_small_fixture_allowed(
    topics: int, messages: int, min_bytes: int, allow_small: bool
) -> None:
    is_small = (
        topics != REQUIRED_TOPICS
        or messages != REQUIRED_MESSAGES
        or min_bytes < REQUIRED_CONTENT_BYTES
    )
    if is_small and not allow_small:
        raise FixtureError(
            "覆盖 B4 默认规模需要显式 --allow-small-fixture；"
            "默认要求 1800 topics、50000 messages、至少 100MiB 正文"
        )
    if topics > messages:
        raise FixtureError(
            "--topics 不能大于 --messages，否则无法为每个话题分配 live message"
        )


def ensure_migration_dir() -> Path:
    if not MIGRATION_DIR.is_dir():
        raise FixtureError(f"找不到 repository migrations: {MIGRATION_DIR}")
    if not list(MIGRATION_DIR.glob("*.sql")):
        raise FixtureError("repository migrations 为空，拒绝生成未迁移 schema")
    return MIGRATION_DIR


def execute_migrations(connection: sqlite3.Connection) -> None:
    migration_dir = ensure_migration_dir()
    for migration in sorted(migration_dir.glob("*.sql")):
        try:
            connection.executescript(migration.read_text(encoding="utf-8"))
        except sqlite3.Error as exc:
            raise FixtureError(f"执行 migration {migration.name} 失败: {exc}") from exc


def configure_connection(connection: sqlite3.Connection) -> None:
    connection.execute("PRAGMA page_size=16384")
    journal_mode = connection.execute("PRAGMA journal_mode=delete").fetchone()[0]
    if str(journal_mode).lower() != "delete":
        raise FixtureError(f"SQLite journal_mode 未锁定为 delete: {journal_mode}")
    connection.execute("PRAGMA auto_vacuum=2")
    connection.execute("PRAGMA foreign_keys=ON")
    if connection.execute("PRAGMA page_size").fetchone()[0] != 16384:
        raise FixtureError("SQLite page_size 未锁定为 16384")


def stable_hash(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def base_content(index: int) -> str:
    return (
        f"B4 fixture message {index:06d}; 全 全局 三个字 English a.b; "
        f"owner={DEFAULT_OWNER_ID}; deterministic search corpus. "
    )


def content_for(index: int, target_bytes: int) -> str:
    prefix = base_content(index)
    prefix_bytes = prefix.encode("utf-8")
    if len(prefix_bytes) >= target_bytes:
        return prefix
    needed = target_bytes - len(prefix_bytes)
    filler_bytes = FILLER.encode("ascii")
    repeated = (filler_bytes * ((needed // len(filler_bytes)) + 1))[:needed]
    return prefix + repeated.decode("ascii")


def topic_message_counts(topics: int, messages: int) -> Iterable[int]:
    base, remainder = divmod(messages, topics)
    for index in range(topics):
        yield base + (1 if index < remainder else 0)


def content_target(messages: int, min_bytes: int) -> int:
    base_size = len(base_content(1).encode("utf-8"))
    return max(base_size, math.ceil(min_bytes / messages) if messages else 0)


def pragma_value(connection: sqlite3.Connection, expression: str) -> Any:
    return connection.execute(f"PRAGMA {expression}").fetchone()[0]


def table_columns(connection: sqlite3.Connection, table: str) -> set[str]:
    return {row[1] for row in connection.execute(f"PRAGMA table_info({table})")}


def semantic_count(connection: sqlite3.Connection, query: str) -> int:
    if len(query) < 3:
        statement = "SELECT COUNT(*) FROM messages_fts WHERE instr(content, ?) > 0"
        params = (query,)
    else:
        escaped = query.replace('"', '""')
        statement = "SELECT COUNT(*) FROM messages_fts WHERE content MATCH ?"
        params = (f'"{escaped}"',)
    return int(connection.execute(statement, params).fetchone()[0])
