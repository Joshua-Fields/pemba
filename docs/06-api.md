# 06 — Public APIs (Rust, Python, CLI)

Public surface is frozen once 0.1 ships. Before then, change it only by editing this file in
the same change (CLAUDE.md rule 10).

---

## 1. Rust (`warden` facade crate re-exports `warden-core` + chosen stores)

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
        where S: SqlStore;                                   // SQL stores only (05 §2)
    pub fn delete_relationships(&self, filter: RelationshipFilter) -> Result<DeleteResult, Error>;

    // ---- reads ----
    pub fn check(&self, req: CheckRequest) -> Result<CheckResponse, Error>;
    pub fn check_bulk(&self, items: Vec<CheckItem>, c: Consistency) -> Result<Vec<Result<bool, Error>>, Error>;
    pub fn lookup_resources(&self, req: LookupResourcesRequest) -> Result<LookupResourcesResponse, Error>;
    pub fn lookup_subjects(&self, req: LookupSubjectsRequest) -> Result<LookupSubjectsResponse, Error>;
    pub fn read_relationships(&self, req: ReadRelationshipsRequest) -> Result<ReadRelationshipsResponse, Error>;
    pub fn head_revision(&self) -> Result<Revision, Error>;

    // ---- maintenance ----
    pub fn migrate(&self) -> Result<(), Error>;
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
    pub auto_migrate: bool,             // true
}
```

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

`check_bulk` returns `Err` for the whole call on `NoSchema`, consistency errors, `Store`,
and `RevisionExpired` raised mid-call by GC; every other error (`InvalidRequest`,
`MaxDepthExceeded`, `NonMonotonicCycle`, `FanoutExceeded`, `CheckReadsExceeded`,
`StaleRelationship`) belongs to
its item (OQ-019).

`write` checks, in order: batch size, schema present, op validation (`02` §6), precondition
filters, then the store's duplicates, preconditions and `Create` conflicts (`02` §5).
`read_relationships` and precondition filters must name only types, relations (not
permissions) and subject relations the schema defines, else `InvalidRequest` (OQ-017).
`write_schema` with the same hash as the current schema is a no-op (`05` §7); the orphan
check arrives in T2.6.

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
    CheckReadsExceeded { max: u64 },               // more than max_check_reads store reads (04 §3.6)
    StaleRelationship { tuple: String, relation: String }, // stored tuple the schema no longer allows (04 §3.3)
    LookupTooLarge { max: usize },
    NotMigrated,
    StorageVersionTooNew { found: u32, supported: u32 },
    ForeignConnectionMismatch,
    Store(StoreError),
}
```
`InvalidRelationship.code` is a `warden_schema::ErrorCode` (`E_UNKNOWN_TYPE`,
`E_UNKNOWN_RELATION`, `E_WRITE_TO_PERMISSION`, `E_SUBJECT_TYPE_NOT_ALLOWED`; `02` §6).
Every variant has a stable machine code (`Error::code() -> &'static str`, e.g.
`"MAX_DEPTH_EXCEEDED"`) used by the Python exception mapping. Codes so far: `SCHEMA_ERROR`,
`NO_SCHEMA`, `INVALID_RELATIONSHIP`, `INVALID_REQUEST`, `DUPLICATE_IN_BATCH`,
`ALREADY_EXISTS`, `PRECONDITION_FAILED`, `BATCH_TOO_LARGE`, `REVISION_NOT_YET_AVAILABLE`,
`REVISION_EXPIRED`, `STORE_ERROR`, `MAX_DEPTH_EXCEEDED`, `NON_MONOTONIC_CYCLE`,
`FANOUT_EXCEEDED`, `CHECK_READS_EXCEEDED`, `STALE_RELATIONSHIP`; variants added later use their name in SCREAMING_SNAKE
case.

---

## 2. Python (`warden` package, built from `warden-py`)

### 2.1 Usage

```python
import sqlite3
from warden import Engine, Consistency

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

### 2.2 Signatures (ship as `warden/__init__.pyi`)

```python
class Revision(int): ...

class Engine:
    @staticmethod
    def sqlite(path: str, *, reader_pool: int = 4, busy_timeout_ms: int = 5000,
               auto_migrate: bool = True, max_depth: int = 50) -> "Engine": ...
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
              conn: Any | None = None, allow_autocommit: bool = False) -> Revision: ...
    def delete_relationships(self, *, resource_type: str, resource_id: str | None = None,
                             relation: str | None = None, subject_type: str | None = None,
                             subject_id: str | None = None,
                             subject_relation: str | None = None) -> "DeleteResult": ...

    @overload
    def check(self, tuple_str: str, /, *, at_least: int | None = None,
              at_exactly: int | None = None) -> bool: ...
    @overload
    def check(self, resource: str, permission: str, subject: str, /, *,
              at_least: int | None = None, at_exactly: int | None = None) -> bool: ...
    def check_detailed(self, resource: str, permission: str, subject: str, /, **consistency) -> "CheckResult": ...
    def check_bulk(self, items: Sequence[tuple[str, str, str]], /, **consistency) -> list[bool | "WardenError"]: ...

    def lookup_resources(self, resource_type: str, permission: str, subject: str, /, *,
                         limit: int | None = None, cursor: str | None = None,
                         **consistency) -> "LookupResult": ...
    def iter_resources(self, resource_type: str, permission: str, subject: str, /,
                       **consistency) -> Iterator[str]: ...          # pages internally at one revision
    def lookup_subjects(self, resource: str, permission: str, subject_type: str, /,
                        **consistency) -> "SubjectsResult": ...
    def read_relationships(self, *, resource_type: str, **filters) -> list["Relationship"]: ...
    def head_revision(self) -> Revision: ...
    def migrate(self) -> None: ...
    def gc(self, keep_from: int) -> "GcStats": ...
```

Rules:
- `at_least` and `at_exactly` are mutually exclusive (`ValueError`). Neither → `Latest`.
- `iter_resources` pins the revision from the first page (`Exact`) for later pages.
- All engine calls release the GIL except foreign-connection writes (they call Python).
- `Engine` is thread-safe; share one instance per process. Not fork-safe: create it after
  forking (document for gunicorn `--preload` users).
- Dataclass-like result types (`Relationship`, `CheckResult`, `LookupResult`,
  `SubjectsResult`, `Diagnostic`, `GcStats`, `DeleteResult`) are frozen `#[pyclass]`es with
  `__repr__`, `__eq__`, `__hash__` where sensible.

### 2.3 Exceptions

```text
WardenError(Exception)               .code: str  (Rust Error::code())
├── SchemaError                      .diagnostics: list[Diagnostic]
├── InvalidRelationshipError
├── InvalidRequestError
├── ConsistencyError                 (RevisionNotYetAvailable, RevisionExpired)
├── EvaluationLimitError             (MaxDepthExceeded, FanoutExceeded, CheckReadsExceeded, LookupTooLarge, NonMonotonicCycle)
├── WriteConflictError               (AlreadyExists, PreconditionFailed, DuplicateInBatch)
├── StorageError                     (Store, NotMigrated, StorageVersionTooNew, ForeignConnectionMismatch)
```
Python argument mistakes raise built-in `ValueError` / `TypeError`, not `WardenError`.

### 2.4 FastAPI helper (stretch, `warden.contrib.fastapi`)

```python
from warden.contrib.fastapi import Authz
authz = Authz(engine, subject=lambda request: f"user:{request.state.user_id}")

@app.get("/documents/{doc_id}")
def get_doc(doc_id: str, _=Depends(authz.require("document", "view", path_param="doc_id"))):
    ...
```
Raises `HTTPException(403)` on deny, `HTTPException(503)` on `WardenError` (fail closed).
Pure Python, optional extra `pip install warden[fastapi]`.

---

## 3. CLI (`warden-cli`, binary name `warden`)

```text
warden schema validate <file.zed>           exit 0 ok / 1 errors; prints rustc-style diagnostics
warden schema fmt <file.zed> [--check]      canonical formatting
warden test <fixture.yaml>...               runs fixture files (docs/07 §3); exit 1 on failure
warden check --db app.db <tuple_string>     ad-hoc check against a SQLite store
warden migrate --db app.db
```
Use `clap` (derive). Output colours via `anstream`/`owo-colors`, disabled when not a TTY
(not yet implemented; output is plain text).

`schema validate` writes diagnostics (with the file path in the location line) and a
summary to stderr, and `<file>: ok` / `<file>: ok with N warnings` to stdout on success.

`test` takes files and directories (searched recursively for `.yaml` / `.yml`), runs each
fixture on every store kind the runner supports (memory only until M2), prints
`ok    <file> [<store>] (<n> checked)` or `FAIL  <file> [<store>]` followed by one line per
failure (`  <file>: <location> \`<assertion>\`: <message>`) to stdout, warnings for skipped
keys to stderr, and a summary line. An unreadable file is exit 2.

`schema fmt` is **experimental**: it drops non-doc comments; it does not modify files.
It prints the canonical form to stdout. Because plain `//` / `/* */` comments are dropped,
`--check` fails on any file containing them (OQ-013; a comment-preserving formatter is
T3.8). `--check` prints nothing on success and exits 1 with a one-line message if the file
differs from its canonical form. A schema with errors makes `fmt` print the same
diagnostics as `validate` and exit 1.

Exit codes for every subcommand:

| Code | Meaning |
|---|---|
| 0 | success (warnings alone do not fail `validate`) |
| 1 | input rejected: schema errors, `fmt --check` found a non-canonical file, or a `test` fixture failed (including a fixture that is not valid YAML or uses an unknown key) |
| 2 | usage or I/O error: bad arguments, unreadable file, unwritable output |
