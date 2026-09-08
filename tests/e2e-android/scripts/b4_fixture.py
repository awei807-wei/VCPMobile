#!/usr/bin/env python3
"""CLI for deterministic, migration-backed B4 Android search fixtures."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sqlite3
import sys
from typing import Any

from b4_fixture_data import (
    REQUIRED_CONTENT_BYTES,
    REQUIRED_MESSAGES,
    REQUIRED_TOPICS,
    FixtureError,
    assert_small_fixture_allowed,
    configure_connection,
    execute_migrations,
    normalize_path,
    non_negative_int,
    positive_int,
    sidecar_paths,
)
from b4_fixture_insert import insert_fixture
from b4_fixture_verify import verify_database


def prepare_database(
    output: Path,
    topics: int,
    messages: int,
    min_bytes: int,
    allow_small_fixture: bool,
    overwrite: bool,
) -> dict[str, Any]:
    """Build into a side file and publish only after a complete verification."""
    assert_small_fixture_allowed(topics, messages, min_bytes, allow_small_fixture)
    existing = (output, *sidecar_paths(output))
    if output.exists() and output.is_dir():
        raise FixtureError("output 不能指向目录")
    if any(candidate.exists() for candidate in existing) and not overwrite:
        raise FixtureError("output 已存在；如需替换请显式传入 --overwrite")
    building = output.with_name(output.name + ".building")
    building_paths = (building, *sidecar_paths(building))
    if any(candidate.exists() for candidate in building_paths):
        if building.exists() and building.is_dir():
            raise FixtureError("遗留 .building 路径是目录，拒绝覆盖")
        if not overwrite:
            raise FixtureError("检测到遗留 .building 文件；清理或显式 --overwrite 后重试")
        remove_exact_paths(building_paths)
    output.parent.mkdir(parents=True, exist_ok=True)
    try:
        verified = build_and_verify(
            building, topics, messages, min_bytes, allow_small_fixture
        )
        verified = publish_fixture(output, building, verified, overwrite)
        verified["command"] = "prepare"
        verified["prepared"] = True
        return verified
    except Exception:
        remove_exact_paths((building, *sidecar_paths(building)))
        raise


def build_and_verify(
    building: Path,
    topics: int,
    messages: int,
    min_bytes: int,
    allow_small_fixture: bool,
) -> dict[str, Any]:
    connection: sqlite3.Connection | None = None
    try:
        connection = sqlite3.connect(building)
        configure_connection(connection)
        execute_migrations(connection)
        insert_fixture(connection, topics, messages, min_bytes)
        connection.commit()
        quick_check = str(connection.execute("PRAGMA quick_check").fetchone()[0]).lower()
        if quick_check != "ok":
            raise FixtureError("生成后 quick_check 非 ok")
    finally:
        if connection is not None:
            connection.close()
    if any(candidate.exists() for candidate in sidecar_paths(building)):
        raise FixtureError("生成后检测到 SQLite sidecar，拒绝发布 fixture")
    verified = verify_database(building, topics, messages, min_bytes, allow_small_fixture)
    if not verified.get("ok"):
        raise FixtureError(str(verified.get("error", "生成 fixture 未通过 verify")))
    return verified


def backup_paths(output: Path) -> tuple[tuple[Path, Path], ...]:
    backup = output.with_name(output.name + ".b4-old")
    return tuple(
        (source, backup.with_name(backup.name + suffix))
        for source, suffix in zip(
            (output, *sidecar_paths(output)), ("", "-wal", "-shm", "-journal")
        )
    )


def move_existing_to_backup(output: Path, overwrite: bool) -> tuple[tuple[Path, Path], ...]:
    pairs = backup_paths(output)
    if not any(source.exists() for source, _ in pairs):
        return pairs
    if not overwrite:
        raise FixtureError("output 已存在；如需替换请显式传入 --overwrite")
    if any(destination.exists() for _, destination in pairs):
        raise FixtureError("检测到遗留 B4 备份文件，拒绝覆盖")
    moved: list[tuple[Path, Path]] = []
    try:
        for source, destination in pairs:
            if source.exists():
                os.replace(source, destination)
                moved.append((source, destination))
    except Exception:
        restore_moved_paths(moved)
        raise
    return pairs


def restore_moved_paths(moved: list[tuple[Path, Path]]) -> None:
    """Restore only paths whose source-to-backup rename completed."""
    for source, destination in reversed(moved):
        if destination.exists():
            if source.exists():
                raise FixtureError(f"恢复 B4 备份时目标已存在: {source}")
            os.replace(destination, source)


def restore_backup(output: Path, pairs: tuple[tuple[Path, Path], ...]) -> None:
    remove_exact(output)
    restore_moved_paths(list(pairs))


def publish_fixture(
    output: Path, building: Path, verified: dict[str, Any], overwrite: bool
) -> dict[str, Any]:
    pairs = move_existing_to_backup(output, overwrite)
    committed = False
    try:
        os.replace(building, output)
        published = verify_database(
            output,
            int(verified["requirements"]["topics"]),
            int(verified["requirements"]["messages"]),
            int(verified["requirements"]["minDecodedContentBytes"]),
            True,
        )
        if not published.get("ok"):
            raise FixtureError(str(published.get("error", "发布后 verify 失败")))
        committed = True
        for _, destination in pairs:
            remove_exact(destination)
        return published
    except Exception:
        if not committed:
            restore_backup(output, pairs)
        raise


def remove_exact(path: Path) -> bool:
    if path.exists() or path.is_symlink():
        if path.is_dir() and not path.is_symlink():
            raise FixtureError("拒绝递归删除目录")
        path.unlink()
        return True
    return False


def remove_exact_paths(paths: tuple[Path, ...]) -> None:
    for path in paths:
        remove_exact(path)


def cleanup_database(path: Path) -> dict[str, Any]:
    """Delete only the requested database and its known SQLite sidecars."""
    if path.exists() and path.is_dir():
        raise FixtureError("cleanup 不能指向目录")
    removed_file = False
    removed_sidecars = 0
    failures: list[str] = []
    try:
        removed_file = remove_exact(path)
    except (FixtureError, OSError) as exc:
        failures.append(f"{path}: {exc}")
    for candidate in sidecar_paths(path):
        try:
            if remove_exact(candidate):
                removed_sidecars += 1
        except (FixtureError, OSError) as exc:
            failures.append(f"{candidate}: {exc}")
    residual_paths = [
        str(candidate)
        for candidate in (path, *sidecar_paths(path))
        if candidate.exists() or candidate.is_symlink()
    ]
    result = {
        "schema": "vcp.android.b4.search-fixture.v1",
        "ok": not failures and not residual_paths,
        "command": "cleanup",
        "cleaned": not failures and not residual_paths,
        "removedFile": removed_file,
        "removedSidecars": removed_sidecars,
        "residualPaths": residual_paths,
    }
    if failures:
        result["error"] = "fixture cleanup 失败: " + "; ".join(failures)
    return result


def add_size_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--topics", default=str(REQUIRED_TOPICS))
    parser.add_argument("--messages", default=str(REQUIRED_MESSAGES))
    parser.add_argument(
        "--min-content-bytes",
        "--min-decoded-content-bytes",
        dest="min_content_bytes",
        default=str(REQUIRED_CONTENT_BYTES),
    )
    parser.add_argument("--allow-small-fixture", action="store_true")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="B4 deterministic SQLite fixture")
    subparsers = parser.add_subparsers(dest="command", required=True)
    prepare = subparsers.add_parser("prepare")
    prepare.add_argument("--output", required=True)
    add_size_options(prepare)
    prepare.add_argument("--overwrite", action="store_true")
    verify = subparsers.add_parser("verify")
    verify.add_argument("--input", required=True)
    add_size_options(verify)
    cleanup = subparsers.add_parser("cleanup")
    cleanup.add_argument("--input", required=True)
    return parser.parse_args(argv)


def run(argv: list[str]) -> dict[str, Any]:
    args = parse_args(argv)
    if args.command == "prepare":
        return prepare_database(
            normalize_path(args.output, "--output"),
            positive_int(args.topics, "--topics"),
            positive_int(args.messages, "--messages"),
            non_negative_int(args.min_content_bytes, "--min-content-bytes"),
            bool(args.allow_small_fixture),
            bool(args.overwrite),
        )
    if args.command == "verify":
        return verify_database(
            normalize_path(args.input),
            positive_int(args.topics, "--topics"),
            positive_int(args.messages, "--messages"),
            non_negative_int(args.min_content_bytes, "--min-content-bytes"),
            bool(args.allow_small_fixture),
        )
    return cleanup_database(normalize_path(args.input))


def main(argv: list[str] | None = None) -> int:
    try:
        result = run(list(sys.argv[1:] if argv is None else argv))
        print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return 0 if result.get("ok") else 1
    except (FixtureError, OSError, sqlite3.Error, ValueError) as exc:
        print(
            json.dumps(
                {"schema": "vcp.android.b4.search-fixture.v1", "ok": False, "error": str(exc)},
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
