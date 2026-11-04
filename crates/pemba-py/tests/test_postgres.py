"""``Engine.postgres`` and foreign mode on Postgres (T5.9, ``docs/10-postgres.md`` §2.2–§2.4,
§4 item 6).

Needs ``PEMBA_PG_URL`` (a ``postgres://`` URL to a server the tests may create schemas and
databases on); without it the module reports "not run", unless ``PEMBA_PG_REQUIRED`` is
set. Each driver's tests run only where that driver is installed (psycopg 3, psycopg2,
SQLAlchemy 2, Django).
"""

from __future__ import annotations

import asyncio
import importlib
import os
import threading
import uuid
import warnings
from collections.abc import Coroutine, Iterator
from contextlib import AbstractContextManager
from typing import Any, TypeVar
from urllib.parse import urlsplit

import pytest

import pemba
from conftest import SCHEMA
from pemba import aio

URL = os.environ.get("PEMBA_PG_URL", "")
if not URL:
    if os.environ.get("PEMBA_PG_REQUIRED"):
        raise RuntimeError("PEMBA_PG_REQUIRED is set but PEMBA_PG_URL is not")
    pytest.skip("not run: set PEMBA_PG_URL (docs/10-postgres.md §4)", allow_module_level=True)

# Required service runs must fail rather than silently skip the tutorial without its driver.
psycopg = (
    importlib.import_module("psycopg")
    if os.environ.get("PEMBA_PG_REQUIRED")
    else pytest.importorskip("psycopg")
)

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
    from django.conf import settings

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
    from django.db import connection, transaction

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


def test_a_bulk_create_conflict_undoes_only_pembas_savepoint(
    engine: pemba.Engine, schema: str, driver: Driver
) -> None:
    # One statement for the Creates fails (OQ-056); pemba reruns per op inside its own
    # savepoint. The app's rows before and after, and the transaction, are untouched.
    carl = "document:readme#viewer@user:carl"
    engine.write(touch=[carl])
    driver.execute(f"INSERT INTO \"{schema}\".documents (id) VALUES ('readme')")
    with pytest.raises(pemba.WriteConflictError) as exc:
        engine.write(create=[ANNE, carl], touch=[BOB], conn=driver.conn())
    assert "carl" in str(exc.value)
    driver.execute(f"INSERT INTO \"{schema}\".documents (id) VALUES ('notes')")
    pending = engine.write(touch=["document:notes#viewer@user:dora"], conn=driver.conn())
    driver.commit()
    pending.revision()
    assert documents(schema) == ["notes", "readme"]
    assert not engine.check("document:readme#view@user:anne")
    assert not engine.check("document:readme#view@user:bob")
    assert engine.check("document:notes#view@user:dora")


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


def test_tls_verifies_the_server_certificate() -> None:
    """T5.10: the extension speaks TLS (rustls) and fails closed on an untrusted server.

    Needs ``PEMBA_PG_TLS_URL``: a server with TLS on and a certificate the platform does
    not trust (the image's self-signed one), e.g. ``postgres -c ssl=on -c
    ssl_cert_file=/etc/ssl/certs/ssl-cert-snakeoil.pem -c
    ssl_key_file=/etc/ssl/private/ssl-cert-snakeoil.key``.

    CI's servers trust every connection, so the DSN has no password to leak, and this test
    does not check for one. That a password never reaches an error message is tested in
    Rust: ``bad_configuration_is_an_invalid_request`` (``pemba-store-postgres``,
    ``tests/migrate.rs``) for a malformed DSN; the TLS error here is built from the
    driver's, IO and TLS causes only, which never quote the DSN (``store_err``).
    """
    tls_url = os.environ.get("PEMBA_PG_TLS_URL")
    if not tls_url:
        pytest.skip("not run: set PEMBA_PG_TLS_URL")
    sep = "&" if "?" in tls_url else "?"
    with pytest.raises(pemba.StorageError) as exc:
        pemba.Engine.postgres(tls_url + sep + "sslmode=require", auto_migrate=True)
    assert "TLS handshake" in str(exc.value) and "certificate" in str(exc.value), exc.value
    # Without TLS the same server is reachable: the failure above was the certificate.
    pemba.Engine.postgres(tls_url + sep + "sslmode=disable", auto_migrate=True)


# ---- pinned_certificate_pem (ADR-017, docs/guides/postgres-certificates.md) ----

# A public certificate for `localhost`, as the TLS servers' are, with another key. Its private
# key was discarded when it was generated.
OTHER_CERTIFICATE = """-----BEGIN CERTIFICATE-----
MIIBkTCCATigAwIBAgIUPFOpirP+FwOTPBNB+pUNBJlCK4IwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTEwNDExMzkwMloYDzIxMjYxMDEx
MTEzOTAyWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAAS8NdYSiWHTzg4Eb15dlQlkIEjtq6FZNnJYc/xg1IMSbph3ZnrR8vpY
YHwbdcGnRMjdJyY2bINa9DYzzIvcE1cMo2YwZDAdBgNVHQ4EFgQU4iHlRzFyMR8M
6c8C6uNR7YylkgEwHwYDVR0jBBgwFoAU4iHlRzFyMR8M6c8C6uNR7YylkgEwFAYD
VR0RBA0wC4IJbG9jYWxob3N0MAwGA1UdEwEB/wQCMAAwCgYIKoZIzj0EAwIDRwAw
RAIgQa4LojC5G8zBoKojvLOdC3vZ5p+pSl5rOuYl3oaMdMsCIHKHABQOfU8PHQGd
fMuX2UPAyM7eopq5TZLnMxGRqQ4n
-----END CERTIFICATE-----
"""


def tls_server(url_var: str, cert_var: str) -> tuple[str, str]:
    """The server's URL and the PEM of its certificate, or "not run" without them."""
    url = os.environ.get(url_var)
    cert_file = os.environ.get(cert_var)
    if not url or not cert_file:
        pytest.skip(f"not run: set {url_var} and {cert_var}")
    with open(cert_file, encoding="ascii") as f:
        return url, f.read()


def with_param(url: str, param: str) -> str:
    return url + ("&" if "?" in url else "?") + param


def test_ca_true_certificate_without_a_pin_raises_what_needle_logged() -> None:
    url, pem = tls_server("PEMBA_PG_TLS_CA_LEAF_URL", "PEMBA_PG_TLS_CA_LEAF_CERT_FILE")
    with pytest.raises(pemba.StorageError) as refused:
        pemba.Engine.postgres(url)
    assert str(refused.value) == (
        "database error: error performing TLS handshake: invalid peer certificate: "
        "Other(OtherError(CaUsedAsEndEntity))"
    )
    assert pem not in str(refused.value)


@pytest.mark.parametrize(
    ("url_var", "cert_var", "at_ip"),
    [
        ("PEMBA_PG_TLS_URL", "PEMBA_PG_TLS_CERT_FILE", False),
        # The certificates name localhost, so here the name differs: the pin must still connect.
        ("PEMBA_PG_TLS_URL", "PEMBA_PG_TLS_CERT_FILE", True),
        # Railway's image: CA:TRUE, issued by a private root, over SCRAM.
        ("PEMBA_PG_TLS_CA_LEAF_URL", "PEMBA_PG_TLS_CA_LEAF_CERT_FILE", True),
    ],
)
def test_pinned_certificate_works_end_to_end(url_var: str, cert_var: str, at_ip: bool) -> None:
    url, pem = tls_server(url_var, cert_var)
    if at_ip:
        url = url.replace("@localhost:", "@127.0.0.1:")
    name = f"pemba_pin_{uuid.uuid4().hex[:12]}"
    with psycopg.connect(url, autocommit=True) as c:
        c.execute(f'CREATE SCHEMA "{name}"')
    try:
        engine = pemba.Engine.postgres(
            url, schema=name, auto_migrate=True, pinned_certificate_pem=pem
        )
        engine.write_schema(SCHEMA)
        engine.write(touch=[ANNE])
        assert engine.check("document:readme", "view", "user:anne")
        assert not engine.check("document:readme", "view", "user:bob")

        # A foreign write runs on the app's own connection, with the app's own TLS settings.
        with psycopg.connect(url) as app:
            pending = engine.write(touch=[BOB], conn=app)
            app.commit()
        assert engine.check("document:readme", "view", "user:bob", at_least=pending)
        engine.close()
    finally:
        with psycopg.connect(url, autocommit=True) as c:
            c.execute(f'DROP SCHEMA "{name}" CASCADE')


def test_pinned_certificate_refuses_a_different_certificate() -> None:
    url, _ = tls_server("PEMBA_PG_TLS_URL", "PEMBA_PG_TLS_CERT_FILE")
    with pytest.raises(pemba.StorageError) as exc:
        pemba.Engine.postgres(url, pinned_certificate_pem=OTHER_CERTIFICATE)
    assert "TLS handshake" in str(exc.value), exc.value
    assert "not the pinned certificate" in str(exc.value), exc.value


def test_pinned_certificate_none_is_the_platform_trust_store() -> None:
    url, _ = tls_server("PEMBA_PG_TLS_URL", "PEMBA_PG_TLS_CERT_FILE")
    with pytest.raises(pemba.StorageError) as exc:
        pemba.Engine.postgres(url, pinned_certificate_pem=None)
    assert "TLS handshake" in str(exc.value) and "certificate" in str(exc.value), exc.value


@pytest.mark.parametrize(
    ("pem", "expected"),
    [
        ("", "holds no certificate"),
        ("not a certificate", "holds no certificate"),
        ("-----BEGIN CERTIFICATE-----\nnot*base64\n-----END CERTIFICATE-----\n", "not valid PEM"),
        ("-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n", "X.509"),
        (OTHER_CERTIFICATE + OTHER_CERTIFICATE, "more than one"),
    ],
)
def test_pinned_certificate_bad_pem_fails_at_open(pem: str, expected: str) -> None:
    # Nothing listens on the port, so an attempt to connect would be a StorageError.
    dsn = "host=127.0.0.1 port=1 user=u password=dsn-password-value"  # ggignore: fake password; the test checks it never reaches an error message
    with pytest.raises(pemba.InvalidRequestError, match=expected) as exc:
        pemba.Engine.postgres(dsn, pinned_certificate_pem=pem)
    assert "dsn-password-value" not in str(exc.value)
    assert "BEGIN" not in str(exc.value) and "aGVsbG8" not in str(exc.value)


def test_pinned_certificate_with_sslmode_disable_is_refused() -> None:
    with pytest.raises(pemba.InvalidRequestError, match="sslmode=disable"):
        pemba.Engine.postgres(
            "host=127.0.0.1 port=1 user=u sslmode=disable",
            pinned_certificate_pem=OTHER_CERTIFICATE,
        )


def test_pinned_certificate_sslmode_require_connects() -> None:
    url, pem = tls_server("PEMBA_PG_TLS_URL", "PEMBA_PG_TLS_CERT_FILE")
    pemba.Engine.postgres(
        with_param(url, "sslmode=require"), auto_migrate=True, pinned_certificate_pem=pem
    ).close()


def test_pinned_certificate_is_keyword_only() -> None:
    with pytest.raises(TypeError):
        pemba.Engine.postgres("host=127.0.0.1", None, 8, False, 50, OTHER_CERTIFICATE)  # type: ignore[call-arg]


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


def test_transactional_postgres_tutorial(schema: str) -> None:
    import runpy
    from pathlib import Path

    root = Path(__file__).resolve().parents[3]
    scope = runpy.run_path(str(root / "examples" / "transactional_postgres.py"), run_name="example")
    scope["run"](URL, schema)
    assert documents(schema) == ["committed"]
    e = pemba.Engine.postgres(URL, schema=schema)
    assert e.check("document:committed", "view", "user:anne") is True
    assert e.check("document:rolled_back", "view", "user:anne") is False


# ---- close (06 §1.2) ----


def test_database_can_be_dropped_right_after_close() -> None:
    """Without gc.collect() or WITH (FORCE), holding what used to keep an engine alive."""
    database = f"pemba_py_{uuid.uuid4().hex[:12]}"
    with admin() as c:
        c.execute(f'CREATE DATABASE "{database}"')
    url = psycopg.conninfo.make_conninfo(URL, dbname=database)
    engine = pemba.Engine.postgres(url, schema="public", auto_migrate=True, pool_size=4)
    engine.write_schema(SCHEMA)
    with psycopg.connect(url) as app:
        pending = engine.write(touch=[ANNE], conn=app)
    assert engine.check("document:readme#view@user:anne", at_least=pending)
    resources = engine.iter_resources("document", "view", "user:anne")

    threads = [
        threading.Thread(target=lambda: engine.check("document:readme", "view", "user:anne"))
        for _ in range(8)
    ]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    engine.close()

    with admin() as c:
        c.execute(f'DROP DATABASE "{database}"')
    # Both still there, and both say so rather than answer.
    assert pending.resolved
    with pytest.raises(pemba.EngineClosedError):
        next(resources)


def test_context_manager_on_postgres(schema: str) -> None:
    with pemba.Engine.postgres(URL, schema=schema, auto_migrate=True) as engine:
        engine.write_schema(SCHEMA)
        engine.write(touch=[ANNE])
    assert engine.closed
    with pytest.raises(pemba.EngineClosedError):
        engine.check("document:readme", "view", "user:anne")


def test_close_leaves_the_applications_transaction_working(engine: pemba.Engine, schema: str) -> None:
    with psycopg.connect(URL) as app:
        app.execute(f"""INSERT INTO "{schema}".documents VALUES ('readme')""")
        pending = engine.write(touch=[ANNE], conn=app)

        engine.close()

        with pytest.raises(pemba.EngineClosedError):
            engine.write(touch=[BOB], conn=app)
        with pytest.raises(pemba.EngineClosedError):
            pending.revision()
    # The block committed: the outcome was the application's, not the engine's.
    assert documents(schema) == ["readme"]
    with pemba.Engine.postgres(URL, schema=schema) as reopened:
        assert reopened.check("document:readme", "view", "user:anne")
        assert not reopened.check("document:readme", "view", "user:bob")


def test_closed_engine_sends_nothing_on_the_applications_connection(engine: pemba.Engine) -> None:
    engine.close()
    with psycopg.connect(URL, autocommit=True) as app:
        with pytest.raises(pemba.EngineClosedError):
            engine.write(touch=[ANNE], conn=app, allow_autocommit=True)
        assert app.info.transaction_status == psycopg.pq.TransactionStatus.IDLE


def test_readers_racing_close_on_postgres(engine: pemba.Engine) -> None:
    engine.write(touch=[ANNE])
    start = threading.Barrier(9)
    bad: list[object] = []

    def read() -> None:
        start.wait()
        while True:
            try:
                answer = engine.check("document:readme", "view", "user:anne")
            except pemba.EngineClosedError:
                return
            except BaseException as e:
                bad.append(e)
                return
            if answer is not True:
                bad.append(answer)
                return

    threads = [threading.Thread(target=read) for _ in range(8)]
    for t in threads:
        t.start()
    start.wait()
    engine.close()
    for t in threads:
        t.join()
    assert not bad


def test_write_async_and_aio_close(engine: pemba.Engine, schema: str) -> None:
    async def run() -> None:
        async with await psycopg.AsyncConnection.connect(URL) as app:
            pending = await engine.write_async(touch=[ANNE], conn=app)
            await aio.close(engine)
            with pytest.raises(pemba.EngineClosedError):
                await engine.write_async(touch=[BOB], conn=app)
            with pytest.raises(pemba.EngineClosedError):
                await aio.resolve(pending)
            # Still the application's transaction, and it commits as this block ends.

    run_async(run())
    with pemba.Engine.postgres(URL, schema=schema) as reopened:
        assert reopened.check("document:readme", "view", "user:anne")
        assert not reopened.check("document:readme", "view", "user:bob")
