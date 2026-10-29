"""Run with: python examples/transactional_sqlite.py <fresh-app.db>.

An application error after the permission write rolls back both kinds of data.
"""
from __future__ import annotations

import sqlite3
import sys
from contextlib import closing
from pathlib import Path

import pemba

SCHEMA = "definition user {} definition document { relation owner: user permission view = owner }"

def run(path: Path) -> None:
    """Use a fresh file; the engine and app share CPython's SQLite library."""
    engine = pemba.Engine.sqlite(path)
    engine.write_schema(SCHEMA)
    with closing(sqlite3.connect(path, isolation_level=None)) as app:
        app.execute("CREATE TABLE documents (id TEXT PRIMARY KEY)")
        app.execute("BEGIN IMMEDIATE")
        try:
            app.execute("INSERT INTO documents (id) VALUES (?)", ("committed",))
            pending = engine.write(touch=["document:committed#owner@user:anne"], conn=app)
            assert pending.resolved is True  # number known, transaction not yet visible
            try:
                engine.check("document:committed", "view", "user:anne", at_least=pending)
            except pemba.ConsistencyError as error:
                assert error.code == "REVISION_NOT_YET_AVAILABLE"
            else:
                raise AssertionError("uncommitted revision unexpectedly readable")
            assert engine.check("document:committed", "view", "user:anne") is False
            app.execute("COMMIT")
        except BaseException:
            if app.in_transaction:
                app.execute("ROLLBACK")
            raise

        # Resolve only after a successful caller-owned commit in portable application code.
        revision = pending.revision()
        assert engine.check("document:committed", "view", "user:anne", at_least=revision) is True
        assert engine.check("document:committed", "view", "user:anne", at_least=pending) is True
        assert engine.check("document:committed", "view", "user:bob", at_least=revision) is False
        assert app.execute("SELECT id FROM documents").fetchall() == [("committed",)]

        app.execute("BEGIN IMMEDIATE")
        try:
            app.execute("INSERT INTO documents (id) VALUES (?)", ("rolled_back",))
            lost = engine.write(touch=["document:rolled_back#owner@user:anne"], conn=app)
            # Force an app failure AFTER the permission write: duplicate primary key.
            app.execute("INSERT INTO documents (id) VALUES (?)", ("rolled_back",))
        except sqlite3.IntegrityError:
            app.execute("ROLLBACK")
        except BaseException:
            app.execute("ROLLBACK")
            raise
        else:
            app.execute("ROLLBACK")
            raise AssertionError("expected the duplicate app row to fail")
        # SQLite cannot detect rollback from a token; its number is void and reusable.
        del lost
        assert app.execute("SELECT id FROM documents").fetchall() == [("committed",)]
        assert engine.check("document:rolled_back", "view", "user:anne") is False


if __name__ == "__main__":
    run(Path(sys.argv[1]))
