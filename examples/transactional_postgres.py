"""Run using PEMBA_PG_URL and PEMBA_TUTORIAL_SCHEMA (empty disposable schema).

Uses psycopg 3. The caller owns both commit and rollback. See the tutorial guide.
"""
from __future__ import annotations

import os

import psycopg
from psycopg import sql

import pemba

SCHEMA = "definition user {} definition document { relation owner: user permission view = owner }"

def run(dsn: str, schema: str) -> None:
    """Migrate a disposable schema, then exercise commit and forced rollback."""
    engine = pemba.Engine.postgres(dsn, schema=schema, auto_migrate=True)
    engine.write_schema(SCHEMA)
    documents = sql.Identifier(schema, "documents")
    with psycopg.connect(dsn, autocommit=True) as app:
        app.execute(sql.SQL("CREATE TABLE IF NOT EXISTS {} (id text PRIMARY KEY)").format(documents))
        with app.transaction():
            app.execute(sql.SQL("INSERT INTO {} (id) VALUES (%s)").format(documents), ("committed",))
            pending = engine.write(touch=["document:committed#owner@user:anne"], conn=app)
            assert pending.resolved is False
            try:
                pending.revision()
            except pemba.RevisionPending as error:
                assert error.code == "REVISION_PENDING"
            else:
                raise AssertionError("revision unexpectedly assigned before commit")
            assert engine.check("document:committed", "view", "user:anne") is False
        # transaction() commits on normal exit, rolls back on ANY exception (including commit failure).

        # Resolve only after a successful caller-owned commit in portable application code.
        revision = pending.revision()
        assert engine.check("document:committed", "view", "user:anne", at_least=revision) is True
        assert engine.check("document:committed", "view", "user:anne", at_least=pending) is True
        assert engine.check("document:committed", "view", "user:bob", at_least=revision) is False
        assert pending.resolved is True
        assert app.execute(sql.SQL("SELECT id FROM {}").format(documents)).fetchall() == [("committed",)]

        try:
            with app.transaction():
                app.execute(sql.SQL("INSERT INTO {} (id) VALUES (%s)").format(documents), ("rolled_back",))
                lost = engine.write(touch=["document:rolled_back#owner@user:anne"], conn=app)
                # Force an app failure AFTER the permission write: duplicate primary key.
                app.execute(sql.SQL("INSERT INTO {} (id) VALUES (%s)").format(documents), ("rolled_back",))
        except psycopg.errors.UniqueViolation:
            pass  # transaction() already rolled the entire transaction back.
        else:
            raise AssertionError("expected the duplicate app row to fail")
        try:
            lost.revision()
        except pemba.WriteRolledBack as error:
            assert error.code == "WRITE_ROLLED_BACK"
        else:
            raise AssertionError("rolled-back revision unexpectedly resolved")
        assert app.execute(sql.SQL("SELECT id FROM {}").format(documents)).fetchall() == [("committed",)]
        assert engine.check("document:rolled_back", "view", "user:anne") is False


if __name__ == "__main__":
    run(os.environ["PEMBA_PG_URL"], os.environ["PEMBA_TUTORIAL_SCHEMA"])
