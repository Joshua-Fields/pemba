"""``Engine.postgres`` and foreign mode on Postgres (T5.9, ``docs/10-postgres.md`` §2.2–§2.4,
§4 item 6).

Needs ``PEMBA_PG_URL`` (a ``postgres://`` URL to a server the tests may create schemas and
databases on); without it the module reports "not run", unless ``PEMBA_PG_REQUIRED`` is
set. Each driver's tests run only where that driver is installed (psycopg 3, psycopg2,
SQLAlchemy 2, Django).
"""

from __future__ import annotations

import asyncio
import os
import uuid
import warnings
from collections.abc import Coroutine, Iterator
from contextlib import AbstractContextManager
from typing import Any, TypeVar
from urllib.parse import urlsplit

import pytest

import pemba
from conftest import SCHEMA

URL = os.environ.get("PEMBA_PG_URL", "")
if not URL:
    if os.environ.get("PEMBA_PG_REQUIRED"):
        raise RuntimeError("PEMBA_PG_REQUIRED is set but PEMBA_PG_URL is not")
    pytest.skip("not run: set PEMBA_PG_URL (docs/10-postgres.md §4)", allow_module_level=True)

psycopg = pytest.importorskip("psycopg")

T = TypeVar("T")

ANNE = "document:readme#viewer@user:anne"
BOB = "document:readme#viewer@user:bob"


def run_async(coro: Coroutine[Any, Any, T]) -> T:
    """Runs ``coro`` on a selector event loop: psycopg's async mode cannot use Windows'
    default proactor loop."""
    with asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
        return runner.run(coro)


def admin() -> Any:
    return psycopg.connect(URL, autocommit=True)


@pytest.fixture
def schema() -> Iterator[str]:
    """A fresh schema holding pemba's tables and the app's ``documents`` table."""
    name = f"pemba_py_{uuid.uuid4().hex[:12]}"
    with admin() as c:
        c.execute(f'CREATE SCHEMA "{name}"')
        c.execute(f'CREATE TABLE "{name}".documents (id text PRIMARY KEY)')
    yield name
    with admin() as c:
        c.execute(f'DROP SCHEMA "{name}" CASCADE')


@pytest.fixture
def engine(schema: str) -> pemba.Engine:
    e = pemba.Engine.postgres(URL, schema=schema, auto_migrate=True, pool_size=4)
    e.write_schema(SCHEMA)
    return e


def documents(schema: str) -> list[str]:
    with admin() as c:
        return [r[0] for r in c.execute(f'SELECT id FROM "{schema}".documents ORDER BY id')]


# ---- drivers: each opens a transaction, runs app SQL in it, commits or rolls back ----


class Driver:
    """One app connection of one driver kind."""

    name = ""

    def conn(self) -> Any:
        """What the app passes as ``conn=``, inside its transaction."""
        raise NotImplementedError

    def execute(self, sql: str) -> None:
        raise NotImplementedError

    def commit(self) -> None:
        raise NotImplementedError

    def rollback(self) -> None:
        raise NotImplementedError

    def close(self) -> None:
        pass


class Psycopg3(Driver):
    name = "psycopg3"

    def __init__(self) -> None:
        self.c = psycopg.connect(URL)
        self.c.execute("SELECT 1")  # inside its (implicit) transaction from here on

    def conn(self) -> Any:
        return self.c

    def execute(self, sql: str) -> None:
        self.c.execute(sql)

    def commit(self) -> None:
        self.c.commit()

    def rollback(self) -> None:
        self.c.rollback()

    def close(self) -> None:
        self.c.close()


class Psycopg2(Driver):
    name = "psycopg2"

    def __init__(self) -> None:
        psycopg2 = pytest.importorskip("psycopg2")
        self.c = psycopg2.connect(URL)

    def conn(self) -> Any:
        return self.c

    def execute(self, sql: str) -> None:
        with self.c.cursor() as cur:
            cur.execute(sql)

    def commit(self) -> None:
        self.c.commit()

    def rollback(self) -> None:
        self.c.rollback()

    def close(self) -> None:
        self.c.close()


def sqlalchemy_url() -> str:
    return "postgresql+psycopg://" + URL.split("://", 1)[1]


class SaSession(Driver):
    name = "sqlalchemy-session"

    def __init__(self) -> None:
        sa = pytest.importorskip("sqlalchemy")
        orm = pytest.importorskip("sqlalchemy.orm")
        self.sa = sa
        self.engine = sa.create_engine(sqlalchemy_url())
        self.s = orm.Session(self.engine)
        self.s.begin()

    def conn(self) -> Any:
        return self.s

    def execute(self, sql: str) -> None:
        self.s.execute(self.sa.text(sql))

    def commit(self) -> None:
        self.s.commit()

    def rollback(self) -> None:
        self.s.rollback()

    def close(self) -> None:
        self.s.close()
        self.engine.dispose()


class SaConnection(Driver):
    name = "sqlalchemy-connection"

    def __init__(self) -> None:
        sa = pytest.importorskip("sqlalchemy")
        self.sa = sa
        self.engine = sa.create_engine(sqlalchemy_url())
        self.c = self.engine.connect()
        self.t = self.c.begin()

    def conn(self) -> Any:
        return self.c

    def execute(self, sql: str) -> None:
        self.c.execute(self.sa.text(sql))

    def commit(self) -> None:
        self.t.commit()

    def rollback(self) -> None:
        self.t.rollback()

    def close(self) -> None:
        self.c.close()
        self.engine.dispose()


def django_setup() -> Any:
    """Configures Django once per process for the server in ``PEMBA_PG_URL``."""
    django = pytest.importorskip("django")
    from django.conf import settings  # type: ignore[import-untyped]

    if not settings.configured:
        u = urlsplit(URL)
        settings.configure(
            DATABASES={
                "default": {
                    "ENGINE": "django.db.backends.postgresql",
                    "NAME": u.path.lstrip("/") or "postgres",
                    "USER": u.username or "",
                    "PASSWORD": u.password or "",
                    "HOST": u.hostname or "localhost",
                    "PORT": str(u.port or 5432),
                }
            },
            USE_TZ=True,
        )
        django.setup()
    from django.db import connection, transaction  # type: ignore[import-untyped]

    return connection, transaction


class Django(Driver):
    name = "django"

    def __init__(self) -> None:
        self.connection, transaction = django_setup()
        self.atomic: AbstractContextManager[Any] = transaction.atomic()
        self.atomic.__enter__()
        self.transaction = transaction

    def conn(self) -> Any:
        return self.connection

    def execute(self, sql: str) -> None:
        with self.connection.cursor() as cur:
            cur.execute(sql)

    def commit(self) -> None:
        self.atomic.__exit__(None, None, None)

    def rollback(self) -> None:
        self.transaction.set_rollback(True)
        self.atomic.__exit__(None, None, None)

    def close(self) -> None:
        self.connection.close()


DRIVERS: list[type[Driver]] = [Psycopg3, Psycopg2, SaSession, SaConnection, Django]


@pytest.fixture(params=DRIVERS, ids=lambda d: d.name)
def driver(request: pytest.FixtureRequest) -> Iterator[Driver]:
    d: Driver = request.param()
    yield d
    d.close()


# ---- native engine ----


def test_the_native_engine_reads_and_writes(engine: pemba.Engine) -> None:
    rev = engine.write(touch=[ANNE])
    assert isinstance(rev, pemba.Revision)
    assert engine.check("document:readme#view@user:anne", at_least=rev)
    assert engine.lookup_resources("document", "view", "user:anne").ids == ["readme"]
    assert engine.read_schema() is not None
    stats = engine.gc(rev)
    assert stats.horizon == rev


def test_postgres_errors_are_mapped(engine: pemba.Engine) -> None:
    with pytest.raises(pemba.ConsistencyError) as exc:
        engine.check("document:readme#view@user:anne", at_exactly=10**9)
    assert exc.value.code == "REVISION_NOT_YET_AVAILABLE"


# ---- foreign mode, every driver ----


def test_commit_then_resolve(engine: pemba.Engine, schema: str, driver: Driver) -> None:
    driver.execute(f"INSERT INTO \"{schema}\".documents (id) VALUES ('readme')")
    pending = engine.write(touch=[ANNE], conn=driver.conn())
    assert isinstance(pending, pemba.PendingRevision)
    assert not pending.resolved
    with pytest.raises(pemba.RevisionPending) as exc:
        pending.revision()
    assert exc.value.code == "REVISION_PENDING" and isinstance(exc.value, pemba.ConsistencyError)
    assert not engine.check("document:readme#view@user:anne")

    driver.commit()
    rev = pending.revision()
    assert isinstance(rev, pemba.Revision) and pending.resolved
    assert engine.check("document:readme#view@user:anne", at_least=pending)
    assert engine.check("document:readme#view@user:anne", at_least=rev)
    assert documents(schema) == ["readme"]


def test_rollback_never_resolves(engine: pemba.Engine, schema: str, driver: Driver) -> None:
    driver.execute(f"INSERT INTO \"{schema}\".documents (id) VALUES ('readme')")
    pending = engine.write(touch=[ANNE], conn=driver.conn())
    driver.rollback()
    with pytest.raises(pemba.WriteRolledBack) as exc:
        pending.revision()
    assert exc.value.code == "WRITE_ROLLED_BACK"
    assert not engine.check("document:readme#view@user:anne")
    assert documents(schema) == []


def test_a_failed_write_leaves_the_transaction_usable(
    engine: pemba.Engine, schema: str, driver: Driver
) -> None:
    engine.write(touch=[ANNE])
    driver.execute(f"INSERT INTO \"{schema}\".documents (id) VALUES ('readme')")
    with pytest.raises(pemba.WriteConflictError):
        engine.write(create=[ANNE], conn=driver.conn())
    # Undone to its savepoint: the app's transaction goes on.
    pending = engine.write(touch=[BOB], conn=driver.conn())
    driver.commit()
    pending.revision()
    assert documents(schema) == ["readme"]
    assert engine.check("document:readme#view@user:bob")


# ---- guard rails ----


def test_psycopg_without_a_transaction_needs_allow_autocommit(
    engine: pemba.Engine,
) -> None:
    with psycopg.connect(URL, autocommit=True) as c:
        with pytest.raises(ValueError, match="no open transaction"):
            engine.write(touch=[ANNE], conn=c)
        # pemba opens and commits its own transaction, so the result is resolved.
        pending = engine.write(touch=[ANNE], conn=c, allow_autocommit=True)
        assert pending.resolved
        assert engine.check("document:readme#view@user:anne", at_least=pending)


def test_wrappers_without_a_transaction_are_refused(engine: pemba.Engine) -> None:
    sa = pytest.importorskip("sqlalchemy")
    orm = pytest.importorskip("sqlalchemy.orm")
    e = sa.create_engine(sqlalchemy_url())
    try:
        with orm.Session(e) as s:
            for allow in (False, True):
                with pytest.raises(ValueError, match="session.begin"):
                    engine.write(touch=[ANNE], conn=s, allow_autocommit=allow)
    finally:
        e.dispose()
    connection, _ = django_setup()
    with pytest.raises(ValueError, match="atomic"):
        engine.write(touch=[ANNE], conn=connection)


def test_a_failed_transaction_is_refused(engine: pemba.Engine) -> None:
    with psycopg.connect(URL) as c:
        with pytest.raises(psycopg.Error):
            c.execute("SELECT * FROM no_such_table_here")
        with pytest.raises(ValueError, match="failed transaction"):
            engine.write(touch=[ANNE], conn=c)


def test_another_database_is_refused(engine: pemba.Engine) -> None:
    db = f"pemba_py_{uuid.uuid4().hex[:12]}"
    with admin() as c:
        c.execute(f'CREATE DATABASE "{db}"')
    try:
        other = URL.rsplit("/", 1)[0] + f"/{db}"
        with psycopg.connect(other) as c:
            with pytest.raises(pemba.StorageError) as exc:
                engine.write(touch=[ANNE], conn=c)
            assert exc.value.code == "FOREIGN_CONNECTION_MISMATCH"
    finally:
        with admin() as c:
            c.execute(f'DROP DATABASE "{db}"')


def test_a_hostile_search_path_has_no_effect(engine: pemba.Engine, schema: str) -> None:
    with psycopg.connect(URL) as c:
        c.execute("CREATE TEMP TABLE pemba_relationships (x int)")
        c.execute("SET search_path = pg_temp, public")
        pending = engine.write(touch=[ANNE], conn=c)
        c.commit()
    assert engine.check("document:readme#view@user:anne", at_least=pending)


def test_a_percent_in_the_schema_name_is_not_a_placeholder() -> None:
    name = f"pemba%py_{uuid.uuid4().hex[:8]}"
    with admin() as c:
        c.execute(f'CREATE SCHEMA "{name}"')
    try:
        e = pemba.Engine.postgres(URL, schema=name, auto_migrate=True, pool_size=2)
        e.write_schema(SCHEMA)
        with psycopg.connect(URL) as c:
            pending = e.write(touch=[ANNE], conn=c)
            c.commit()
        assert e.check("document:readme#view@user:anne", at_least=pending)
    finally:
        with admin() as c:
            c.execute(f'DROP SCHEMA "{name}" CASCADE')


def test_async_connections_are_refused(engine: pemba.Engine) -> None:
    async def run() -> None:
        aconn = await psycopg.AsyncConnection.connect(URL)
        try:
            with pytest.raises(TypeError, match="async"):
                engine.write(touch=[ANNE], conn=aconn)
        finally:
            await aconn.close()

    run_async(run())


def test_other_objects_are_refused(engine: pemba.Engine) -> None:
    with pytest.raises(TypeError, match="psycopg"):
        engine.write(touch=[ANNE], conn=object())


# ---- SET CONSTRAINTS (docs/10-postgres.md §1.6.7) ----


def test_django_check_constraints_fires_early_and_warns_once(
    engine: pemba.Engine,
) -> None:
    connection, transaction = django_setup()
    pendings = []
    for user in ("anne", "bob"):
        with transaction.atomic():
            pendings.append(
                engine.write(touch=[f"document:readme#viewer@user:{user}"], conn=connection)
            )
            # Django's own: SET CONSTRAINTS ALL IMMEDIATE, then ALL DEFERRED.
            connection.check_constraints()
    with pytest.warns(pemba.PembaWarning, match="before COMMIT"):
        pendings[0].revision()
    with warnings.catch_warnings():
        warnings.simplefilter("error", pemba.PembaWarning)
        pendings[1].revision()  # once per engine


# ---- AsyncSession.run_sync (docs/10-postgres.md §2.4) ----


def test_async_session_run_sync(engine: pemba.Engine, schema: str) -> None:
    pytest.importorskip("greenlet")
    asa = pytest.importorskip("sqlalchemy.ext.asyncio")
    sa = pytest.importorskip("sqlalchemy")

    async def run() -> pemba.PendingRevision:
        e = asa.create_async_engine(sqlalchemy_url())
        try:
            async with asa.AsyncSession(e) as s:
                async with s.begin():
                    await s.execute(
                        sa.text(f"INSERT INTO \"{schema}\".documents (id) VALUES ('readme')")
                    )
                    pending: pemba.PendingRevision = await s.run_sync(
                        lambda sync: engine.write(touch=[ANNE], conn=sync)
                    )
                return pending
        finally:
            await e.dispose()

    pending = run_async(run())
    assert engine.check("document:readme#view@user:anne", at_least=pending)
    assert documents(schema) == ["readme"]
