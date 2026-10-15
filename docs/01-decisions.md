# 01 — Architecture decision records

Each ADR: context → decision → consequences. Status is `Accepted` unless noted.
Changing an accepted ADR requires the human's sign-off.

---

## ADR-001 — Rust core

**Context.** The engine must be fast, memory-safe, and embeddable in several host languages.
**Decision.** Core in Rust. Host languages get thin bindings (PyO3 for Python first).
**Consequences.** One implementation of the evaluator for every language. Build/packaging
complexity moves to CI (maturin wheels).

## ADR-002 — Embedded-first, server later

**Context.** All major open-source Zanzibar engines are services. The gap is in-process use.
**Decision.** The primary deliverable is a library. A server mode (gRPC/HTTP) is a later,
optional crate that wraps the same `Engine`.
**Consequences.** No cluster dispatch, no consistent hashing, no cross-node caches. The
evaluator recurses in-process. Anything distributed is out of scope.

## ADR-003 — Synchronous core API

**Context.** PyO3 + async Rust is awkward; SQLite drivers are synchronous; checks are
CPU-bound graph walks over local storage.
**Decision.** `Engine` and the store traits are **synchronous**. The Python binding releases
the GIL (`py.allow_threads`) around every engine call so Python threads run concurrently.
An async facade can be added later (e.g. `spawn_blocking`) without changing the core.
**Consequences.** A future async Postgres store must be wrapped behind a blocking adapter or
use a sync driver (`postgres` crate). Revisit before Postgres work (logged as open question).

## ADR-004 — Schema DSL is a strict subset of SpiceDB's

**Context.** Familiar syntax lowers adoption cost, and lets us differential-test against
SpiceDB using identical schemas.
**Decision.** Every schema we accept must parse in SpiceDB and mean the same thing there.
Where SpiceDB semantics are surprising, we **reject** rather than diverge — most importantly
operator precedence: SpiceDB gives `+` higher precedence than `&` and `-` (`a + b & c` means
`(a + b) & c`). We **require parentheses whenever different binary operators are mixed at
the same level** (`docs/03-schema-language.md` §4.3). Unsupported SpiceDB features are
recognised and rejected with `E_UNSUPPORTED_FEATURE`.
**Consequences.** Some valid SpiceDB schemas are rejected by us; none of ours mean something
different in SpiceDB.

## ADR-005 — Revision model: one monotonic counter per store

**Context.** Zanzibar uses zookies (snapshot timestamps) across a global system. We have one
process and one database.
**Decision.** Every committed write batch is assigned a `Revision(u64)` by the store, strictly
increasing in **commit order**. Tuples carry `created_rev` / `deleted_rev` (MVCC-by-columns).
Reads take a `Consistency` and resolve it to a single revision before evaluating.
**Consequences.** Read-after-write is easy (`AtLeast(rev_from_write)`). Historical reads up
to the GC horizon are possible (`Exact(rev)`). Commit-order guarantee is trivial on SQLite
(single writer) and needs care on Postgres (open question).

## ADR-006 — Transactional writes through the host's connection

**Context.** The headline feature: relationship writes commit atomically with app data.
**Decision.** All write logic (validation, precondition reads, revision allocation, row
inserts/updates) is implemented once in Rust, generic over a small `WriteTxn` trait
(`execute`, `query`, `last_insert_id`). Two implementations:
1. **Native**: the engine's own `rusqlite` connection (engine-managed transaction).
2. **Foreign**: in `warden-py`, an adapter that forwards SQL to a **Python DB-API 2.0
   connection** supplied by the caller (`engine.write(updates, conn=app_conn)`), so the SQL
   runs inside the app's open transaction.
**Consequences.** SQL must be written in a portable subset (positional `?` params for SQLite).
The caller owns commit/rollback in foreign mode; returned revision is only valid after commit.
Reads always use the engine's own connections.

**Amendment (T3.2a, OQ-032): one SQLite library per process.** *Context:* in Python the
engine's `rusqlite` connections (bundled SQLite) and the app's `sqlite3` connections were
two SQLite copies in one process. On POSIX, a `close()` by either copy drops every
`fcntl` lock the other holds on the file, and can checkpoint and delete the WAL under it.
The T3.2 probe lost a committed transaction this way on Linux and macOS
(<https://www.sqlite.org/howtocorrupt.html> §2.2–2.3). *Decision:* "the engine's own
connections" means connections **of the SQLite library the host already uses**.
(1) In Python, `Engine.sqlite()` uses `warden-store-pysqlite`. Every statement (reads,
native writes, migrations, GC, schema writes) runs on Python `sqlite3` connections the
engine opens and owns, through the same `WriteTxn` / `SqlDialect` / `ReadStore` traits,
so all authorisation logic stays in Rust. Requests on such an engine are serialised and
hold the GIL for their whole duration. `warden-py` does not depend on rusqlite, and CI
fails if the extension contains SQLite. (2) In Rust, a caller's transaction for
`Engine::write_in` must come from `warden_store_sqlite::rusqlite`. Cargo's
`links = "sqlite3"` already forbids a second `libsqlite3-sys`; a dependency that compiles
its own SQLite is unsupported. (3) Rejected: linking the extension against the
interpreter's SQLite (option B). It is impossible for manylinux wheels, python.org macOS
and python-build-standalone without hand-written `unsafe`, and no version string can
detect a mismatch. *Consequences:* a Python check costs about 100 µs single-threaded,
and SQLite engine requests do not run in parallel (OQ-032 has the numbers). CPython's
SQLite must be at least `MIN_SQLITE_VERSION` (`docs/05-storage.md` §4.3). Anything in
the process that opens the database with its own SQLite (`apsw`, other bundled builds)
is unsupported.

## ADR-007 — LookupResources = candidate generation + check filter

**Context.** Exact reverse evaluation through intersections and exclusions is complex and
easy to get wrong.
**Decision.** `lookup_resources` walks the schema graph **backwards** from the subject,
treating every operator as a union (over-approximation), to produce candidate resources;
then runs `check` on each candidate to filter. (`docs/04-evaluation.md` §5)
**Consequences.** Always correct if `check` is correct. Worst case does extra checks for
schemas heavy in intersection/exclusion. Acceptable for MVP; optimisable later.

## ADR-008 — Fail closed, errors are errors

**Decision.** Evaluation never converts an error into `false`. Depth exceeded, store
failure, missing schema element at runtime → `Err`. (`CLAUDE.md` rule 4)

## ADR-009 — Bounded recursion + cycle-safe memoisation

**Decision.** Per-request memo table keyed by `(object, relation, subject)`. A key currently
being evaluated counts as `false` for the purpose of breaking cycles, and **results that
depended on such an assumption are not memoised** (`docs/04-evaluation.md` §3.6).
Hard depth limit (default 50) returns `Error::MaxDepthExceeded`.

## ADR-010 — Engine-owned tables live in the app database, prefixed `warden_`

**Decision.** SQLite tables `warden_meta`, `warden_revisions`, `warden_relationships`,
`warden_schema`. Engine runs its own migrations (`Engine::migrate`), tracked in
`warden_meta.storage_version`. (`docs/05-storage.md` §4)

## ADR-011 — Licence and attribution

**Decision.** Apache-2.0. README credits the Zanzibar paper and SpiceDB as design references.
No SpiceDB code is copied or translated. If adaptation is ever needed: human approval,
retain SpiceDB's copyright header in the file, add an entry to `NOTICE`.

## ADR-012 — Codename `warden`, rename before 0.1

**Status:** Provisional.
**Decision.** Crates are `warden-*`, Python package `warden`, table prefix `warden_`. Before
publishing, the human picks a final name free on crates.io, PyPI, npm and GitHub; rename by
find-replace. Keep the name in exactly these places to make the rename mechanical:
crate names, the Python module name, the table prefix constant `TABLE_PREFIX`, docs.

## ADR-013 — `.all()` intersection arrow is in the MVP; `self`, typechecking, caveats are not

**Context.** SpiceDB supports `->` / `.any()` (alias) and `.all()` (every related object must
grant the permission). `self` and `use typechecking` are recent additions; caveats need CEL.
**Decision.** MVP supports `->`, `.any(x)` and `.all(x)`. An `.all` over **zero** related
objects evaluates to `false` (verify against SpiceDB in differential tests; if SpiceDB
differs, match SpiceDB and update this ADR). Everything else listed is rejected with
`E_UNSUPPORTED_FEATURE`.

## ADR-014 — Python is the first binding; packaging via maturin

**Decision.** `warden-py` crate built with maturin into abi3 wheels (Python ≥ 3.11,
pyo3 feature `abi3-py311`: 3.9 is end-of-life and 3.10 reaches it in October 2026; re-check
the oldest supported CPython before each release) for Linux x86_64/aarch64
(manylinux), macOS x86_64/arm64, Windows x86_64. Type stubs (`.pyi`) shipped.
