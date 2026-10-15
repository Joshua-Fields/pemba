"""OQ-032 regression: the engine and the app share CPython's SQLite, so neither can break
the other's locks (https://www.sqlite.org/howtocorrupt.html §2.2-2.3).

Port of ``probes/sqlite_copies/run_probe.py``. With a second SQLite copy in the process,
an in-process ``close()`` by one copy dropped the other's write lock, a separate process
then committed, and that commit was lost. Both directions are tested deterministically:

* the engine holds its write lock: a ``sqlite3`` trace callback on the engine's writer
  runs the app's open/read/close and the separate process while the engine is inside
  its ``BEGIN IMMEDIATE`` (it fires on the revision insert);
* the app holds the write lock while an engine opens, reads and closes.

Each time the separate process must stay blocked, and afterwards its commit must survive.
"""

from __future__ import annotations

import sqlite3
import subprocess
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

import warden

OTHER_PROCESS = r"""
import sqlite3, sys
c = sqlite3.connect(sys.argv[1], timeout=0, isolation_level=None)
try:
    c.execute("BEGIN IMMEDIATE")
except sqlite3.OperationalError as e:
    print("blocked: " + str(e)); sys.exit(0)
c.execute("INSERT INTO app_rows VALUES ('other-process')")
c.execute("COMMIT")
print("committed")
"""

SCHEMA = "definition user {}\ndefinition doc { relation viewer: user }"


def other_process(path: Path) -> str:
    """A separate process tries ``BEGIN IMMEDIATE`` (no busy wait) and, if it gets the
    lock, inserts a row and commits."""
    out = subprocess.run(
        [sys.executable, "-c", OTHER_PROCESS, str(path)],
        capture_output=True, text=True, timeout=60, check=True,
    )
    return out.stdout.strip()


def app_open_read_close(path: Path) -> None:
    """What broke the lock before: a read makes CPython open the WAL index, and its
    ``close()`` released every ``fcntl`` lock the process held on the files."""
    c = sqlite3.connect(path)
    c.execute("SELECT count(*) FROM app_rows").fetchall()
    c.close()


def rows(path: Path) -> list[str]:
    c = sqlite3.connect(path)
    try:
        assert c.execute("PRAGMA integrity_check").fetchone() == ("ok",)
        return [r[0] for r in c.execute("SELECT who FROM app_rows ORDER BY rowid")]
    finally:
        c.close()


@pytest.fixture
def db(tmp_path: Path) -> Path:
    path = tmp_path / "app.db"
    c = sqlite3.connect(path)
    c.execute("PRAGMA journal_mode = WAL")
    c.execute("CREATE TABLE app_rows (who TEXT NOT NULL)")
    c.commit()
    c.close()
    return path


@pytest.fixture
def traced_writes(monkeypatch: pytest.MonkeyPatch) -> Iterator[list[Callable[[str], None]]]:
    """Installs a trace callback on every read-write connection the engine opens; each
    statement it runs is passed to the callables in the yielded list."""
    hooks: list[Callable[[str], None]] = []
    real_connect = sqlite3.connect

    def connect(*args: Any, **kwargs: Any) -> sqlite3.Connection:
        conn: sqlite3.Connection = real_connect(*args, **kwargs)
        if not kwargs.get("uri"):  # the engine opens its read-only readers by URI
            conn.set_trace_callback(lambda sql: [h(sql) for h in hooks])
        return conn

    monkeypatch.setattr(sqlite3, "connect", connect)
    yield hooks


def test_engine_write_lock_survives_an_app_close(
    db: Path, traced_writes: list[Callable[[str], None]]
) -> None:
    engine = warden.Engine.sqlite(db)
    engine.write_schema(SCHEMA)
    seen: list[str] = []

    def inside_the_write(sql: str) -> None:
        if sql.startswith("INSERT INTO warden_revisions") and not seen:
            seen.append(other_process(db))  # control: the engine holds the lock
            app_open_read_close(db)
            seen.append(other_process(db))  # the bug: this one got the lock

    traced_writes.append(inside_the_write)
    rev = engine.write(touch=["doc:readme#viewer@user:anne"])
    traced_writes.clear()

    assert len(seen) == 2, "the trace callback never saw the engine's write"
    assert seen[0].startswith("blocked"), seen
    assert seen[1].startswith("blocked"), "an app close() released the engine's write lock"
    assert engine.check("doc:readme", "viewer", "user:anne", at_least=rev)
    # The separate process can write now, and its commit survives the engine's next write.
    assert other_process(db) == "committed"
    engine.write(touch=["doc:readme#viewer@user:bob"])
    assert rows(db) == ["other-process"]
    assert engine.check("doc:readme", "viewer", "user:bob")


def test_app_write_lock_survives_an_engine_close(db: Path) -> None:
    warden.Engine.sqlite(db).write_schema(SCHEMA)  # migrate while the file is unlocked
    app = sqlite3.connect(db, isolation_level=None)
    app.execute("BEGIN IMMEDIATE")
    app.execute("INSERT INTO app_rows VALUES ('app')")
    assert other_process(db).startswith("blocked")

    # auto_migrate=False: migrating takes the write lock even when there is nothing to do.
    engine = warden.Engine.sqlite(db, auto_migrate=False)
    assert not engine.check("doc:readme", "viewer", "user:anne")
    del engine  # closes every engine connection now, not at interpreter exit

    assert other_process(db).startswith("blocked"), "an engine close released the app's lock"
    app.execute("COMMIT")
    assert other_process(db) == "committed"
    app.close()
    assert rows(db) == ["app", "other-process"]


@pytest.mark.parametrize(
    ("version", "threadsafety", "needle"),
    [
        ("3.25.3", 1, "3.25.3"),
        ("3.8.11", 3, "3.8.11"),
        ("bogus", 1, "bogus"),
        ("3.45.1", 0, "thread"),
    ],
)
def test_refuses_an_unusable_sqlite(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, version: str, threadsafety: int, needle: str
) -> None:
    monkeypatch.setattr(sqlite3, "sqlite_version", version)
    monkeypatch.setattr(sqlite3, "threadsafety", threadsafety)
    with pytest.raises(warden.StorageError) as info:
        warden.Engine.sqlite(tmp_path / "a.db")
    assert info.value.code == "STORE_ERROR"
    message = str(info.value)
    assert needle in message
    if threadsafety:
        assert warden.MIN_SQLITE_VERSION in message, message
    assert not (tmp_path / "a.db").exists(), "refused before opening the file"
