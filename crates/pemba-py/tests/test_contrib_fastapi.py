"""``pemba.contrib.fastapi`` through FastAPI's real request path (``docs/06-api.md`` §2.4).

Needs FastAPI and httpx. Without them the module reports "not run", unless
``PEMBA_FASTAPI_REQUIRED`` is set: then a missing package fails at collection, so CI cannot
go green by skipping. The ``postgres`` store parameter follows the same rule with
``PEMBA_PG_URL`` and ``PEMBA_PG_REQUIRED``.
"""

from __future__ import annotations

import asyncio
import importlib
import inspect
import logging
import os
import sqlite3
import threading
import uuid
from collections.abc import Callable, Coroutine, Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast
from urllib.parse import quote

import pytest

if os.environ.get("PEMBA_FASTAPI_REQUIRED"):
    importlib.import_module("fastapi")
    importlib.import_module("httpx")
else:
    pytest.importorskip("fastapi")
    pytest.importorskip("httpx")

import httpx
from conftest import SCHEMA
from fastapi import Depends, FastAPI, HTTPException, Request, WebSocket
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient

import pemba
from pemba import _pemba  # type: ignore[attr-defined]
from pemba.contrib import fastapi as fastapi_helper
from pemba.contrib.fastapi import Authz
from test_readme_example import ROOT, python_block

DENIED = {"detail": "Forbidden"}
UNAVAILABLE = {"detail": "Authorization is temporarily unavailable"}
BAD_ID = {"detail": "Invalid resource id"}
NO_SUBJECT = {"detail": "Authentication required"}
ROUTES = ["sync", "async"]

PG_URL = os.environ.get("PEMBA_PG_URL", "")


@dataclass
class Store:
    """A real engine, and a way to make its store fail underneath it."""

    kind: str
    engine: pemba.Engine
    sever: Callable[[], None]
    schema: str = ""


def header_subject(request: Request) -> str:
    return f"user:{request.headers['x-user']}"


@dataclass
class Harness:
    """An app with one gated route of each kind, counting how often a body ran."""

    engine: Any
    subject: Any = header_subject
    permission: str = "view"
    at_least: Any = None
    authz_options: dict[str, Any] = field(default_factory=dict)
    ran: list[str] = field(default_factory=list)
    causes: list[BaseException | None] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.app = FastAPI()
        authz = Authz(cast(pemba.Engine, self.engine), subject=self.subject, **self.authz_options)
        gate = Depends(authz.require("document", self.permission, path_param="doc_id", at_least=self.at_least))

        def sync_route(doc_id: str) -> dict[str, str]:
            self.ran.append(doc_id)
            return {"id": doc_id}

        async def async_route(doc_id: str) -> dict[str, str]:
            self.ran.append(doc_id)
            return {"id": doc_id}

        async def keep_cause(request: Request, exc: Exception) -> JSONResponse:
            assert isinstance(exc, HTTPException)
            self.causes.append(exc.__cause__)
            return JSONResponse({"detail": exc.detail}, status_code=exc.status_code, headers=exc.headers)

        self.app.add_exception_handler(HTTPException, keep_cause)
        self.app.add_api_route("/sync/{doc_id}", sync_route, dependencies=[gate])
        self.app.add_api_route("/async/{doc_id}", async_route, dependencies=[gate])
        self.client = TestClient(self.app, raise_server_exceptions=False, headers={"x-user": "anne"})

    def get(self, route: str, doc_id: str, user: str = "anne") -> httpx.Response:
        # Percent-encoded so that `#` and friends reach the server as part of the path.
        return self.raw_get(f"/{route}/{quote(doc_id, safe='')}", user)

    def raw_get(self, path: str, user: str = "anne") -> httpx.Response:
        # Decodes the path once, as a server does; Starlette's TestClient decodes it twice.
        async def main() -> httpx.Response:
            transport = httpx.ASGITransport(app=self.app, raise_app_exceptions=False)
            async with httpx.AsyncClient(transport=transport, base_url="http://test") as client:
                return await client.get(path, headers={"x-user": user})

        return asyncio.run(main())


class StubEngine:
    """Records calls, then raises ``error`` or returns ``answer``."""

    def __init__(self, answer: Any = True, error: BaseException | None = None) -> None:
        self.answer = answer
        self.error = error
        self.calls: list[tuple[tuple[Any, ...], dict[str, Any]]] = []

    def check(self, *args: Any, **kwargs: Any) -> Any:
        self.calls.append((args, kwargs))
        if self.error is not None:
            raise self.error
        return self.answer


def seed(engine: pemba.Engine) -> None:
    engine.write_schema(SCHEMA)
    engine.write(touch=[
        "document:readme#viewer@user:anne",
        "document:public#viewer@user:*",
    ])


@pytest.fixture(params=["memory", "sqlite", "postgres"])
def bare_store(request: pytest.FixtureRequest, tmp_path: Path) -> Iterator[Store]:
    """A real engine with no schema written, on each store."""
    if request.param == "memory":
        def no_failure() -> None:
            pytest.skip("the memory store cannot fail underneath the engine")
        yield Store("memory", pemba.Engine.memory(), no_failure)
    elif request.param == "sqlite":
        path = tmp_path / "authz.db"

        def drop_tables() -> None:
            conn = sqlite3.connect(path)
            tables = conn.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
            ).fetchall()
            for (name,) in tables:
                conn.execute(f'DROP TABLE "{name}"')
            conn.commit()
            conn.close()
        yield Store("sqlite", pemba.Engine.sqlite(path), drop_tables)
    else:
        if not PG_URL:
            if os.environ.get("PEMBA_PG_REQUIRED"):
                raise RuntimeError("PEMBA_PG_REQUIRED is set but PEMBA_PG_URL is not")
            pytest.skip("not run: set PEMBA_PG_URL (docs/10-postgres.md §4)")
        psycopg = importlib.import_module("psycopg")
        name = f"pemba_fastapi_{uuid.uuid4().hex[:12]}"
        with psycopg.connect(PG_URL, autocommit=True) as admin:
            admin.execute(f'CREATE SCHEMA "{name}"')

        def drop_schema() -> None:
            with psycopg.connect(PG_URL, autocommit=True) as admin:
                admin.execute(f'DROP SCHEMA "{name}" CASCADE')
        yield Store(
            "postgres",
            pemba.Engine.postgres(PG_URL, schema=name, auto_migrate=True, pool_size=4),
            drop_schema,
            name,
        )
        with psycopg.connect(PG_URL, autocommit=True) as admin:
            admin.execute(f'DROP SCHEMA IF EXISTS "{name}" CASCADE')


@pytest.fixture
def store(bare_store: Store) -> Store:
    seed(bare_store.engine)
    return bare_store


# Allow, deny and the configured deny


@pytest.mark.parametrize("route", ROUTES)
def test_allow_runs_the_body_and_deny_does_not(store: Store, route: str) -> None:
    app = Harness(store.engine)
    allowed = app.get(route, "readme")
    assert (allowed.status_code, allowed.json()) == (200, {"id": "readme"})
    assert app.ran == ["readme"]
    denied = app.get(route, "readme", user="bob")
    assert (denied.status_code, denied.json()) == (403, DENIED)
    assert app.ran == ["readme"]


@pytest.mark.parametrize("route", ROUTES)
def test_configured_deny_response(store: Store, route: str) -> None:
    app = Harness(store.engine, authz_options={"deny_status": 404, "deny_detail": "Not found"})
    denied = app.get(route, "readme", user="bob")
    assert (denied.status_code, denied.json()) == (404, {"detail": "Not found"})
    assert app.get(route, "readme").status_code == 200
    assert app.ran == ["readme"]


def documented_subject(request: Request) -> str | None:
    """The subject callable of ``06`` §2.4, word for word."""
    user_id = request.state.user_id  # Set by your authentication; None when there is no user.
    return None if user_id is None else f"user:{user_id}"


def test_the_form_in_the_spec_works(store: Store) -> None:
    """``06`` §2.4 as written: a ``_`` parameter, and ``request.state`` for the subject."""
    app = FastAPI()
    authz = Authz(store.engine, subject=documented_subject)
    ran: list[str] = []

    def get_doc(doc_id: str, _: None = Depends(authz.require("document", "view", path_param="doc_id"))) -> str:
        ran.append(doc_id)
        return doc_id

    async def with_user(request: Request, call_next: Any) -> Any:
        request.state.user_id = request.headers.get("x-user")
        return await call_next(request)

    app.add_api_route("/documents/{doc_id}", get_doc)
    app.middleware("http")(with_user)
    client = TestClient(app, raise_server_exceptions=False)
    assert client.get("/documents/readme", headers={"x-user": "anne"}).status_code == 200
    assert client.get("/documents/readme", headers={"x-user": "bob"}).status_code == 403
    assert ran == ["readme"]
    # Anonymous, on a `user:*` document that any real subject could read: 401, never an allow.
    assert store.engine.check("document:public", "view", "user:bob")
    anonymous = client.get("/documents/public")
    assert (anonymous.status_code, anonymous.json()) == (401, NO_SUBJECT)
    assert ran == ["readme"]


@pytest.mark.parametrize("answer", [False, None, 0, 1, "yes", [True]])
def test_only_a_real_true_allows(answer: Any) -> None:
    app = Harness(StubEngine(answer=answer))
    assert app.get("sync", "readme").status_code == 403
    assert app.ran == []


@pytest.mark.parametrize("status", [100, 200, 204, 302, 399, 500, 503, 600])
def test_deny_status_must_be_a_client_error(status: int) -> None:
    with pytest.raises(ValueError, match="deny_status"):
        Authz(cast(pemba.Engine, StubEngine()), subject=header_subject, deny_status=status)


@pytest.mark.parametrize("status", [True, "403", 403.0, None])
def test_deny_status_must_be_an_int(status: Any) -> None:
    with pytest.raises(TypeError, match="deny_status"):
        Authz(cast(pemba.Engine, StubEngine()), subject=header_subject, deny_status=status)


@pytest.mark.parametrize("detail", [None, 403, {"a": 1}, b"no"])
def test_deny_detail_must_be_a_string(detail: Any) -> None:
    with pytest.raises(TypeError, match="deny_detail"):
        Authz(cast(pemba.Engine, StubEngine()), subject=header_subject, deny_detail=detail)


def test_deny_configuration_reaches_no_error() -> None:
    options = {"deny_status": 404, "deny_detail": "Not found"}
    failing = Harness(StubEngine(error=pemba.StorageError("down", "STORE_ERROR")), authz_options=options)
    response = failing.get("sync", "readme")
    assert (response.status_code, response.json()) == (503, UNAVAILABLE)
    bad_input = Harness(StubEngine(), authz_options=options)
    response = bad_input.get("sync", "*")
    assert (response.status_code, response.json()) == (422, BAD_ID)


# Errors stay errors


SECRET = "SELECT * FROM pemba_rel; postgres://admin:hunter2@db.internal:5432/app"
TRANSIENT_CODES = {"STORE_ERROR", "TRANSACTION_CONFLICT"}
# Its own row in the table: a client can send the bound, so it is covered further down.
BOUND_AHEAD = "REVISION_NOT_YET_AVAILABLE"
# Rust's Parse is a built-in ValueError, not a PembaError.
PERMANENT_CODES = sorted(set(_pemba.ERROR_CODES) - TRANSIENT_CODES - {BOUND_AHEAD, "PARSE"})


def error_for(code: str) -> pemba.PembaError:
    cls: type[pemba.PembaError] = getattr(pemba, _pemba._exception_class_name(code))
    return cls(SECRET, code)


def test_the_allowlist_names_real_codes() -> None:
    """A renamed code would otherwise turn every store outage into a 500, silently."""
    assert TRANSIENT_CODES <= set(_pemba.ERROR_CODES)
    assert fastapi_helper._TRANSIENT_CODES == TRANSIENT_CODES
    assert BOUND_AHEAD in _pemba.ERROR_CODES


@pytest.mark.parametrize("route", ROUTES)
@pytest.mark.parametrize("code", sorted(TRANSIENT_CODES))
def test_a_transient_error_is_a_503_that_exposes_nothing(
    code: str, route: str, caplog: pytest.LogCaptureFixture,
) -> None:
    error = error_for(code)
    app = Harness(StubEngine(error=error))
    with caplog.at_level(logging.ERROR, logger="pemba.contrib.fastapi"):
        response = app.get(route, "readme")
    assert (response.status_code, response.json()) == (503, UNAVAILABLE)
    assert "hunter2" not in response.text and "SELECT" not in response.text
    assert app.ran == []
    # The cause is for the server: on the exception chain and in the log, with its message.
    assert app.causes == [error]
    assert [record.exc_info[1] for record in caplog.records if record.exc_info] == [error]
    assert "hunter2" in caplog.text


@pytest.mark.parametrize("route", ROUTES)
@pytest.mark.parametrize("code", [*PERMANENT_CODES, "A_CODE_ADDED_LATER"])
def test_every_other_error_is_a_500_that_exposes_nothing(code: str, route: str) -> None:
    """Retrying cannot fix these, so none may look like an outage, or like a deny."""
    error = error_for(code) if code in PERMANENT_CODES else pemba.PembaError(SECRET, code)
    app = Harness(StubEngine(error=error))
    response = app.get(route, "readme")
    assert response.status_code == 500
    assert "hunter2" not in response.text and "SELECT" not in response.text
    assert app.ran == []


@pytest.mark.parametrize("code", ["MAX_DEPTH_EXCEEDED", "FANOUT_EXCEEDED", "READ_LIMIT_EXCEEDED"])
def test_a_limit_error_is_a_500(code: str) -> None:
    """Only ``max_depth`` can be configured from Python, so the others are stubbed."""
    app = Harness(StubEngine(error=pemba.EvaluationLimitError(SECRET, code)))
    assert app.get("sync", "readme").status_code == 500
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_no_schema_is_a_500(bare_store: Store, route: str) -> None:
    app = Harness(bare_store.engine)
    assert app.get(route, "readme").status_code == 500
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_unknown_permission_is_a_500(store: Store, route: str) -> None:
    app = Harness(store.engine, permission="missing")
    assert app.get(route, "readme").status_code == 500
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_evaluation_limit_is_a_500(tmp_path: Path, route: str) -> None:
    engine = pemba.Engine.sqlite(tmp_path / "deep.db", max_depth=2)
    engine.write_schema(SCHEMA)
    engine.write(touch=[
        "document:deep#viewer@group:one#member",
        "group:one#member@group:two#member",
        "group:two#member@group:three#member",
        "group:three#member@user:anne",
    ])
    app = Harness(engine)
    assert app.get(route, "deep").status_code == 500
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_a_store_failure_is_a_503_not_a_deny(
    store: Store, route: str, caplog: pytest.LogCaptureFixture,
) -> None:
    app = Harness(store.engine)
    assert app.get(route, "readme").status_code == 200
    store.sever()
    with caplog.at_level(logging.ERROR, logger="pemba.contrib.fastapi"):
        response = app.get(route, "readme")
    assert (response.status_code, response.json()) == (503, UNAVAILABLE)
    assert app.ran == ["readme"]
    assert isinstance(app.causes[-1], pemba.StorageError)
    assert any(record.exc_info and record.exc_info[1] is app.causes[-1] for record in caplog.records)


@pytest.mark.parametrize("error", [RuntimeError("bug"), KeyError("bug"), ValueError("bug"), TypeError("bug")])
def test_an_unexpected_engine_exception_is_a_500(error: Exception) -> None:
    """Neither a deny nor an allow, and a ``ValueError`` here does not blame the id."""
    app = Harness(StubEngine(error=error))
    response = app.get("sync", "readme")
    assert response.status_code == 500
    assert "bug" not in response.text
    assert app.ran == []


# Hostile path values

HOSTILE_IDS = [
    "*",
    "#",
    "@",
    ":",
    "a#view",
    "a@user:anne",
    "a:b",
    "readme#view@user:anne",
    "public#viewer@user:*",
    "x" * 1025,
    "x" * 60_000,
    "é",
    "文書",
    "\U0001f512",
    "a b",
    " readme",
    "readme ",
    "readme\n",
    "readme\r\nx-user: anne",
    "\x00",
    "readme\x00",
    "%2A",
    "%23",
    "read%6De",
    "'; DROP TABLE pemba_rel; --",
    "../readme",
]


@pytest.mark.parametrize("route", ROUTES)
def test_hostile_ids_never_reach_the_body_or_look_like_an_outage(store: Store, route: str) -> None:
    """``anne`` can view ``readme`` and everyone ``public``; no hostile value may borrow that."""
    app = Harness(store.engine)
    for doc_id in HOSTILE_IDS:
        for user in ("anne", "bob"):
            response = app.get(route, doc_id, user=user)
            # `../readme` is the only one with a `/`: encoded, it never matches the route.
            expected = (404,) if "/" in doc_id else (422,)
            assert response.status_code in expected, repr(doc_id)
            if response.status_code == 422:
                assert response.json() == BAD_ID, repr(doc_id)
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_encoded_values(store: Store, route: str) -> None:
    app = Harness(store.engine)
    for raw in ("%FF", "%C3%28", "%ED%A0%80", "a%00", "%2A", "%23", "%40", "%3A", "%252A", "%2523", "read%256De"):
        response = app.raw_get(f"/{route}/{raw}")
        assert (response.status_code, response.json()) == (422, BAD_ID), raw
    assert app.ran == []
    # One layer of encoding is the same id: the check and the body see the same value.
    assert app.raw_get(f"/{route}/read%6De").status_code == 200
    assert app.ran == ["readme"]
    assert app.raw_get(f"/{route}/read%6De", user="bob").status_code == 403
    assert app.ran == ["readme"]


def test_hostile_ids_never_reach_the_engine() -> None:
    engine = StubEngine()
    app = Harness(engine)
    for doc_id in HOSTILE_IDS:
        assert app.get("sync", doc_id).status_code in (404, 422)
    assert engine.calls == []
    assert app.ran == []


def test_the_engine_gets_the_three_argument_form() -> None:
    """Never the single tuple string, which would let an id supply a subject."""
    engine = StubEngine()
    assert Harness(engine).get("sync", "a-valid_id.1=+|").status_code == 200
    assert engine.calls == [(("document:a-valid_id.1=+|", "view", "user:anne"), {"at_least": None})]


@pytest.mark.parametrize("route", ROUTES)
def test_an_odd_but_valid_id_gets_an_ordinary_answer(store: Store, route: str) -> None:
    app = Harness(store.engine)
    for doc_id in ("x" * 1024, "...", "a=b+c|d.e-f_g", "None"):
        response = app.get(route, doc_id)
        assert (response.status_code, response.json()) == (403, DENIED), repr(doc_id)
    assert app.ran == []


def test_path_convertors(store: Store) -> None:
    """Starlette converts these values before the helper sees them."""
    ran: list[str] = []
    app = FastAPI()
    authz = Authz(store.engine, subject=header_subject)

    def by_path(doc_id: str) -> str:
        ran.append(doc_id)
        return doc_id

    def by_int(doc_id: int) -> int:
        ran.append(str(doc_id))
        return doc_id

    gate = Depends(authz.require("document", "view", path_param="doc_id"))
    app.add_api_route("/path/{doc_id:path}", by_path, dependencies=[gate])
    app.add_api_route("/int/{doc_id:int}", by_int, dependencies=[gate])
    store.engine.write(touch=["document:a/b#viewer@user:anne", "document:7#viewer@user:anne"])
    client = TestClient(app, raise_server_exceptions=False, headers={"x-user": "anne"})
    assert client.get("/path/a/b").status_code == 200
    assert client.get("/int/7").status_code == 200
    assert client.get("/int/8").status_code == 403
    assert client.get("/path/c/d").status_code == 403
    # An empty value is not a missing one: it must not skip the check.
    empty = client.get("/path/")
    assert (empty.status_code, empty.json()) == (422, BAD_ID)
    assert client.get("/path/a:b").status_code == 422
    assert ran == ["a/b", "7"]


def test_the_check_reads_the_raw_path_value_not_the_validated_one() -> None:
    """Why the guide says to keep a gated parameter a plain ``str`` or a path convertor."""
    engine = StubEngine()
    app = FastAPI()
    gate = Depends(Authz(cast(pemba.Engine, engine), subject=header_subject).require(
        "document", "view", path_param="doc_id"))

    def annotated(doc_id: int) -> int:
        return doc_id

    def converted(doc_id: int) -> int:
        return doc_id

    app.add_api_route("/annotated/{doc_id}", annotated, dependencies=[gate])
    app.add_api_route("/converted/{doc_id:int}", converted, dependencies=[gate])
    client = TestClient(app, raise_server_exceptions=False, headers={"x-user": "anne"})

    # The annotation applies after the check: it allowed 007, and the body reads 7.
    response = client.get("/annotated/007")
    assert (response.status_code, response.json()) == (200, 7)
    assert engine.calls[-1][0] == ("document:007", "view", "user:anne")

    # A convertor applies before it: the check and the body agree.
    response = client.get("/converted/007")
    assert (response.status_code, response.json()) == (200, 7)
    assert engine.calls[-1][0] == ("document:7", "view", "user:anne")

    # A failed annotation is not reached if the check denies: 403, not 422.
    engine.answer = False
    assert client.get("/annotated/abc").status_code == 403


# Sync and async: the event loop keeps running


def blocked_request(
    route: str, make_app: Callable[[threading.Event, threading.Event], Harness],
) -> tuple[httpx.Response, int, Harness]:
    """Hold a check or subject call open, and count loop ticks until it has started."""
    entered, release = threading.Event(), threading.Event()
    app = make_app(entered, release)

    async def main() -> tuple[httpx.Response, int]:
        transport = httpx.ASGITransport(app=app.app)
        async with httpx.AsyncClient(transport=transport, base_url="http://test") as client:
            request = asyncio.create_task(client.get(f"/{route}/readme", headers={"x-user": "anne"}))
            while not entered.is_set():
                await asyncio.sleep(0.005)
            ticks = 0
            for _ in range(5):
                await asyncio.sleep(0.005)
                ticks += 1
            release.set()
            return await request, ticks

    response, ticks = asyncio.run(main())
    return response, ticks, app


def waits_for_the_loop(entered: threading.Event, release: threading.Event) -> None:
    entered.set()
    # Run on the loop's thread, this would hold the loop, and no tick could release it.
    if not release.wait(5):
        raise AssertionError("the event loop was blocked")


@pytest.mark.parametrize("route", ROUTES)
def test_a_slow_check_does_not_block_the_event_loop(route: str) -> None:
    class Slow(StubEngine):
        def __init__(self, entered: threading.Event, release: threading.Event) -> None:
            super().__init__()
            self.entered, self.release = entered, release

        def check(self, *args: Any, **kwargs: Any) -> Any:
            waits_for_the_loop(self.entered, self.release)
            return True

    response, ticks, app = blocked_request(route, lambda e, r: Harness(Slow(e, r)))
    assert response.status_code == 200
    assert ticks == 5
    assert app.ran == ["readme"]


@pytest.mark.parametrize("route", ROUTES)
def test_a_slow_subject_does_not_block_the_event_loop(route: str) -> None:
    def make(entered: threading.Event, release: threading.Event) -> Harness:
        def subject(request: Request) -> str:
            waits_for_the_loop(entered, release)
            return "user:anne"
        return Harness(StubEngine(), subject=subject)

    response, ticks, app = blocked_request(route, make)
    assert response.status_code == 200
    assert ticks == 5
    assert app.ran == ["readme"]


@pytest.mark.parametrize("route", ROUTES)
def test_a_check_blocked_on_a_real_postgres_lock_leaves_the_loop_running(store: Store, route: str) -> None:
    if store.kind != "postgres":
        pytest.skip("needs a real lock in the store")
    psycopg = importlib.import_module("psycopg")
    app = Harness(store.engine)
    with psycopg.connect(PG_URL) as locker:
        tables = locker.execute(
            "SELECT tablename FROM pg_tables WHERE schemaname = %s", (store.schema,)
        ).fetchall()
        for (table,) in tables:
            locker.execute(f'LOCK TABLE "{store.schema}"."{table}" IN ACCESS EXCLUSIVE MODE')
        # On the loop's thread, the check would wait for a lock only the loop can release;
        # this rollback ends that deadlock with too few ticks to pass.
        rescue = threading.Timer(10, locker.rollback)
        rescue.start()

        def waiting() -> int:
            with psycopg.connect(PG_URL, autocommit=True) as watcher:
                row = watcher.execute(
                    "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock'"
                    " AND query ILIKE %s AND pid <> pg_backend_pid()",
                    (f"%{store.schema}%",),
                ).fetchone()
                return int(row[0])

        async def main() -> tuple[httpx.Response, int, int]:
            transport = httpx.ASGITransport(app=app.app)
            async with httpx.AsyncClient(transport=transport, base_url="http://test") as client:
                request = asyncio.create_task(client.get(f"/{route}/readme", headers={"x-user": "anne"}))
                ticks, blocked = 0, 0
                for _ in range(400):
                    await asyncio.sleep(0.01)
                    ticks += 1
                    if ticks % 5 == 0:
                        blocked = max(blocked, await asyncio.to_thread(waiting))
                        if blocked:
                            break
                await asyncio.to_thread(locker.rollback)
                return await request, ticks, blocked

        try:
            response, ticks, blocked = asyncio.run(main())
        finally:
            rescue.cancel()
    assert blocked >= 1, "the check never reached the lock"
    assert ticks >= 5
    assert response.status_code == 200
    assert app.ran == ["readme"]


# The subject callable


@pytest.mark.parametrize("route", ROUTES)
def test_a_subject_that_raises_is_a_500_with_no_check(route: str) -> None:
    engine = StubEngine()

    def subject(request: Request) -> str:
        raise RuntimeError("session store secret")

    app = Harness(engine, subject=subject)
    response = app.get(route, "readme")
    assert response.status_code == 500
    assert "secret" not in response.text
    assert engine.calls == [] and app.ran == []


def test_a_missing_request_state_attribute_is_not_anonymous(store: Store) -> None:
    """The spec's subject when no auth middleware ran: ``public`` must stay closed."""
    app = Harness(store.engine, subject=lambda request: f"user:{request.state.user_id}")
    assert app.get("sync", "public").status_code == 500
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_a_subject_can_raise_its_own_http_exception(route: str) -> None:
    engine = StubEngine()

    def subject(request: Request) -> str:
        raise HTTPException(401, "Sign in", headers={"WWW-Authenticate": "Bearer"})

    app = Harness(engine, subject=subject)
    response = app.get(route, "readme")
    assert (response.status_code, response.json()) == (401, {"detail": "Sign in"})
    assert response.headers["www-authenticate"] == "Bearer"
    assert engine.calls == [] and app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_no_subject_is_a_401_with_no_check(route: str) -> None:
    engine = StubEngine()
    app = Harness(engine, subject=lambda request: None)
    response = app.get(route, "readme")
    assert (response.status_code, response.json()) == (401, NO_SUBJECT)
    assert "www-authenticate" not in response.headers
    assert engine.calls == [] and app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_the_401_carries_the_configured_headers(route: str) -> None:
    engine = StubEngine()
    options = {"unauthenticated_headers": {"WWW-Authenticate": 'Bearer realm="docs"'}}
    app = Harness(engine, subject=lambda request: None, authz_options=options)
    response = app.get(route, "readme")
    assert (response.status_code, response.json()) == (401, NO_SUBJECT)
    assert response.headers["www-authenticate"] == 'Bearer realm="docs"'
    assert engine.calls == [] and app.ran == []


def test_the_configured_headers_reach_only_the_401() -> None:
    options = {"unauthenticated_headers": {"WWW-Authenticate": "Bearer"}}
    denied = Harness(StubEngine(answer=False), authz_options=options)
    allowed = Harness(StubEngine(), authz_options=options)
    failing = Harness(StubEngine(error=pemba.StorageError("down", "STORE_ERROR")), authz_options=options)
    for app, status in ((denied, 403), (allowed, 200), (failing, 503)):
        response = app.get("sync", "readme")
        assert response.status_code == status
        assert "www-authenticate" not in response.headers


def test_the_configured_headers_are_copied() -> None:
    headers = {"WWW-Authenticate": "Bearer"}
    app = Harness(StubEngine(), subject=lambda request: None, authz_options={"unauthenticated_headers": headers})
    headers["WWW-Authenticate"] = "Changed"
    assert app.get("sync", "readme").headers["www-authenticate"] == "Bearer"


@pytest.mark.parametrize("value", ["Bearer", [("WWW-Authenticate", "Bearer")], {"a": 1}, {1: "a"}, {"a": None}])
def test_unauthenticated_headers_must_map_str_to_str(value: Any) -> None:
    with pytest.raises(TypeError, match="unauthenticated_headers"):
        Authz(cast(pemba.Engine, StubEngine()), subject=header_subject, unauthenticated_headers=value)


@pytest.mark.parametrize("route", ROUTES)
def test_a_formatted_none_is_no_subject_and_opens_no_wildcard(store: Store, route: str) -> None:
    """``f"user:{None}"`` is a valid subject, and ``user:*`` on ``public`` would admit it."""
    assert store.engine.check("document:public", "view", "user:None")
    app = Harness(store.engine, subject=lambda request: "user:None")
    response = app.get(route, "public")
    assert (response.status_code, response.json()) == (401, NO_SUBJECT)
    assert app.ran == []


@pytest.mark.parametrize("route", ROUTES)
def test_an_empty_subject_id_is_no_subject(store: Store, route: str) -> None:
    """``f"user:{''}"``: the same mistake as ``None``, so the same answer."""
    app = Harness(store.engine, subject=lambda request: "user:")
    response = app.get(route, "public")
    assert (response.status_code, response.json()) == (401, NO_SUBJECT)
    assert app.ran == []


def test_other_stringified_empties_are_not_blocklisted() -> None:
    """``null`` is an ordinary id: returning ``None`` is the callable's job (OQ-067)."""
    engine = StubEngine()
    assert Harness(engine, subject=lambda request: "user:null").get("sync", "readme").status_code == 200
    assert engine.calls[0][0] == ("document:readme", "view", "user:null")


MALFORMED_SUBJECTS = [
    "", " ", "user", ":", ":anne", "*", "user:*", "nosuch:*", "user:anne ", " user:anne", "a:b:", "USER:",
    "user:anne\n", "user:a@b", "user:a:b", "USER:anne", "user:é", "group:eng#member",
    "user:anne#member", "group:eng#", "document:readme#view@user:anne", "user:*#member",
    "user:" + "x" * 1025,
]


@pytest.mark.parametrize("route", ROUTES)
def test_a_malformed_subject_is_a_500_with_no_check(route: str) -> None:
    for value in MALFORMED_SUBJECTS:
        engine = StubEngine()
        app = Harness(engine, subject=lambda request, value=value: value)
        response = app.get(route, "readme")
        assert response.status_code == 500, repr(value)
        assert engine.calls == [] and app.ran == [], repr(value)


def test_a_malformed_subject_on_a_real_store_is_not_an_outage_or_a_grant(store: Store) -> None:
    for value in ("user:*", "user:anne\n", "group:eng#member", ""):
        app = Harness(store.engine, subject=lambda request, value=value: value)
        assert app.get("sync", "public").status_code == 500, repr(value)
        assert app.ran == []


@pytest.mark.parametrize("value", [1, 1.5, b"user:anne", ["user:anne"], True, object()])
def test_a_subject_of_the_wrong_type_is_a_500(value: Any) -> None:
    engine = StubEngine()
    app = Harness(engine, subject=lambda request: value)
    assert app.get("sync", "readme").status_code == 500
    assert engine.calls == [] and app.ran == []


def test_an_async_subject_is_refused_up_front() -> None:
    async def subject(request: Request) -> str:
        return "user:anne"

    with pytest.raises(TypeError, match="subject"):
        Authz(cast(pemba.Engine, StubEngine()), subject=cast(Any, subject))


class ReturnsCoroutine:
    """A callable object, which ``inspect.iscoroutinefunction`` does not recognise."""

    def __init__(self) -> None:
        self.made: list[Coroutine[Any, Any, Any]] = []

    async def result(self) -> str:
        return "user:anne"

    def __call__(self, request: Request) -> Coroutine[Any, Any, Any]:
        self.made.append(self.result())
        return self.made[-1]


def test_a_subject_that_returns_a_coroutine_is_a_500_and_closes_it() -> None:
    """A coroutine is truthy: it must not pass for an identity, and nothing will await it."""
    subject = ReturnsCoroutine()
    engine = StubEngine()
    app = Harness(engine, subject=subject)
    assert app.get("sync", "readme").status_code == 500
    assert engine.calls == [] and app.ran == []
    assert inspect.getcoroutinestate(subject.made[0]) == inspect.CORO_CLOSED


@pytest.mark.parametrize("value", [None, "user:anne", 3])
def test_subject_must_be_callable(value: Any) -> None:
    with pytest.raises(TypeError, match="subject"):
        Authz(cast(pemba.Engine, StubEngine()), subject=value)


# A missing path parameter


@pytest.mark.parametrize("name", ["id", "doc", "DOC_ID", "doc_id "])
def test_a_path_param_the_route_lacks_is_an_error_not_a_skip(name: str) -> None:
    ran: list[str] = []
    engine = StubEngine()
    app = FastAPI()
    authz = Authz(cast(pemba.Engine, engine), subject=header_subject)

    def sync_route(doc_id: str) -> str:
        ran.append(doc_id)
        return doc_id

    async def async_route(doc_id: str) -> str:
        ran.append(doc_id)
        return doc_id

    gate = Depends(authz.require("document", "view", path_param=name))
    app.add_api_route("/sync/{doc_id}", sync_route, dependencies=[gate])
    app.add_api_route("/async/{doc_id}", async_route, dependencies=[gate])
    client = TestClient(app, raise_server_exceptions=False, headers={"x-user": "anne"})
    for path in ("/sync/readme", "/async/readme"):
        assert client.get(path).status_code == 500
        # The query string is the attacker's: it must not supply the value.
        assert client.get(path, params={name: "readme"}).status_code == 500
        assert client.get(path, headers={name.strip(): "readme"}).status_code == 500
    assert engine.calls == [] and ran == []


@pytest.mark.parametrize("value", ["", None, 3])
def test_path_param_is_required_at_declaration(value: Any) -> None:
    authz = Authz(cast(pemba.Engine, StubEngine()), subject=header_subject)
    with pytest.raises((TypeError, ValueError), match="path_param"):
        authz.require("document", "view", path_param=value)


def test_path_param_has_no_default() -> None:
    authz: Any = Authz(cast(pemba.Engine, StubEngine()), subject=header_subject)
    with pytest.raises(TypeError):
        authz.require("document", "view")


@pytest.mark.parametrize(("resource_type", "permission"), [
    ("", "view"), ("Document", "view"), ("document:x", "view"), ("document#a", "view"),
    ("document", ""), ("document", "VIEW"), ("document", "view@user:anne"), ("document", "a#b"),
    ("doc ument", "view"), ("document", "view\n"),
])
def test_names_are_validated_at_declaration(resource_type: str, permission: str) -> None:
    authz = Authz(cast(pemba.Engine, StubEngine()), subject=header_subject)
    with pytest.raises(ValueError):
        authz.require(resource_type, permission, path_param="doc_id")


@pytest.mark.parametrize(("resource_type", "permission"), [(None, "view"), ("document", None), (1, 2)])
def test_names_must_be_strings(resource_type: Any, permission: Any) -> None:
    authz = Authz(cast(pemba.Engine, StubEngine()), subject=header_subject)
    with pytest.raises(TypeError):
        authz.require(resource_type, permission, path_param="doc_id")


def test_a_websocket_route_fails_closed() -> None:
    """The dependency needs an HTTP ``Request``; a websocket must not get past it."""
    ran: list[str] = []
    engine = StubEngine()
    app = FastAPI()
    authz = Authz(cast(pemba.Engine, engine), subject=header_subject)

    async def socket(websocket: WebSocket, doc_id: str) -> None:
        ran.append(doc_id)

    app.add_api_websocket_route(
        "/ws/{doc_id}", socket,
        dependencies=[Depends(authz.require("document", "view", path_param="doc_id"))],
    )
    client = TestClient(app, raise_server_exceptions=False)
    with pytest.raises(Exception):
        with client.websocket_connect("/ws/readme", headers={"x-user": "anne"}):
            pass
    assert engine.calls == [] and ran == []


# Consistency


def test_the_default_is_latest() -> None:
    engine = StubEngine()
    assert Harness(engine).get("sync", "readme").status_code == 200
    assert engine.calls[0][1] == {"at_least": None}


def test_at_least_is_read_per_request(store: Store) -> None:
    def bound(request: Request) -> int:
        return int(request.headers["x-revision"])

    revision = store.engine.write(touch=["document:fresh#viewer@user:anne"])
    app = Harness(store.engine, at_least=bound)
    assert app.client.get("/sync/fresh", headers={"x-revision": str(int(revision))}).status_code == 200
    assert app.ran == ["fresh"]


def test_a_bound_ahead_of_the_store_is_a_500_a_client_cannot_flood_the_log_with(
    store: Store, caplog: pytest.LogCaptureFixture,
) -> None:
    def bound(request: Request) -> int:
        return int(request.headers["x-revision"])

    revision = store.engine.write(touch=["document:fresh#viewer@user:anne"])
    app = Harness(store.engine, at_least=bound)
    with caplog.at_level(logging.INFO, logger="pemba.contrib.fastapi"):
        ahead = app.client.get("/sync/fresh", headers={"x-revision": str(int(revision) + 1000)})
    assert (ahead.status_code, ahead.json()) == (500, {"detail": "Internal Server Error"})
    assert app.ran == []
    assert isinstance(app.causes[-1], pemba.ConsistencyError)
    records = [record for record in caplog.records if record.name == "pemba.contrib.fastapi"]
    assert [record.levelno for record in records] == [logging.INFO]
    assert records[0].exc_info is None


def test_at_least_none_means_latest() -> None:
    engine = StubEngine()
    assert Harness(engine, at_least=lambda request: None).get("sync", "readme").status_code == 200
    assert engine.calls[0][1] == {"at_least": None}


def test_at_least_takes_a_pending_revision_after_commit(tmp_path: Path) -> None:
    engine = pemba.Engine.sqlite(tmp_path / "pending.db")
    seed(engine)
    conn = sqlite3.connect(tmp_path / "pending.db", isolation_level=None)
    conn.execute("BEGIN IMMEDIATE")
    pending = engine.write(touch=["document:new#viewer@user:anne"], conn=conn)
    conn.execute("COMMIT")
    conn.close()
    app = Harness(engine, at_least=lambda request: pending)
    assert app.get("sync", "new").status_code == 200


def test_an_unresolved_postgres_write_is_a_500(store: Store) -> None:
    if store.kind != "postgres":
        pytest.skip("only Postgres has an unresolved PendingRevision")
    psycopg = importlib.import_module("psycopg")
    with psycopg.connect(PG_URL) as conn:
        pending = store.engine.write(touch=["document:new#viewer@user:anne"], conn=conn)
        app = Harness(store.engine, at_least=lambda request: pending)
        assert app.get("sync", "new").status_code == 500
        conn.commit()
        assert app.get("sync", "new").status_code == 200
    assert app.ran == ["new"]


@pytest.mark.parametrize("value", [-1, 2**64, "1", True, False, 1.0, b"1", [1]])
def test_a_bad_at_least_is_a_500(store: Store, value: Any) -> None:
    app = Harness(store.engine, at_least=lambda request: value)
    assert app.get("sync", "readme").status_code == 500
    assert app.ran == []


def test_at_least_raising_is_a_500_with_no_check() -> None:
    def bound(request: Request) -> int:
        raise RuntimeError("no token")

    engine = StubEngine()
    app = Harness(engine, at_least=bound)
    assert app.get("sync", "readme").status_code == 500
    assert engine.calls == [] and app.ran == []


def test_at_least_can_raise_its_own_http_exception() -> None:
    def bound(request: Request) -> int:
        raise HTTPException(400, "Bad token")

    engine = StubEngine()
    app = Harness(engine, at_least=bound)
    assert app.get("sync", "readme").status_code == 400
    assert engine.calls == [] and app.ran == []


def test_at_least_must_be_a_synchronous_callable() -> None:
    authz = Authz(cast(pemba.Engine, StubEngine()), subject=header_subject)

    async def bound(request: Request) -> int:
        return 1

    for value in (5, "5", bound):
        with pytest.raises(TypeError, match="at_least"):
            authz.require("document", "view", path_param="doc_id", at_least=cast(Any, value))


def test_a_coroutine_from_at_least_is_a_500_and_closes_it() -> None:
    bound = ReturnsCoroutine()
    engine = StubEngine()
    app = Harness(engine, at_least=bound)
    assert app.get("sync", "readme").status_code == 500
    assert engine.calls == [] and app.ran == []
    assert inspect.getcoroutinestate(bound.made[0]) == inspect.CORO_CLOSED


def test_at_exactly_is_not_offered() -> None:
    """A permission gate must not answer from before a revocation."""
    authz: Any = Authz(cast(pemba.Engine, StubEngine()), subject=header_subject)
    with pytest.raises(TypeError):
        authz.require("document", "view", path_param="doc_id", at_exactly=1)


# The guide


def test_guide_example(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    path = ROOT / "docs" / "guides" / "fastapi.md"
    monkeypatch.chdir(tmp_path)
    code = python_block(path, "## Runnable example")
    exec(compile(code, str(path), "exec"), {"__name__": "__main__"})
