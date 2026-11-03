import threading
from urllib.parse import quote

import pemba
from django.db import connection

SCHEMA = """
definition user {}

definition team {
    relation member: user
}

definition folder {
    relation viewer: user | team#member
    permission view = viewer
}

definition document {
    relation parent: folder
    relation owner: user
    relation editor: user
    relation viewer: user | team#member
    permission edit = owner + editor
    permission view = viewer + edit + parent->view
}
"""


def _location():
    # Read on every call, because the test runner renames the database.
    s = connection.settings_dict
    if connection.vendor == "sqlite":
        return str(s["NAME"])
    user, password = quote(s["USER"], safe=""), quote(s["PASSWORD"], safe="")
    sslmode = s["OPTIONS"].get("sslmode", "prefer")
    return f"postgres://{user}:{password}@{s['HOST']}:{s['PORT']}/{s['NAME']}?sslmode={sslmode}"


def _open(location, auto_migrate):
    if connection.vendor == "sqlite":
        return pemba.Engine.sqlite(location, auto_migrate=auto_migrate)
    return pemba.Engine.postgres(location, schema="authz", auto_migrate=auto_migrate)


_engines = {}
_engines_lock = threading.Lock()


def get_engine():
    # Opened on first use, so each gunicorn worker gets its own: Engine isn't fork-safe.
    location = _location()
    with _engines_lock:
        if location not in _engines:
            _engines[location] = _open(location, auto_migrate=False)
        return _engines[location]


def write_conn():
    # Postgres takes Django's connection. SQLite takes the sqlite3.Connection inside it.
    return connection if connection.vendor == "postgresql" else connection.connection


def migrate_store():
    with _open(_location(), auto_migrate=True) as engine:
        engine.write_schema(SCHEMA)


def close_engines():
    # The test runner needs this before it can drop the test database.
    with _engines_lock:
        engines = list(_engines.values())
        _engines.clear()
    for engine in engines:
        engine.close()
