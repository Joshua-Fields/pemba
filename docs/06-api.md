# 06 — Public APIs (Rust, Python, CLI)

Public surface is frozen once 0.1 ships. Before then, change it only by editing this file in
the same change (CLAUDE.md rule 10).

---

## 1. Rust (`pemba` facade crate re-exports `pemba-core` + chosen stores)

```rust
pub struct Engine<S: WritableStore> { /* schema: ArcSwap<CompiledSchema>, store: S, config */ }

impl<S: WritableStore> Engine<S> {
    pub fn builder() -> EngineBuilder<S>;

    // ---- schema ----
    pub fn write_schema(&self, source: &str) -> Result<SchemaWriteResult, Error>;
    pub fn read_schema(&self) -> Result<Option<String>, Error>;
    pub fn validate_schema(source: &str) -> Result<ValidatedSchema, SchemaErrors>; // static, no store

    // ---- writes ----
    pub fn write(&self, req: WriteRequest) -> Result<Revision, Error>;
    pub fn write_in<T: WriteTxn>(&self, txn: &mut T, req: WriteRequest) -> Result<Revision, Error>
        where S: SqlStore;                                   // SQL stores only (05 §2, §10)
    pub fn delete_relationships(&self, filter: RelationshipFilter, limit: Option<usize>)
        -> Result<DeleteResult, Error>;                      // T2.6, 02 §5

    // ---- reads ----
    pub fn check(&self, req: CheckRequest) -> Result<CheckResponse, Error>;
    pub fn check_bulk(&self, items: Vec<CheckItem>, c: Consistency) -> Result<Vec<Result<bool, Error>>, Error>;
    pub fn lookup_resources(&self, req: LookupResourcesRequest) -> Result<LookupResourcesResponse, Error>;
    pub fn lookup_subjects(&self, req: LookupSubjectsRequest) -> Result<LookupSubjectsResponse, Error>;
    pub fn read_relationships(&self, req: ReadRelationshipsRequest) -> Result<ReadRelationshipsResponse, Error>;
    pub fn head_revision(&self) -> Result<Revision, Error>;

    // ---- maintenance ----
    pub fn gc(&self, keep_from: Revision) -> Result<GcStats, Error>;
}

pub struct EngineConfig {
    pub max_depth: u32,                 // 50
    pub max_write_batch: usize,         // 1000
    pub max_delete_by_filter: usize,    // 10_000
    pub max_lookup_results: u32,        // 1000
    pub max_lookup_nodes: usize,        // 200_000
    pub max_check_fanout: usize,        // 100_000
    pub max_check_reads: u64,           // 200_000 (04 §3.6)
}
```

Migrations are a store concern, not an engine one (OQ-025 (d)): the store is opened, and its
storage version checked, before any engine exists. SQLite: `SqliteConfig::auto_migrate`
(default `true`) is read by `SqliteStore::open`; the explicit deploy step is
`SqliteStore::migrate(path, &config)` (`05` §4.1). There is no `Engine::migrate` and no
`EngineConfig::auto_migrate`.

Supporting types (defined in T1.2):

```rust
pub struct EngineBuilder<S> { /* store: Option<S>, config */ }
impl<S: WritableStore> EngineBuilder<S> {
    pub fn store(self, store: S) -> Self;               // required
    pub fn config(self, config: EngineConfig) -> Self;  // default EngineConfig::default()
    /// InvalidRequest if no store, or max_write_batch / max_lookup_results is 0;
    /// Schema if the store's current schema no longer compiles. Loads that schema.
    pub fn build(self) -> Result<Engine<S>, Error>;
}

pub struct SchemaWriteResult {
    pub version: u64,                   // current schema version (1, 2, ...)
    pub revision: Revision,             // revision that version was written at
    pub changed: bool,                  // false: same hash as current, nothing written
    pub warnings: Vec<SchemaError>,     // W_... diagnostics
}

pub struct ReadRelationshipsRequest {
    pub filter: RelationshipFilter,
    pub consistency: Consistency,       // ::new(filter) gives Latest
    pub limit: Option<u32>,             // None = max_lookup_results; 0 or > max -> InvalidRequest
    pub cursor: Option<Relationship>,   // exclusive keyset cursor (previous next_cursor)
}
pub struct ReadRelationshipsResponse {
    pub relationships: Vec<Relationship>, // storage-column order (05 §2)
    pub revision: Revision,               // pass Consistency::Exact(revision) for later pages
    pub next_cursor: Option<Relationship>,// None on the last page
}
```

Check types (defined in T1.3; `04` §3.1):

```rust
pub struct CheckRequest { pub resource: ObjectRef, pub permission: RelName,
                          pub subject: SubjectRef, pub consistency: Consistency }
pub struct CheckResponse { pub allowed: bool, pub revision: Revision, pub stats: EvalStats }
pub struct CheckItem { pub resource: ObjectRef, pub permission: RelName,
                       pub subject: SubjectRef }   // CheckRequest minus consistency
pub struct EvalStats {
    pub store_reads: u64, pub tuples_read: u64, pub memo_hits: u64,
    pub max_depth_reached: u32,          // root node = depth 1
    pub cycle_passes: u64,               // passes of SCC heads that re-ran (04 §3.6)
    pub elapsed: Duration,               // on the span as `elapsed_us`
}
```

Lookup types (defined in T2.4; `04` §5):

```rust
pub struct LookupResourcesRequest {
    pub resource_type: TypeName,
    pub permission: RelName,            // a permission or relation of resource_type
    pub subject: SubjectRef,            // Object or Set; Wildcard -> InvalidRequest
    pub consistency: Consistency,       // ::new(resource_type, permission, subject) gives Latest
    pub limit: Option<u32>,             // None = max_lookup_results; 0 or > max -> InvalidRequest
    pub cursor: Option<ObjectId>,       // exclusive: ids strictly after it
}
pub struct LookupResourcesResponse {
    pub resource_ids: Vec<ObjectId>,    // ascending byte-wise
    pub next_cursor: Option<ObjectId>,  // last returned id if candidates remain, else None
    pub revision: Revision,             // pass Consistency::Exact(revision) for later pages
}
```

**Paging consistency.** Each `lookup_resources` call is one request on one read snapshot
(`05` §2 "Revision checks"); no snapshot is held between pages. Callers **must** request
every page after the first at `Consistency::Exact(first_page.revision)`: at `Latest` a later
page reads whatever is newest, so resources added or removed between pages can be skipped or
returned twice relative to one revision. A `next_cursor` only says candidates remain, not
that any is allowed, so **a page may be empty**: callers loop, passing each `next_cursor`
back, **until `next_cursor` is `None`**, and must not treat an empty page as the end (OQ-028
(c)). Every page re-runs phase 1 (`04` §5.1);
a page fails as a whole with the first error any candidate's `check` returns, or
`LookupTooLarge` past `max_lookup_nodes` phase 1 nodes.

`lookup_subjects` types (defined in T2.5; `04` §6):

```rust
pub struct LookupSubjectsRequest {
    pub resource: ObjectRef,
    pub permission: RelName,
    pub subject_type: TypeName,         // object subjects only
    pub consistency: Consistency,       // ::new(resource, permission, subject_type) gives Latest
}
pub struct LookupSubjectsResponse {
    pub subject_ids: Vec<ObjectId>,     // ascending byte-wise; empty if wildcard
    pub wildcard: bool,                 // every object of subject_type (even unwritten) ...
    pub excluded_ids: Vec<ObjectId>,    // ... except these, ascending
    pub revision: Revision,
}
impl LookupSubjectsResponse { pub fn contains(&self, id: &ObjectId) -> bool; }
```

`lookup_subjects` is one request on one snapshot, not paginated. It is one evaluation,
bounded like one `check`: its store reads count against `max_check_reads`
(`ReadLimitExceeded`), and `max_depth` / `max_check_fanout` apply (fan-out counts every
tuple read on a relation, OQ-029). It fails with the same evaluation errors as `check`.
It is **stricter** than `check` about stale tuples (stored tuples the current schema no
longer allows, possible only at revisions before a schema change; `05` §7): it resolves
every stored subject set on each node it reaches, so it may fail with `StaleRelationship`
where `check` short-circuits past the stale tuple after finding a match and succeeds
(OQ-029 (d)).

`check_bulk` returns `Err` for the whole call on `NoSchema`, consistency errors, `Store`,
and `RevisionExpired` raised mid-call by GC; every other error (`InvalidRequest`,
`MaxDepthExceeded`, `NonMonotonicCycle`, `FanoutExceeded`, `ReadLimitExceeded`,
`StaleRelationship`, `AmbiguousWildcardExclusion`) belongs to
its item (OQ-019).

`write` checks, in order: batch size, schema present, op validation (`02` §6), precondition
filters, then the store's duplicates, preconditions and `Create` conflicts (`02` §5).
`read_relationships` and precondition filters must name only types, relations (not
permissions) and subject relations the schema defines, else `InvalidRequest` (OQ-017).
`write_schema` with the same hash as the current schema is a no-op (`05` §7). Otherwise it
fails with `SchemaChangeOrphans(Vec<OrphanReport>)` if any live tuple would fail
validation under the new schema (`03` §10), writing nothing.

T2.6 types:

```rust
pub struct OrphanReport {           // one group of live tuples the new schema rejects
    pub resource_type: TypeName, pub relation: RelName,
    pub subject: String,            // "user", "user:*" or "group#member"
    pub count: u64,
    pub code: ErrorCode,            // as a write of one would fail: E_UNKNOWN_TYPE, ...
    pub message: String,
}
pub struct DeleteResult { pub deleted: u64, pub more: bool, pub revision: Revision }
pub struct GcStats { pub rows_removed: u64, pub horizon: Revision, pub batches: u64 }
```

`delete_relationships(filter, limit)`: `limit` `None` = `max_delete_by_filter` (also the
maximum; 0 or above → `InvalidRequest`); the filter is validated like `read_relationships`'.
One atomic revision; loop while `more`.

`gc(keep_from)` raises the GC horizon first (its own transaction), then deletes collected
rows in batches of 10 000 (`05` §8). From the moment the horizon moves, `Exact(r)` with
`r < keep_from` fails with `RevisionExpired`; a request already running answers from its
snapshot, in full.

Schema across processes (T2.6, `05` §7): every read request uses the schema version stored
as of its own snapshot, so a schema written by another process is picked up by the next
request, and schema and data never come from different moments. Writes validate against
the newest schema the engine knows and commit only if the store is still at that version;
otherwise the engine reloads and retries (a write can therefore fail validation against a
schema another process just wrote). `StoreError::SchemaChanged` (retryable) surfaces only
if the schema keeps changing for 3 attempts.

`write_in` (T2.2) runs the same checks and the same store write algorithm (`05` §5) inside a
transaction the caller owns, after verifying it is on the store's database
(`SqlStore::check_same_database`, else `ForeignConnectionMismatch`). It never commits or
rolls back, and changes no engine or store state in memory. The returned revision exists
only once the caller commits: until then reads at it fail with `RevisionNotYetAvailable`,
and **if the caller rolls back, the revision is void** — the next write may be given the
same number, so never keep a revision from a rolled-back transaction. On error the batch's
own statements are undone to a savepoint, but the caller should roll back anyway. With
SQLite, wrap the caller's transaction in `SqliteTxn` and open it with `BEGIN IMMEDIATE`: a
deferred transaction that read before another connection committed cannot take the write
lock, and the write fails with the retryable `StoreError::Conflict` (`05` §5.1).

`StoreError::Conflict` (`is_retryable()` is true) means the caller's **whole transaction is
stale**, not just the failed statement: it read a snapshot that another connection has since
committed past, so everything it decided on (its own reads, preconditions, application rows)
may be out of date. Roll the whole transaction back and run it again from the start,
re-reading what it depends on; retrying only the `write_in` call inside the same transaction
fails the same way.


```rust
let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
tx.execute("INSERT INTO documents (id) VALUES (?1)", ["readme"])?;
let rev = engine.write_in(&mut SqliteTxn::new(&tx), req)?;
tx.commit()?;                       // only now is `rev` real
```

Ergonomic helpers (thin wrappers, all tested):

```rust
engine.check_str("document:readme", "view", "user:anne", Consistency::Latest)?;   // -> bool
engine.write_str(&["document:readme#viewer@user:anne"])?;                         // Touch each
engine.delete_str(&["document:readme#viewer@user:anne"])?;
```

Store constructors:

```rust
let store = SqliteStore::open("authz.db", SqliteConfig::default())?;
let store = MemoryStore::new();
let engine = Engine::builder().store(store).config(cfg).build()?;
engine.write_schema(SCHEMA)?;
```

### 1.1 Error type

```rust
#[non_exhaustive]
pub enum Error {
    Schema(SchemaErrors),                          // compile/validate failures, all collected
    SchemaChangeOrphans(Vec<OrphanReport>),
    NoSchema,                                      // read/write before any schema
    InvalidRelationship { rel: String, code: ErrorCode, message: String },
    InvalidRequest { message: String },
    Parse(ParseError),                             // tuple/object strings
    DuplicateInBatch(String),
    AlreadyExists(String),                         // Create on existing tuple
    PreconditionFailed { index: usize },
    BatchTooLarge { max: usize, got: usize },
    RevisionNotYetAvailable { requested: Revision, head: Revision },
    RevisionExpired { requested: Revision, horizon: Revision },
    MaxDepthExceeded { path: Vec<String> },
    NonMonotonicCycle { path: Vec<String> },
    FanoutExceeded { node: String, max: usize },
    ReadLimitExceeded { max: u64 },               // more than max_check_reads store reads (04 §3.6)
    StaleRelationship { tuple: String, relation: String }, // stored tuple the schema no longer allows (04 §3.3)
    AmbiguousWildcardExclusion { wildcard: String, subject: String }, // t:* under a subtracted side vs a subject-set subject (04 §3.3, T3.4b)
    LookupTooLarge { max: usize },
    NotMigrated,
    StorageVersionTooNew { found: u32, supported: u32 },
    ForeignConnectionMismatch,
    Store(StoreError),
}
```
`InvalidRelationship.code` is a `pemba_schema::ErrorCode` (`E_UNKNOWN_TYPE`,
`E_UNKNOWN_RELATION`, `E_WRITE_TO_PERMISSION`, `E_SUBJECT_TYPE_NOT_ALLOWED`; `02` §6).
Every variant has a stable machine code (`Error::code() -> &'static str`, e.g.
`"MAX_DEPTH_EXCEEDED"`) used by the Python exception mapping. Codes so far: `SCHEMA_ERROR`,
`NO_SCHEMA`, `INVALID_RELATIONSHIP`, `INVALID_REQUEST`, `DUPLICATE_IN_BATCH`,
`ALREADY_EXISTS`, `PRECONDITION_FAILED`, `BATCH_TOO_LARGE`, `REVISION_NOT_YET_AVAILABLE`,
`REVISION_EXPIRED`, `STORE_ERROR`, `MAX_DEPTH_EXCEEDED`, `NON_MONOTONIC_CYCLE`,
`FANOUT_EXCEEDED`, `READ_LIMIT_EXCEEDED`, `STALE_RELATIONSHIP`, `NOT_MIGRATED`,
`STORAGE_VERSION_TOO_NEW`, `FOREIGN_CONNECTION_MISMATCH`, `LOOKUP_TOO_LARGE` (T2.4),
`SCHEMA_CHANGE_ORPHANS` (T2.6), `TRANSACTION_CONFLICT` (T3.1: `Store(StoreError::Conflict)`,
split from `STORE_ERROR` so the code alone tells a caller to retry the whole transaction),
`AMBIGUOUS_WILDCARD_EXCLUSION` (T3.4b);
variants added later use their name in SCREAMING_SNAKE case. Runtime codes never carry the
`E_` prefix, which belongs to schema diagnostics (`ErrorCode`). `Error::CODES` lists every
code `code()` can return.

**`Consistency::Exact(r)` is not history.** It evaluates the *current* schema over the
data as of `r`, not what was allowed at `r` (`04` §1, OQ-030): after `banned` is removed
from `view = viewer - banned`, a check at an `Exact` revision from before the removal no
longer subtracts `banned`. It is not an audit log.

---

## 2. Python (`pemba` package, built from `pemba-py`)

### 2.1 Usage

```python
import sqlite3
from pemba import Engine

engine = Engine.sqlite("app.db")              # or Engine.memory()
engine.write_schema(open("authz.zed").read())

rev = engine.write(touch=["document:readme#viewer@user:anne"])
engine.check("document:readme", "view", "user:anne", at_least=rev)    # True
engine.check("document:readme#view@user:bob")                          # single-string form

engine.lookup_resources("document", "view", "user:anne")              # LookupResult(ids=[...], cursor=None, revision=..)
engine.lookup_subjects("document:readme", "view", "user")             # SubjectsResult(ids, wildcard, excluded, revision)

# transactional with app data (headline feature)
conn = sqlite3.connect("app.db", isolation_level=None)
conn.execute("BEGIN IMMEDIATE")
conn.execute("INSERT INTO documents(id, title) VALUES (?, ?)", ("readme", "Readme"))
rev = engine.write(touch=["document:readme#owner@user:anne"], conn=conn)
conn.execute("COMMIT")
```

### 2.2 Signatures (ship as `pemba/__init__.pyi`)

```python
MIN_SQLITE_VERSION: str   # "3.26.0": oldest sqlite3.sqlite_version Engine.sqlite() accepts

class Revision(int): ...

class Engine:
    @staticmethod
    def sqlite(path: str | os.PathLike[str], *, reader_pool: int = 4,
               busy_timeout_ms: int = 5000, auto_migrate: bool = True,
               max_depth: int = 50) -> "Engine": ...
    @staticmethod
    def memory(*, max_depth: int = 50) -> "Engine": ...

    def write_schema(self, source: str) -> int: ...                 # schema version
    def read_schema(self) -> str | None: ...
    @staticmethod
    def validate_schema(source: str) -> list["Diagnostic"]: ...      # errors + warnings, never raises

    def write(self, *, touch: Iterable[str | Relationship] = (),
              create: Iterable[str | Relationship] = (),
              delete: Iterable[str | Relationship] = (),
              preconditions: Iterable["Precondition"] = (),
              conn: Any | None = None, allow_autocommit: bool = False) -> Revision: ...  # 05 §10
    def delete_relationships(self, *, resource_type: str, resource_id: str | None = None,
                             relation: str | None = None, subject_type: str | None = None,
                             subject_id: str | None = None,
                             subject_relation: str | None = None,
                             limit: int | None = None) -> "DeleteResult": ...

    @overload
    def check(self, tuple_str: str, /, *, at_least: int | None = None,
              at_exactly: int | None = None) -> bool: ...
    @overload
    def check(self, resource: str, permission: str, subject: str, /, *,
              at_least: int | None = None, at_exactly: int | None = None) -> bool: ...
    def check_detailed(self, resource: str, permission: str, subject: str, /, *,
                       at_least: int | None = None, at_exactly: int | None = None) -> "CheckResult": ...
    def check_bulk(self, items: Sequence[tuple[str, str, str]], /, *,
                   at_least: int | None = None, at_exactly: int | None = None
                   ) -> list[bool | "PembaError"]: ...

    def lookup_resources(self, resource_type: str, permission: str, subject: str, /, *,
                         limit: int | None = None, cursor: str | None = None,
                         at_least: int | None = None, at_exactly: int | None = None) -> "LookupResult": ...
    def iter_resources(self, resource_type: str, permission: str, subject: str, /, *,
                       at_least: int | None = None, at_exactly: int | None = None
                       ) -> Iterator[str]: ...                       # pages internally at one revision
    def lookup_subjects(self, resource: str, permission: str, subject_type: str, /, *,
                        at_least: int | None = None, at_exactly: int | None = None) -> "SubjectsResult": ...
    def read_relationships(self, *, resource_type: str, resource_id: str | None = None,
                           relation: str | None = None, subject_type: str | None = None,
                           subject_id: str | None = None, subject_relation: str | None = None,
                           at_least: int | None = None, at_exactly: int | None = None
                           ) -> list["Relationship"]: ...            # every page, at one revision
    def head_revision(self) -> Revision: ...
    def gc(self, keep_from: int) -> "GcStats": ...

class Relationship:                       # frozen; str() is the tuple string
    def __init__(self, resource_type: str, resource_id: str, relation: str,
                 subject_type: str, subject_id: str, subject_relation: str | None = None) -> None: ...
    @staticmethod
    def parse(s: str) -> "Relationship": ...

class Precondition:                       # frozen; filters as in delete_relationships
    @staticmethod
    def must_exist(*, resource_type: str, **filter: str | None) -> "Precondition": ...
    @staticmethod
    def must_not_exist(*, resource_type: str, **filter: str | None) -> "Precondition": ...
```

Rules:
- Consistency is spelled out as the keyword-only `at_least` / `at_exactly` on every read,
  rather than `**consistency`, so `mypy --strict` checks it. They are mutually exclusive
  (`ValueError`). Neither → `Latest`. A negative revision → `ValueError`.
- `at_exactly=r` has the Rust `Exact(r)` meaning: **the current schema applied to the data
  as of `r`**, not what was allowed at `r`. If `view = viewer - banned` loses `banned`, a
  check at an older `at_exactly` no longer subtracts `banned`. It is not an audit log
  (`04` §1, OQ-030).
- Filters (`delete_relationships`, `read_relationships`, `Precondition`): `None` matches
  anything; `subject_relation=""` matches only subjects without a relation (plain objects
  and wildcards). A wildcard subject has `subject_id == "*"`.
- `iter_resources` pins the revision of its first page (`Exact`) for every later page and
  stops only when the cursor runs out (pages may be empty, `06` §1). It fetches lazily.
- `read_relationships` returns every match, paging internally at the first page's
  revision; bound it with filters.
- Malformed tuple/object strings raise `ValueError` (argument mistakes), not `PembaError`.
- Migration is a store concern (OQ-025 (d)), so there is no `Engine.migrate()`:
  `Engine.sqlite(path)` migrates on open while `auto_migrate=True` (the default); a
  production deploy step runs `Engine.sqlite(path, auto_migrate=True)` once, and app
  processes open with `auto_migrate=False` (`NotMigrated` if the step was skipped). An
  instance method could never run where it matters: with `auto_migrate=False` an
  unmigrated database fails in `Engine.sqlite` before any engine exists.
- Memory-engine calls release the GIL. SQLite-engine calls run one at a time per engine
  and hold the GIL throughout, because every statement is a call into `sqlite3` (§2.5,
  OQ-033 (b)). Other Python threads still run while SQLite itself works or waits on a
  lock (`sqlite3` releases the GIL there).
- `write(..., conn=c)` runs the batch inside `c`'s open transaction (`05` §10): `c` is a
  `sqlite3.Connection` on the engine's database file (else `TypeError`, or
  `StorageError` `FOREIGN_CONNECTION_MISMATCH` for another file); the caller commits or
  rolls back, and the returned revision is readable only after commit. No open
  transaction → `ValueError` unless `allow_autocommit=True`. Memory engine → `ValueError`.
- `Engine` is thread-safe; share one instance per process. Not fork-safe: create it after
  forking (document for gunicorn `--preload` users).
- Dataclass-like result types (`Relationship`, `Precondition`, `CheckResult`,
  `LookupResult`, `SubjectsResult`, `Diagnostic`, `GcStats`, `DeleteResult`,
  `OrphanReport`) are frozen `#[pyclass]`es with `__repr__`, `__eq__`, `__hash__` where
  sensible. Revisions in them are `Revision`.

### 2.3 Exceptions

```text
PembaError(Exception)               .code: str  (Rust Error::code()), .retryable: bool
├── SchemaError                      SCHEMA_ERROR; .diagnostics: list[Diagnostic]
├── SchemaChangeOrphansError         SCHEMA_CHANGE_ORPHANS; .orphans: list[OrphanReport] (per-group counts)
├── NoSchemaError                    NO_SCHEMA
├── InvalidRelationshipError         INVALID_RELATIONSHIP; .relationship: str, .reason: str (E_ code)
├── InvalidRequestError              INVALID_REQUEST, BATCH_TOO_LARGE
├── ConsistencyError                 REVISION_NOT_YET_AVAILABLE, REVISION_EXPIRED
├── EvaluationLimitError             MAX_DEPTH_EXCEEDED, FANOUT_EXCEEDED, READ_LIMIT_EXCEEDED,
│                                    LOOKUP_TOO_LARGE, NON_MONOTONIC_CYCLE
├── StaleRelationshipError           STALE_RELATIONSHIP; .relationship: str
├── AmbiguousWildcardExclusionError  AMBIGUOUS_WILDCARD_EXCLUSION; .relationship: str (the
│                                    stored t:*), .subject: str (04 §3.3, T3.4b)
├── WriteConflictError               ALREADY_EXISTS, PRECONDITION_FAILED, DUPLICATE_IN_BATCH
└── StorageError                     STORE_ERROR, NOT_MIGRATED, STORAGE_VERSION_TOO_NEW,
    │                                FOREIGN_CONNECTION_MISMATCH
    └── TransactionConflictError     TRANSACTION_CONFLICT: retry the whole transaction
```
Every `Error::code()` maps to exactly one class (tested by iterating `Error::CODES`).
`retryable` is `StoreError::is_retryable()` (true for `TRANSACTION_CONFLICT` and a
persistent `SchemaChanged`), else false.
Python argument mistakes raise built-in `ValueError` / `TypeError`, not `PembaError`.

### 2.4 FastAPI helper (stretch, `pemba.contrib.fastapi`)

```python
from pemba.contrib.fastapi import Authz
authz = Authz(engine, subject=lambda request: f"user:{request.state.user_id}")

@app.get("/documents/{doc_id}")
def get_doc(doc_id: str, _=Depends(authz.require("document", "view", path_param="doc_id"))):
    ...
```
Raises `HTTPException(403)` on deny, `HTTPException(503)` on `PembaError` (fail closed).
Pure Python, optional extra `pip install pemba[fastapi]`.

### 2.5 SQLite in the same process

`Engine.sqlite()` runs on **CPython's own `sqlite3` module**. The engine shares the
interpreter's SQLite library and bundles none (ADR-006, OQ-032). It requires that library
to be at least `pemba.MIN_SQLITE_VERSION` (`05` §4.3), and refuses an older one with a
`StorageError` naming both versions.

Safe in the same process on the same database file: `sqlite3`, and everything built on it
(SQLAlchemy's `sqlite` / `pysqlite` dialect, Django, `aiosqlite`, `sqlite-utils`). **Not
safe:** `apsw`, `pysqlite3-binary`, `sqlean.py`, or any module, extension or
`ctypes`-loaded library that brings its own SQLite, and any code that `open()`s and
`close()`s the database or its `-wal`/`-shm` files directly (backups included; use
`sqlite3.Connection.backup`). Why: on Linux and macOS, SQLite's locks are POSIX `fcntl`
locks, which belong to the process, and a `close()` of any descriptor on the file
silently releases all of them. SQLite works around this with a process-wide list of open
files, but each SQLite copy keeps its own list. A close by one copy therefore drops the
other's locks: another process can then write at the same time, and one of the commits is
lost or the database corrupted, with no error anywhere. pemba's T3.2 probe lost a
committed transaction exactly this way
(<https://www.sqlite.org/howtocorrupt.html>, §2.2–2.3). Windows locks per handle and is
not affected, but the same rule applies so code stays portable.

---

## 3. CLI (`pemba-cli`, binary name `pemba`)

```text
pemba schema validate <file.zed>           exit 0 ok / 1 errors; prints rustc-style diagnostics
pemba schema fmt <file.zed> [--check]      canonical formatting
pemba test <fixture.yaml>...               runs fixture files (docs/07 §3); exit 1 on failure
pemba check --db app.db <tuple_string>     ad-hoc check against a SQLite store
pemba migrate --db app.db
```
Use `clap` (derive). Output colours via `anstream`/`owo-colors`, disabled when not a TTY
(not yet implemented; output is plain text).

`schema validate` writes diagnostics (with the file path in the location line) and a
summary to stderr, and `<file>: ok` / `<file>: ok with N warnings` to stdout on success.

`test` takes files and directories (searched recursively for `.yaml` / `.yml`), runs each
fixture on every store kind the runner supports (memory only until M2), prints
`ok    <file> [<store>] (<n> checked)` or `FAIL  <file> [<store>]` followed by one line per
failure (`  <file>: <location> \`<assertion>\`: <message>`) to stdout, warnings for skipped
keys to stderr, and a summary line. Paths are printed with `/` separators on every
platform, so output is identical on Windows. An unreadable file is exit 2.

`schema fmt` is **experimental**: it does not modify files. It prints
`pemba_schema::format(source)` to stdout: the canonical form with every comment kept
(`03` §9.1, T3.8). `--check` prints nothing on success and exits 1 with a one-line message
if the file differs from that output, so a canonical file with comments passes. A schema
with errors makes `fmt` print the same diagnostics as `validate` and exit 1. If the
formatter cannot show that its output keeps the schema and every comment
(`FormatError::NotPreserved`, a bug), it prints nothing and exits 2.

Exit codes for every subcommand:

| Code | Meaning |
|---|---|
| 0 | success (warnings alone do not fail `validate`) |
| 1 | input rejected: schema errors, `fmt --check` found a non-canonical file, or a `test` fixture failed (including a fixture that is not valid YAML or uses an unknown key) |
| 2 | usage or I/O error: bad arguments, unreadable file, unwritable output |
