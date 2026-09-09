"""Lightweight failure-injection tests for the B4 fixture publisher/verifier."""

from __future__ import annotations

from pathlib import Path
import os
import sqlite3
import tempfile
import unittest
from unittest import mock

import b4_fixture
from b4_fixture import FixtureError
from b4_fixture_verify import verify_database


class FixturePublisherTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="b4-fixture-test-")
        self.root = Path(self.temp_dir.name)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def make_fixture(self) -> Path:
        output = self.root / "fixture.db"
        b4_fixture.prepare_database(output, 4, 8, 4096, True, True)
        return output

    def test_backup_replace_failure_restores_only_completed_moves(self) -> None:
        output = self.root / "fixture.db"
        wal = Path(str(output) + "-wal")
        output.write_bytes(b"old-db")
        wal.write_bytes(b"old-wal")
        original_replace = os.replace
        calls = 0

        def fail_second_replace(source: Path, destination: Path) -> None:
            nonlocal calls
            calls += 1
            if calls == 2:
                raise OSError("injected backup rename failure")
            original_replace(source, destination)

        with mock.patch.object(b4_fixture.os, "replace", side_effect=fail_second_replace):
            with self.assertRaises(OSError):
                b4_fixture.move_existing_to_backup(output, True)
        self.assertEqual(output.read_bytes(), b"old-db")
        self.assertEqual(wal.read_bytes(), b"old-wal")
        self.assertFalse(Path(str(output) + ".b4-old").exists())

    def test_publish_verify_failure_restores_old_bytes_and_building(self) -> None:
        output = self.root / "fixture.db"
        building = Path(str(output) + ".building")
        output.write_bytes(b"old-db")
        building.write_bytes(b"new-db")
        verified = {
            "requirements": {"topics": 4, "messages": 8, "minDecodedContentBytes": 4096}
        }
        failed = {"ok": False, "error": "injected verify failure"}
        with mock.patch.object(b4_fixture, "verify_database", return_value=failed):
            with self.assertRaises(FixtureError):
                b4_fixture.publish_fixture(output, building, verified, True)
        self.assertEqual(output.read_bytes(), b"old-db")
        self.assertFalse(building.exists())
        self.assertFalse(Path(str(output) + ".b4-old").exists())

    def test_backup_cleanup_failure_keeps_committed_new_database(self) -> None:
        output = self.root / "fixture.db"
        building = Path(str(output) + ".building")
        backup = Path(str(output) + ".b4-old")
        output.write_bytes(b"old-db")
        building.write_bytes(b"new-db")
        verified = {
            "requirements": {"topics": 4, "messages": 8, "minDecodedContentBytes": 4096}
        }
        published = {"ok": True}
        original_remove = b4_fixture.remove_exact

        def fail_backup_cleanup(path: Path) -> bool:
            if path == backup:
                raise OSError("injected backup unlink failure")
            return original_remove(path)

        with mock.patch.object(b4_fixture, "verify_database", return_value=published):
            with mock.patch.object(b4_fixture, "remove_exact", side_effect=fail_backup_cleanup):
                with self.assertRaises(OSError):
                    b4_fixture.publish_fixture(output, building, verified, True)
        self.assertEqual(output.read_bytes(), b"new-db")
        self.assertEqual(backup.read_bytes(), b"old-db")

    def test_cleanup_reports_unlink_failure_and_residual_path(self) -> None:
        output = self.root / "fixture.db"
        wal = Path(str(output) + "-wal")
        output.write_bytes(b"db")
        wal.write_bytes(b"wal")
        original_remove = b4_fixture.remove_exact

        def fail_wal_cleanup(path: Path) -> bool:
            if path == wal:
                raise OSError("injected sidecar unlink failure")
            return original_remove(path)

        with mock.patch.object(b4_fixture, "remove_exact", side_effect=fail_wal_cleanup):
            result = b4_fixture.cleanup_database(output)
        self.assertFalse(result["ok"])
        self.assertEqual(result["residualPaths"], [str(wal)])
        self.assertIn("injected sidecar unlink failure", result["error"])


class FixtureContentTypeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="b4-content-test-")
        self.path = Path(self.temp_dir.name) / "fixture.db"
        b4_fixture.prepare_database(self.path, 4, 8, 4096, True, True)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def mutate_content(self, table: str, deleted: bool = False) -> dict[str, object]:
        connection = sqlite3.connect(self.path)
        try:
            if table == "messages":
                predicate = "deleted_at IS NOT NULL" if deleted else "deleted_at IS NULL"
                connection.execute(
                    f"UPDATE messages SET content = ? WHERE rowid = "
                    f"(SELECT rowid FROM messages WHERE {predicate} LIMIT 1)",
                    (sqlite3.Binary(b"bad-content"),),
                )
            else:
                connection.execute(
                    "UPDATE messages_fts SET content = ? WHERE rowid = "
                    "(SELECT rowid FROM messages_fts LIMIT 1)",
                    (sqlite3.Binary(b"bad-index-content"),),
                )
            connection.commit()
        finally:
            connection.close()
        return verify_database(self.path, 4, 8, 4096, True)

    def test_live_message_blob_fails(self) -> None:
        result = self.mutate_content("messages")
        self.assertFalse(result["ok"])
        self.assertFalse(result["checks"]["messagesContentText"])

    def test_deleted_message_blob_fails(self) -> None:
        result = self.mutate_content("messages", deleted=True)
        self.assertFalse(result["ok"])
        self.assertFalse(result["checks"]["messagesContentText"])

    def test_fts_blob_fails(self) -> None:
        result = self.mutate_content("messages_fts")
        self.assertFalse(result["ok"])
        self.assertFalse(result["checks"]["ftsContentText"])


if __name__ == "__main__":
    unittest.main()
