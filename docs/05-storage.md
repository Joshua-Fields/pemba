# 05 — Storage

Crates: traits in `pemba-core::store`; implementations in `pemba-store-memory` and
`pemba-store-sqlite`. Postgres is post-MVP but the traits must not preclude it.

---

## 1. Principles

1. The evaluator talks only to the `ReadStore` trait. It never sees SQL.
2. All write **logic** (validation, preconditions, revision allocation, MVCC updates) lives in
   one place, `pemba-core::write`, generic over the `WriteTxn` trait (ADR-006). Stores only
   supply SQL text and a transaction handle. The memory store implements the same logic
   natively but must pass the same conformance suite (§9).
3. Every read is at an explicit `Revision`, which must lie in `gc_horizon..=head`; outside
   that range a read fails with `StoreError::RevisionOutOfRange` (fail closed: below the
   horizon history is incomplete, above head a later commit could change the answer).
4. Stores are `Send + Sync`.

---

## 2. Traits

```rust
pub trait ReadStore: Send + Sync {
    fn head_revision(&self) -> Result<Revision, StoreError>;
    fn gc_horizon(&self) -> Result<Revision, StoreError>;     // lowest readable revision

    /// Exact tuple exists at `rev`?
    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError>;

    /// Visible stored subject *sets* on `resource#relation` (subject_relation non-empty),
    /// ordered by (subject_type, subject_id, subject_relation) byte-wise.
    fn read_subject_sets(&self, rev: Revision, resource: &ObjectRef, relation: &RelName)
        -> Result<Vec<(ObjectRef, RelName)>, StoreError>;

    /// Distinct related objects on `resource#relation`, wildcards excluded,
    /// subject relation dropped, ordered by (subject_type, subject_id) byte-wise.
    /// Used by arrows.
    fn read_related_objects(&self, rev: Revision, resource: &ObjectRef, relation: &RelName)
        -> Result<Vec<ObjectRef>, StoreError>;

    /// Reverse index read for lookups: *distinct* pairs, ordered. See §6.3 for the SQL.
    fn read_reverse(&self, rev: Revision, f: &ReverseFilter)
        -> Result<Vec<(ObjectRef, RelName)>, StoreError>;   // (resource, relation)

    /// General filtered read for read_relationships / tooling, paginated. Ordered by the six
    /// storage columns (resource_type, resource_id, relation, subject_type, subject_id,
    /// subject_relation) byte-wise; `after` is an exclusive keyset cursor in that order.
    fn read_relationships(&self, rev: Revision, f: &RelationshipFilter,
                          limit: u32, after: Option<&Relationship>)
        -> Result<Vec<Relationship>, StoreError>;

    /// All visible tuples on `resource#relation` (used by lookup_subjects), ordered by
    /// (subject_type, subject_id, subject_relation) byte-wise.
    fn read_tuples(&self, rev: Revision, resource: &ObjectRef, relation: &RelName)
        -> Result<Vec<SubjectRef>, StoreError>;

    /// Counts for schema-change safety checks (§7), grouped by (resource_type, relation,
    /// subject_type, subject_relation, wildcard).
    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError>;

    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError>;

    /// One fixed view of the store for a whole request (T2.3): head, horizon and every read
    /// through it see the same state, whatever commits or GCs meanwhile, in any process.
    /// Dropping it releases what it holds. Default: the store itself (see "Revision checks").
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> { Ok(Box::new(self)) }
}

pub struct ReverseFilter {
    pub subject_type: TypeName,
    pub subject_id: String,                      // may be "*"
    pub subject_relation: SubjectRelMatch,       // None | Exact(rel) | Any
    pub resource_type: Option<TypeName>,
    pub relation: Option<RelName>,
}
pub enum SubjectRelMatch { None, Exact(RelName), Any }

/// A transaction the write logic runs inside.
pub trait WriteTxn {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError>; // rows affected
    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError>;
    fn last_insert_id(&mut self) -> Result<i64, StoreError>;
}
pub enum SqlValue { Null, Integer(i64), Text(String) }

pub trait SqlDialect: Send + Sync {
    fn placeholder_style(&self) -> PlaceholderStyle; // Question (SQLite), Dollar, Format (%s)
    fn placeholder(&self, n: usize) -> String;       // default: from placeholder_style
    fn revision_strategy(&self) -> RevisionStrategy; // AllocateNow (SQLite) | AssignAtCommit
    fn table(&self, name: &str) -> String;           // applies TABLE_PREFIX and optional schema
}
// AllocateNow: §5 below. AssignAtCommit (Postgres, ADR-015): rows carry created_batch /
// deleted_batch, the write queues a pemba_pending row, and write::write returns
// Applied::Pending { batch, xid } (docs/10-postgres.md §1.6.1). Statement text never holds a
// literal `%` (Format style). delete_matching and write_schema refuse AssignAtCommit until
// T5.5.

/// Typed mutations every store supports (OQ-014). Each call is one atomic transaction.
pub trait WritableStore: ReadStore {
    /// Steps 2–6 of §5; validation (step 1) is the engine's job. With
    /// `schema_version`, commits only if the store is still at it (§5 step 2b, T2.6).
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error>;
    /// New schema version at a new revision (kind 'schema'). Inside the same transaction,
    /// first passes the live counts at head to `check`, which may refuse (§7). Hash
    /// de-duplication is the engine's job.
    fn write_schema(&self, source: &str, hash: &str, check: SchemaCheck<'_>)
        -> Result<StoredSchema, Error>;
    /// Delete-by-filter (`02` §5): at most `limit` live matches, storage order, one revision.
    fn delete_matching(&self, filter: &RelationshipFilter, limit: usize)
        -> Result<DeleteResult, Error>;
    /// §8 step 1, its own committed transaction. Never lowers; keep_from > head ->
    /// InvalidRequest. Commits no revision. Returns the horizon after.
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error>;
    /// §8 step 2: removes at most `max_rows` rows deleted at or before the stored horizon.
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error>;
    /// Provided: raise_gc_horizon, then gc_batch(GC_BATCH_ROWS = 10_000) until one removes 0.
    fn gc(&self, keep_from: Revision) -> Result<GcStats, Error> { /* default */ }
}
pub type SchemaCheck<'a> = &'a dyn Fn(&[RelationCount]) -> Result<(), Error>;
pub struct DeleteResult { pub deleted: u64, pub more: bool, pub revision: Revision }
pub struct GcStats { pub rows_removed: u64, pub horizon: Revision, pub batches: u64 }
// ReadStore also gains (T2.6) `fn schema_version(&self) -> Result<u64, StoreError>`:
// latest schema version (0 if none), as of the snapshot when called through one; the
// default reads `current_schema()`. SqlDialect gains `fn row_id(&self) -> &'static str`
// (`rowid` / `ctid`) for batched DELETE/UPDATE. StoreError gains
// `SchemaChanged { expected, found }`, retryable.

/// SQL-backed stores: the shared write logic (§5) runs over their `WriteTxn`.
pub trait SqlStore: WritableStore {
    /// Engine-managed transaction: begin, run f, commit (rollback on Err).
    fn with_write_txn<T>(&self, f: &mut dyn FnMut(&mut dyn WriteTxn) -> Result<T, Error>)
        -> Result<T, Error>;
    fn dialect(&self) -> &dyn SqlDialect;
    /// Foreign writes (§10): is this caller transaction on the store's database?
    /// Mismatch -> Err(ForeignConnectionMismatch). Added in T2.2 (OQ-026).
    fn check_same_database(&self, txn: &mut dyn WriteTxn) -> Result<(), Error>;
}
```

`StoreError::Conflict { message }` (T2.2, OQ-026): the transaction lost a lock race — on
SQLite, `SQLITE_BUSY_SNAPSHOT` (a deferred transaction read before another connection
committed and cannot upgrade to a write lock) or `SQLITE_BUSY`/`SQLITE_LOCKED` past the busy
timeout. `StoreError::is_retryable()` is true for it: roll back and rerun the whole
transaction.

Result order (T2.1): every read that returns a list has a fixed order, compared byte-wise on
the storage columns (SQLite `BINARY` collation; wildcard id `*`, absent subject relation
`''`), as given on each method above; `read_reverse` orders by (resource_type, resource_id,
relation). The order is part of the contract, not a convenience: the evaluator visits
subject sets and arrow targets in the order the store returns them, and short-circuits, so
the order decides which branch's error (`MaxDepthExceeded`, `FanoutExceeded`, ...) a `check`
reports and how many reads it makes. Two stores returning the same set in different orders
would give different answers to the same request (property 5). The conformance suite pins
the order (`read_order_bytewise`), and the SQLite queries (§6) have a matching `ORDER BY`
that the forward index satisfies without a sort.

Object safety (decided in T1.1): `ReadStore` and `WritableStore` are object-safe, so the
evaluator, tooling and the conformance suite can use `&dyn ReadStore` / `&dyn WritableStore`.
`SqlStore` is not: `with_write_txn` stays generic over the closure's result so callers can
return a revision, schema version or GC stats without boxing, and nothing needs a
`dyn SqlStore` (`Engine<S>` is generic; foreign writes use a `WriteTxn` directly). The
memory store implements `WritableStore` natively and not `SqlStore`.

Revision checks (OQ-015):
- The **engine** resolves a request's `Consistency` to exactly one revision and validates it
  against `head_revision()` and `gc_horizon()` **once per request**, at the start
  (`docs/02-data-model.md` §4), reporting `RevisionNotYetAvailable` / `RevisionExpired`.
  These two calls are authoritative: on SQLite they read the database (the horizon query can
  piggyback on the head query), never a cache.
- Every store read still checks its `rev` against `gc_horizon..=head` (§1 rule 3) as a
  fail-closed backstop, but that check must cost **no extra query**: it compares against a
  horizon cached in memory (the memory store holds it in `Inner`; the SQLite store caches
  `pemba_meta.gc_horizon`, refreshed by its own `gc` and whenever `gc_horizon()` is called)
  and the head last observed the same way.
- A backstop failure because a GC moved the horizon after the engine resolved the revision
  surfaces as `RevisionExpired`, not as a store error.
- Cross-process GC (closed in T2.3): a cached horizon cannot see a GC run by **another
  process** between the engine's check and a later read, and such a read could silently
  miss rows. So every request (`check`, the whole of `check_bulk`, `read_relationships`, and
  the lookups from T2.4) takes one `ReadStore::snapshot()` first, resolves its revision
  against the snapshot's `head_revision()`/`gc_horizon()`, and makes every read through it.
  On SQLite the snapshot is one leased reader connection inside `BEGIN DEFERRED`, whose
  first statement is the head/horizon query, so WAL pins all later reads to that state: a
  GC committed afterwards, by any process, is invisible to the request (SQLite never
  checkpoints over pages an open reader still needs), and the next request sees the new
  horizon and fails `Exact(r)` below it with `RevisionExpired`. The snapshot's backstop
  compares against its own head and horizon. Dropping it rolls the read transaction back
  and returns the connection to the pool, on every exit path including errors and panics.
  The memory store keeps the default (`snapshot()` is the store itself): its range check
  reads the authoritative horizon under the same lock as the rows, so a concurrent GC fails
  the read closed (`RevisionExpired`) rather than hiding rows. Tested by
  `pemba-store-sqlite/tests/snapshot.rs` (two `SqliteStore`s on one file standing in for
  two processes).
- A request holds one reader connection for its whole duration, so at most
  `SqliteConfig::reader_pool` requests read at once; the rest wait for a connection.
- **Long read transactions hold back WAL checkpoints.** A checkpoint cannot copy WAL frames
  past the oldest open reader's snapshot, nor reset the WAL while a reader uses it, so a
  long-lived snapshot lets the WAL file grow under write load. `check` is short (one
  request, bounded by `max_check_reads`); **T2.4 lookups must watch this**: a
  `lookup_resources` page visiting up to `max_lookup_nodes` nodes holds its snapshot for
  the whole page, and pagination must not keep a snapshot open between pages (each page
  is its own request at `Exact(first_page.revision)`).

---

## 3. In-memory store (`pemba-store-memory`)

- `RwLock<Inner>`; `Inner` holds `Vec<Row>` plus indexes:
  - `fwd: HashMap<(TypeName, ObjectId, RelName), Vec<RowIdx>>`
  - `rev: HashMap<(TypeName, String /*subject id*/), Vec<RowIdx>>` (no subject relation in
    the key, so `SubjectRelMatch::Any` is a single lookup)
  - `live: HashMap<Relationship, RowIdx>` (the partial unique index)
  - `head: Revision`, `horizon: Revision`, `schema_versions: Vec<StoredSchema>` (no
    per-revision info: nothing reads `committed_at`/`kind` through the traits)
- `Row { rel: Relationship, created: Revision, deleted: Option<Revision> }`.
- Implements the write algorithm (§5) directly against `Inner` under the write lock.
- Must pass the store conformance suite (§9). Not persistent; per-process.

---

## 4. SQLite schema (`pemba-store-sqlite`)

Two stores share this layout and every statement (T3.2a). Statement text, migrations and
row decoding live in the driver-free `pemba-sqlite-common`:
- `pemba-store-sqlite` (Rust): `rusqlite` with the `bundled` feature (one SQLite version
  everywhere).
- `pemba-store-pysqlite` (Python, used by `Engine.sqlite()`): CPython's own `sqlite3`
  module, so the process has one SQLite library (§4.4, ADR-006, OQ-032).

Requires SQLite ≥ `MIN_SQLITE_VERSION` = 3.26.0 (§4.3).

```sql
CREATE TABLE IF NOT EXISTS pemba_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- rows: ('storage_version','1'), ('gc_horizon','0'), ('created_at', ISO-8601)

CREATE TABLE IF NOT EXISTS pemba_revisions (
    rev          INTEGER PRIMARY KEY AUTOINCREMENT,   -- AUTOINCREMENT: never reuse committed ids
    committed_at TEXT NOT NULL,                       -- ISO-8601 UTC, informational
    kind         TEXT NOT NULL                        -- 'write' | 'schema' | 'delete_filter'
);

CREATE TABLE IF NOT EXISTS pemba_relationships (
    resource_type    TEXT    NOT NULL,
    resource_id      TEXT    NOT NULL,
    relation         TEXT    NOT NULL,
    subject_type     TEXT    NOT NULL,
    subject_id       TEXT    NOT NULL,                -- '*' for wildcard
    subject_relation TEXT    NOT NULL DEFAULT '',     -- '' = none (never NULL: keeps unique index simple)
    created_rev      INTEGER NOT NULL,
    deleted_rev      INTEGER                          -- NULL = live
);

-- At most one live copy of a tuple.
CREATE UNIQUE INDEX IF NOT EXISTS pemba_rel_live
    ON pemba_relationships (resource_type, resource_id, relation,
                             subject_type, subject_id, subject_relation)
    WHERE deleted_rev IS NULL;

-- Forward reads (check, arrows). Subject columns follow the resource columns so the index
-- order is the §2 result order (no sort step); created_rev/deleted_rev make it covering.
CREATE INDEX IF NOT EXISTS pemba_rel_fwd
    ON pemba_relationships (resource_type, resource_id, relation,
                             subject_type, subject_id, subject_relation,
                             created_rev, deleted_rev);

-- Reverse reads (lookups).
CREATE INDEX IF NOT EXISTS pemba_rel_rev
    ON pemba_relationships (subject_type, subject_id, subject_relation,
                             resource_type, relation, created_rev);

-- GC.
CREATE INDEX IF NOT EXISTS pemba_rel_deleted
    ON pemba_relationships (deleted_rev) WHERE deleted_rev IS NOT NULL;

CREATE TABLE IF NOT EXISTS pemba_schema (
    version     INTEGER PRIMARY KEY AUTOINCREMENT,
    source      TEXT    NOT NULL,
    hash        TEXT    NOT NULL,          -- hex sha-256 of normalised source
    created_rev INTEGER NOT NULL
);
```

Connection setup (every connection the engine opens):
```sql
PRAGMA journal_mode = WAL;          -- readers never block the writer
PRAGMA synchronous = NORMAL;        -- safe with WAL; configurable to FULL
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;         -- ms, configurable
```
Do **not** change `journal_mode` on a foreign (app-owned) connection; document that the app
should use WAL for concurrency.

### 4.1 Migrations

- `pemba_meta.storage_version` tracks the engine's table layout. MVP ships version `1`.
  Version `1` uses the `pemba_` prefix. Databases created before the rename (ADR-012,
  OQ-001) carry the codename's prefix and are not migrated; none were ever published.
- `SqliteStore::migrate(path, &config)` (explicit) creates/upgrades tables inside one
  transaction.
- `SqliteStore::open()` checks the version: missing tables → `Err(NotMigrated)` unless
  `SqliteConfig::auto_migrate = true` (default `true` for SQLite file convenience, `false`
  documented as recommended for production). Migration is a store concern; the engine has
  no `migrate` and no `auto_migrate` (OQ-025 (d)).
- `SqliteStore::open_read_only(path, config)` (T4.1, for inspection tools such as `pemba
  check --db`) opens an existing file with every connection read-only, the "writer"
  included: it never creates the file, never migrates (`auto_migrate` is ignored, so an
  old or empty database is `NotMigrated`, a newer one `StorageVersionTooNew`), and every
  write fails with `Store`. The file must already be in WAL mode, which every database
  `open()` (or the Python store) created is; otherwise `InvalidRequest` says so, since a
  read-only connection cannot switch it. The database file itself is never changed, but as
  for any SQLite reader of a WAL database, empty `-wal` / `-shm` files are created beside
  it if missing (so the directory must be writable) and may be left behind; no WAL frame is
  written. `immutable=1` would avoid them only by turning off locking, which is unsafe
  while another process writes.
- Newer `storage_version` than the library knows → `Err(StorageVersionTooNew)`.
- The Postgres store (M5) follows the same rules (`NotMigrated`, `StorageVersionTooNew`,
  an explicit `PostgresStore::migrate`) with its own layout and runner, except that
  `PostgresConfig::auto_migrate` defaults to `false`: the database is shared and app
  roles usually cannot run DDL, so migrating is a deploy step (OQ-047, `10` §3.4).
- Migrations are a static ordered list of SQL scripts in
  `pemba-sqlite-common/migrations/`. The shared runner executes them one statement at a
  time (a statement ends at a line ending in `;`), because a driver's `execute` takes one
  statement. A test pins the result to SQLite's own script runner.

### 4.2 Connections

- One **writer** connection behind a `Mutex` (SQLite has one writer anyway).
- A small **reader pool** (default 4, configurable) of connections — hand-rolled
  `Mutex<Vec<Connection>>` + condvar or `r2d2`/`r2d2_sqlite` (justify if added).
- In-memory SQLite is **not supported** (decided in T2.1, OQ-025): it cannot use WAL, a
  plain `:memory:` database is per-connection, and a shared-cache one replaces WAL's
  snapshot isolation with table locks. Opening one fails with `InvalidRequest` (any
  connection whose `journal_mode` does not come back `wal` does). Use `pemba-store-memory`.
- Reader connections are opened read-only, after the writer has set WAL and migrated.
- A reader returned to the pool inside a transaction is rolled back (or discarded if that
  fails) so no caller inherits an old snapshot.

### 4.3 Minimum SQLite version

The newest feature each statement needs:

| Feature | Where | SQLite |
|---|---|---|
| WAL (`journal_mode = WAL`) | every connection | 3.7.0 |
| `SAVEPOINT` / `RELEASE` / `ROLLBACK TO` | write algorithm (§5) | 3.6.8 |
| `INDEXED BY`, `AUTOINCREMENT`, `?NNN` parameters | reads (§6), tables | 3.6 |
| multi-row `VALUES` | migration 1 | 3.7.11 |
| partial indexes (`pemba_rel_live`, `pemba_rel_deleted`) | migration 1 | 3.8.0 |
| row values (`(a, b, …) > (?, ?, …)`) | `read_relationships` cursor (§6) | 3.15.0 |
| upsert (`ON CONFLICT … DO NOTHING` / `DO UPDATE`, with a partial-index target) | Touch (§5), migration runner | 3.24.0 |

Not used (they would raise the floor): `sqlite_schema` (3.33; the runner reads
`sqlite_master`), `RETURNING` (3.35), `UPDATE … FROM` (3.33), window functions (3.25),
`NULLS FIRST` (3.30), generated columns (3.31), JSON operators (3.38).

The SQL needs 3.24.0. **`MIN_SQLITE_VERSION` is 3.26.0**, the oldest release CI runs
(Rocky Linux 8: Python 3.11 with its system SQLite 3.26.0, job `python-oldest`).
`Engine.sqlite()` refuses an older `sqlite3` with a `StorageError` naming both versions.
It also refuses a `sqlite3` built without thread support (`sqlite3.threadsafety == 0`),
because connections move between threads. Raising the floor needs this table updated and
the CI job moved.

### 4.4 The Python store (`pemba-store-pysqlite`)

The same as §4.2 except that each connection is a `sqlite3.Connection`
(`isolation_level=None`; the store issues `BEGIN IMMEDIATE`, `BEGIN DEFERRED`, `COMMIT`
and `ROLLBACK` itself; `check_same_thread=False`; statement cache 128). Snapshots (T2.3:
one `BEGIN DEFERRED` read transaction per request, head, horizon and schema version read
inside it), the write algorithm (§5) and the GC order (§8) are unchanged. Every statement
takes the GIL. The store never waits for a lock while holding it: the reader pool never
blocks (with nothing idle a request opens a connection, and at most `reader_pool` are
kept), and the writer mutex is taken before any statement. `pemba-py` holds the GIL for
a whole request and serialises requests per engine (OQ-033 (b)). Dropping the store
closes every connection at once. The conformance suite (§9) and property 5 run against
it in an embedded interpreter.

---

## 5. Write algorithm (shared, in `pemba-core::write`)

Inside one transaction (`WriteTxn`), in this order. Implemented once as
`pemba_core::write::write`; steps 3–6 run inside `SAVEPOINT pemba_write`, rolled back to on
any error, so a failed write leaves a caller-owned transaction as it found it (OQ-026).
`write_schema` (§7) and `gc` (§8) are in the same module. The function never begins, commits
or rolls back the transaction itself.

```text
1. validate all ops against the current schema (docs/02 §6)        -- no SQL
2. reject duplicates within the batch                              -- no SQL
2b. (T2.6) if the engine passed the schema version it validated against:
     SELECT COALESCE(MAX(version), 0) FROM pemba_schema
     differs -> Err(Store(SchemaChanged))  -- the engine reloads, revalidates, retries (≤3)
3. for each precondition:
     SELECT 1 FROM pemba_relationships WHERE <filter> AND deleted_rev IS NULL LIMIT 1
     MustExist & none found  -> Err(PreconditionFailed)
     MustNotExist & found    -> Err(PreconditionFailed)
4. INSERT INTO pemba_revisions (committed_at, kind) VALUES (?, 'write')
   rev = last_insert_id()
5. for each op:
   Create: INSERT ... (created_rev = rev)          -- unique index violation -> Err(AlreadyExists)
   Touch:  INSERT ... ON CONFLICT DO NOTHING       -- needs the partial unique index:
           INSERT INTO pemba_relationships (...) VALUES (...)
           ON CONFLICT (resource_type, resource_id, relation, subject_type, subject_id, subject_relation)
           WHERE deleted_rev IS NULL DO NOTHING
   Delete: UPDATE pemba_relationships SET deleted_rev = ?rev
           WHERE resource_type=? AND resource_id=? AND relation=? AND subject_type=?
             AND subject_id=? AND subject_relation=? AND deleted_rev IS NULL
6. return rev
```

Use **prepared, cached statements** (`prepare_cached`) on the native path. Statement text is
produced by `SqlDialect` so Postgres can reuse the algorithm.

SQLite (T2.2): `SqliteStore`'s `WritableStore` methods call these functions through
`with_write_txn`, which runs `BEGIN IMMEDIATE` on the writer connection; `SqliteTxn` (a
`WriteTxn` over a `&rusqlite::Transaction`, statements via `prepare_cached`) serves both that
path and a caller's transaction for `Engine::write_in`. `committed_at` is computed in Rust
(UTC, millisecond ISO-8601) so the statement stays portable.

### 5.1 Why revision order equals commit order on SQLite

SQLite allows one write transaction at a time. Step 4 runs while the transaction holds the
write lock, which it keeps until commit. Two transactions therefore cannot interleave their
revision allocations and commits. **Requirement for foreign transactions:** the first
statement of the write must acquire the write lock (true for any INSERT), and the app must
not hold a read snapshot older than another writer's commit. Document this; `BEGIN IMMEDIATE`
is recommended for app transactions that call `engine.write(..., conn=...)`.

If a deferred caller transaction has read (e.g. the precondition `SELECT` of step 3) and
another connection commits before its first write, SQLite refuses the lock upgrade with
`SQLITE_BUSY_SNAPSHOT`: the write fails with the retryable `StoreError::Conflict` and never
commits on a stale precondition (tested in T2.2,
`precondition_race_in_a_deferred_transaction_is_a_retryable_conflict`).

### 5.2 Head revision

`SELECT COALESCE(MAX(rev), 0) FROM pemba_revisions` on a reader connection. Because of WAL,
a reader sees only committed revisions, so head never points at an uncommitted batch.

---

## 6. Read queries (SQLite)

Visibility predicate, used everywhere (`?R` = revision):
```sql
created_rev <= ?R AND (deleted_rev IS NULL OR deleted_rev > ?R)
```

### 6.1 `exists`
```sql
SELECT 1 FROM pemba_relationships
WHERE resource_type=? AND resource_id=? AND relation=?
  AND subject_type=? AND subject_id=? AND subject_relation=?
  AND created_rev <= ?R AND (deleted_rev IS NULL OR deleted_rev > ?R)
LIMIT 1;
```

### 6.2 `read_subject_sets` / `read_related_objects` / `read_tuples`
```sql
SELECT subject_type, subject_id, subject_relation FROM pemba_relationships
WHERE resource_type=? AND resource_id=? AND relation=?
  AND <visible at ?R>
  [AND subject_relation <> '']                 -- read_subject_sets
ORDER BY subject_type, subject_id, subject_relation;

-- read_related_objects
SELECT DISTINCT subject_type, subject_id FROM pemba_relationships
WHERE resource_type=? AND resource_id=? AND relation=?
  AND <visible at ?R> AND subject_id <> '*'
ORDER BY subject_type, subject_id;
```
All three, and §6.1, are a covering-index seek on `pemba_rel_fwd` with no
`USE TEMP B-TREE` step: the `ORDER BY` (and `DISTINCT`) is the index order, which is the §2
result order.

### 6.3 `read_reverse`
```sql
SELECT DISTINCT resource_type, resource_id, relation FROM pemba_relationships
WHERE subject_type=? AND subject_id=?
  [AND subject_relation = ?]                   -- Exact(rel); None -> = ''; Any -> omit
  [AND resource_type = ?] [AND relation = ?]
  AND <visible at ?R>
ORDER BY resource_type, resource_id, relation;
```
Seeks `pemba_rel_rev`; the `DISTINCT`/`ORDER BY` needs a temporary B-tree because
`resource_id` is not in that index (accepted: the result set is one subject's resources).

Every §6 query names its index with `INDEXED BY` (`pemba_rel_fwd` for §6.1–6.2,
`pemba_rel_rev` for §6.3). Without it SQLite answers `read_reverse` with a resource type
from `pemba_rel_fwd` (index order matches the `ORDER BY`, but it ranges over every resource
of that type), and `ANALYZE` statistics from the app (`PRAGMA optimize`) could move any
query; with it a lost index is a prepare error, not a slow plan. `EXPLAIN QUERY PLAN` tests
in `pemba-store-sqlite` pin every query (all twelve §6.3 filter shapes) by index name, with
and without `ANALYZE` statistics.

---

## 7. Schema storage and safe changes

- `write_schema(source)`: compile → if hash equals the store's current one, no-op returning
  the current version.
- Else, **inside the schema write's own transaction** (`WritableStore::write_schema`'s
  `check`), read the live counts at head:
  ```sql
  SELECT resource_type, relation, subject_type, subject_relation,
         CASE WHEN subject_id = '*' THEN 1 ELSE 0 END AS wildcard, COUNT(*)
  FROM pemba_relationships WHERE deleted_rev IS NULL
  GROUP BY resource_type, relation, subject_type, subject_relation, wildcard;
  ```
  and refuse with `Error::SchemaChangeOrphans` (code `SCHEMA_CHANGE_ORPHANS`, `03` §10) if
  any group is **orphaned: a tuple of its shape would fail write validation (`02` §6)
  under the new schema**. One `OrphanReport` per group: resource type, relation, subject
  form (`user`, `user:*`, `group#member`), count, and the validation code. That covers a
  removed type (as resource or subject), a removed relation, a relation that became a
  permission of the same name, a removed allowed type, subject set (including `x#p` whose
  `p` was deleted from `x`) or wildcard (wildcards are grouped apart from objects, OQ-015
  (c)). Removing a permission is allowed exactly when no live tuple uses it as a subject
  relation. Running the check in the write transaction means no write can slip in between.
  On SQLite the write lock guarantees that. On Postgres (READ COMMITTED) a **schema lock**
  does (`SqlDialect::schema_lock`, OQ-049): the schema write takes it exclusively before
  reading the counts, and every write that checks the version (§5 step 2b) takes it shared
  before reading the version, each held to commit.
- Insert a `pemba_revisions` row (`kind='schema'`) and the `pemba_schema` row with that
  `created_rev`.
- **Writes check the version** (§5 step 2b): every engine write passes the version it
  validated against; a mismatch aborts the transaction (`SchemaChanged`), and the engine
  reloads the schema, revalidates and retries (up to 3 attempts). If validation fails
  against a cached schema, the engine compares the store's version and revalidates
  against the newer one before refusing. Together with the orphan check, **no live tuple
  at or above the current schema's revision contradicts it** (stale tuples are impossible
  at head).
- **Reads take the schema from their snapshot.** The engine caches compiled schemas by
  version; each request, after taking its snapshot (§2), reads
  `ReadStore::schema_version()` through it and uses that version (compiling it from the
  snapshot's `current_schema()` if the cache holds another). Schema and data therefore
  always come from the same moment, and a schema written by another process is used by the
  next request without any reload call.
- MVP: checks always evaluate with the **current** schema, even for `Exact(old_rev)` reads.
  Below the current schema's `created_rev` a read can meet tuples that schema rejects
  (written before the change, deleted before it was allowed); the evaluator fails closed
  on them with `StaleRelationship` (`04` §3.3). A relation that was removed entirely, or
  became a permission, is never read by the current schema, so its old tuples simply do
  not count at `Exact(old)` (OQ-030 (d)).

---

## 8. Garbage collection

`Engine::gc(keep_from: Revision) -> GcStats` (T2.6), in this order:

1. **Raise the horizon first**, in its own committed transaction:
   `UPDATE pemba_meta SET value = ?keep_from WHERE key = 'gc_horizon'` (never lowering it;
   `keep_from > head` → `InvalidRequest`).
2. **Then delete in batches**, each its own transaction, until a batch removes nothing:
   ```sql
   DELETE FROM pemba_relationships WHERE rowid IN (
     SELECT rowid FROM pemba_relationships
     WHERE deleted_rev IS NOT NULL AND deleted_rev <= ?horizon LIMIT 10000);
   ```
   (`rowid` is `SqlDialect::row_id()`; `ctid` on Postgres.) The bound is the **stored**
   horizon, so a batch can never remove a row some readable revision still needs.

Why this order: deleting first (the pre-T2.6 spec) let a reader pass the horizon check and
then read half-deleted history, a silent wrong answer. With the horizon committed first, a
request whose snapshot was taken before it sees all of its revision (WAL keeps its pages),
and every later request at `Exact(r < keep_from)` fails with `RevisionExpired`: never a
partial answer. Tested by `pemba-store-sqlite/tests/multi_process.rs`
(`a_reader_mid_gc_gets_the_full_answer_or_revision_expired`, pausing between batches
through a test wrapper). If a batch fails, the horizon is already raised; the remaining
rows are removed by the next GC and change no answer.

- After GC, `Exact(r)` with `r < keep_from` → `Err(RevisionExpired)`.
- Optionally prune `pemba_revisions` rows below the horizon except the newest (keep head).
- No automatic GC in the MVP. Document a recommended cron (e.g. keep 24h of history).

---

## 9. Store conformance suite

`pemba-testkit::store_conformance::run::<S: WritableStore>(factory)` — a generic test
suite every store must pass, invoked from each store crate's tests. Covers:
visibility at revisions, Touch/Create/Delete semantics, duplicates, preconditions,
monotonic revisions, read_reverse filters, pagination ordering, GC horizon errors,
schema version storage, concurrent writer + readers (threads) never observing a partial batch,
reads outside `gc_horizon..=head` failing. Cases drive the store only through
`&dyn WritableStore`; `run` gives each case a fresh store and reports every failing case.

---

## 10. Foreign-transaction mode (Python, SQLite)

`engine.write(updates, conn=sqlite3_connection)` (T3.2b):

1. `pemba-py` wraps the Python connection in a `WriteTxn`: `PyTxn::foreign`, the adapter
   the native Python writes use (T3.2a) with foreign error mapping. The app's connection
   and the engine's connections are the same SQLite library (§4.4), which is what makes
   sharing a file in one process safe:
   - `execute` → `conn.execute(sql, params)`; return `cursor.rowcount`.
   - `query` → `conn.execute(...)` then `fetchall()`.
   - `last_insert_id` → `SELECT last_insert_rowid()` on the same connection.
   - Errors are classified by `sqlite3.Error.sqlite_errorcode`:
     `SQLITE_CONSTRAINT_UNIQUE` → `StoreError::UniqueViolation`; `SQLITE_BUSY_SNAPSHOT`,
     `SQLITE_BUSY` and `SQLITE_LOCKED` → the retryable `StoreError::Conflict`; any other
     error → `StoreError::Foreign { message }` (OQ-034 (d)).
2. Runs `Engine::write_in`, i.e. the shared write algorithm (§5), with the GIL **held**
   and the engine's request lock taken, like every SQLite-engine request (§4.4).
3. Does **not** commit or roll back. Returns a `PendingRevision` (ADR-015), already resolved
   on SQLite because the number is allocated inside the write (§5, step 4).
4. The caller commits. Until then, readers (engine connections) do not see the revision;
   `AtLeast(rev)` before commit → `Err(RevisionNotYetAvailable)` (`ConsistencyError`).
   After a rollback the revision is void and a later write may be given the same number.

Guard rails:
- `conn` must be a `sqlite3.Connection` (or subclass), else `TypeError`; `conn=` on a
  memory engine is a `ValueError` (OQ-034 (b)).
- The engine verifies the foreign connection points at the **same database file**
  (`PRAGMA database_list` on both, compare `file` paths after canonicalisation). Mismatch
  (including an in-memory connection) → `Err(ForeignConnectionMismatch)`. The check runs
  on **every** write, not cached by `id(conn)`: ids are reused after garbage collection,
  so a cache could trust a connection to another file (OQ-034 (a), resolved).
- If the foreign connection has no open transaction (`conn.in_transaction` is false,
  whatever its `isolation_level`), raise `ValueError` explaining that the write would not
  be atomic with app data — unless `allow_autocommit=True` is passed, in which case the
  write commits on its own (OQ-034 (c)).

Postgres later: same pattern with psycopg (`%s` placeholders via dialect) and revision order
needing a transaction-level advisory lock or an `xid8`-snapshot scheme — open question.
