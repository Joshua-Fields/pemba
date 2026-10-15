# 05 — Storage

Crates: traits in `warden-core::store`; implementations in `warden-store-memory` and
`warden-store-sqlite`. Postgres is post-MVP but the traits must not preclude it.

---

## 1. Principles

1. The evaluator talks only to the `ReadStore` trait. It never sees SQL.
2. All write **logic** (validation, preconditions, revision allocation, MVCC updates) lives in
   one place, `warden-core::write`, generic over the `WriteTxn` trait (ADR-006). Stores only
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

    /// Visible stored subject *sets* on `resource#relation` (subject_relation non-empty).
    fn read_subject_sets(&self, rev: Revision, resource: &ObjectRef, relation: &RelName)
        -> Result<Vec<(ObjectRef, RelName)>, StoreError>;

    /// Distinct related objects on `resource#relation`, wildcards excluded,
    /// subject relation dropped. Used by arrows.
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

    /// All visible tuples on `resource#relation` (used by lookup_subjects).
    fn read_tuples(&self, rev: Revision, resource: &ObjectRef, relation: &RelName)
        -> Result<Vec<SubjectRef>, StoreError>;

    /// Counts for schema-change safety checks (§7), grouped by (resource_type, relation,
    /// subject_type, subject_relation, wildcard).
    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError>;

    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError>;
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
    fn placeholder(&self, n: usize) -> String;   // "?" for SQLite, "$n" for Postgres
    fn table(&self, name: &str) -> String;       // applies TABLE_PREFIX and optional schema
}

/// Typed mutations every store supports (OQ-014). Each call is one atomic transaction.
pub trait WritableStore: ReadStore {
    /// Steps 2–6 of §5; validation (step 1) is the engine's job.
    fn write(&self, req: &WriteRequest) -> Result<Revision, Error>;
    /// New schema version at a new revision (kind 'schema'). Always writes; hash
    /// de-duplication and orphan checks (§7) are the engine's job.
    fn write_schema(&self, source: &str, hash: &str) -> Result<StoredSchema, Error>;
    /// §8. Horizon never moves down; keep_from > head -> InvalidRequest. Commits no revision.
    fn gc(&self, keep_from: Revision) -> Result<GcStats, Error>;
}

/// SQL-backed stores: the shared write logic (§5) runs over their `WriteTxn`.
pub trait SqlStore: WritableStore {
    /// Engine-managed transaction: begin, run f, commit (rollback on Err).
    fn with_write_txn<T>(&self, f: &mut dyn FnMut(&mut dyn WriteTxn) -> Result<T, Error>)
        -> Result<T, Error>;
    fn dialect(&self) -> &dyn SqlDialect;
}
```

Object safety (decided in T1.1): `ReadStore` and `WritableStore` are object-safe, so the
evaluator, tooling and the conformance suite can use `&dyn ReadStore` / `&dyn WritableStore`.
`SqlStore` is not: `with_write_txn` stays generic over the closure's result so callers can
return a revision, schema version or GC stats without boxing, and nothing needs a
`dyn SqlStore` (`Engine<S>` is generic; foreign writes use a `WriteTxn` directly). The
memory store implements `WritableStore` natively and not `SqlStore`.

---

## 3. In-memory store (`warden-store-memory`)

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

## 4. SQLite schema (`warden-store-sqlite`)

Driver: `rusqlite` with the `bundled` feature (consistent SQLite version everywhere).
Requires SQLite ≥ 3.35 (partial indexes, `RETURNING`; bundled satisfies this).

```sql
CREATE TABLE IF NOT EXISTS warden_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- rows: ('storage_version','1'), ('gc_horizon','0'), ('created_at', ISO-8601)

CREATE TABLE IF NOT EXISTS warden_revisions (
    rev          INTEGER PRIMARY KEY AUTOINCREMENT,   -- AUTOINCREMENT: never reuse ids
    committed_at TEXT NOT NULL,                       -- ISO-8601 UTC, informational
    kind         TEXT NOT NULL                        -- 'write' | 'schema' | 'delete_filter'
);

CREATE TABLE IF NOT EXISTS warden_relationships (
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
CREATE UNIQUE INDEX IF NOT EXISTS warden_rel_live
    ON warden_relationships (resource_type, resource_id, relation,
                             subject_type, subject_id, subject_relation)
    WHERE deleted_rev IS NULL;

-- Forward reads (check, arrows).
CREATE INDEX IF NOT EXISTS warden_rel_fwd
    ON warden_relationships (resource_type, resource_id, relation, created_rev);

-- Reverse reads (lookups).
CREATE INDEX IF NOT EXISTS warden_rel_rev
    ON warden_relationships (subject_type, subject_id, subject_relation,
                             resource_type, relation, created_rev);

-- GC.
CREATE INDEX IF NOT EXISTS warden_rel_deleted
    ON warden_relationships (deleted_rev) WHERE deleted_rev IS NOT NULL;

CREATE TABLE IF NOT EXISTS warden_schema (
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

- `warden_meta.storage_version` tracks the engine's table layout. MVP ships version `1`.
- `Engine::migrate()` (explicit) creates/upgrades tables inside one transaction.
- `Engine::open()` checks the version: missing tables → `Err(NotMigrated)` unless
  `EngineConfig::auto_migrate = true` (default `true` for SQLite file convenience, `false`
  documented as recommended for production).
- Newer `storage_version` than the library knows → `Err(StorageVersionTooNew)`.
- Migrations are a static ordered list of SQL scripts in `warden-store-sqlite/migrations/`.

### 4.2 Connections

- One **writer** connection behind a `Mutex` (SQLite has one writer anyway).
- A small **reader pool** (default 4, configurable) of connections — hand-rolled
  `Mutex<Vec<Connection>>` + condvar or `r2d2`/`r2d2_sqlite` (justify if added).
- `:memory:` SQLite is supported only with a pool size of 1 and a shared-cache URI
  (`file::memory:?cache=shared`); otherwise each connection would see a different database.
  Simpler: for in-memory use, recommend `warden-store-memory`.

---

## 5. Write algorithm (shared, in `warden-core::write`)

Inside one transaction (`WriteTxn`), in this order:

```text
1. validate all ops against the current schema (docs/02 §6)        -- no SQL
2. reject duplicates within the batch                              -- no SQL
3. for each precondition:
     SELECT 1 FROM warden_relationships WHERE <filter> AND deleted_rev IS NULL LIMIT 1
     MustExist & none found  -> Err(PreconditionFailed)
     MustNotExist & found    -> Err(PreconditionFailed)
4. INSERT INTO warden_revisions (committed_at, kind) VALUES (?, 'write')
   rev = last_insert_id()
5. for each op:
   Create: INSERT ... (created_rev = rev)          -- unique index violation -> Err(AlreadyExists)
   Touch:  INSERT ... ON CONFLICT DO NOTHING       -- needs the partial unique index:
           INSERT INTO warden_relationships (...) VALUES (...)
           ON CONFLICT (resource_type, resource_id, relation, subject_type, subject_id, subject_relation)
           WHERE deleted_rev IS NULL DO NOTHING
   Delete: UPDATE warden_relationships SET deleted_rev = ?rev
           WHERE resource_type=? AND resource_id=? AND relation=? AND subject_type=?
             AND subject_id=? AND subject_relation=? AND deleted_rev IS NULL
6. return rev
```

Use **prepared, cached statements** (`prepare_cached`) on the native path. Statement text is
produced by `SqlDialect` so Postgres can reuse the algorithm.

### 5.1 Why revision order equals commit order on SQLite

SQLite allows one write transaction at a time. Step 4 runs while the transaction holds the
write lock, which it keeps until commit. Two transactions therefore cannot interleave their
revision allocations and commits. **Requirement for foreign transactions:** the first
statement of the write must acquire the write lock (true for any INSERT), and the app must
not hold a read snapshot older than another writer's commit. Document this; `BEGIN IMMEDIATE`
is recommended for app transactions that call `engine.write(..., conn=...)`.

### 5.2 Head revision

`SELECT COALESCE(MAX(rev), 0) FROM warden_revisions` on a reader connection. Because of WAL,
a reader sees only committed revisions, so head never points at an uncommitted batch.

---

## 6. Read queries (SQLite)

Visibility predicate, used everywhere (`?R` = revision):
```sql
created_rev <= ?R AND (deleted_rev IS NULL OR deleted_rev > ?R)
```

### 6.1 `exists`
```sql
SELECT 1 FROM warden_relationships
WHERE resource_type=? AND resource_id=? AND relation=?
  AND subject_type=? AND subject_id=? AND subject_relation=?
  AND created_rev <= ?R AND (deleted_rev IS NULL OR deleted_rev > ?R)
LIMIT 1;
```

### 6.2 `read_subject_sets` / `read_related_objects` / `read_tuples`
```sql
SELECT subject_type, subject_id, subject_relation FROM warden_relationships
WHERE resource_type=? AND resource_id=? AND relation=?
  AND <visible at ?R>
  [AND subject_relation <> '']                 -- read_subject_sets
  [AND subject_id <> '*']                      -- read_related_objects (+ DISTINCT on type,id)
ORDER BY subject_type, subject_id, subject_relation;
```

### 6.3 `read_reverse`
```sql
SELECT DISTINCT resource_type, resource_id, relation FROM warden_relationships
WHERE subject_type=? AND subject_id=?
  [AND subject_relation = ?]                   -- Exact(rel); None -> = ''; Any -> omit
  [AND resource_type = ?] [AND relation = ?]
  AND <visible at ?R>
ORDER BY resource_type, resource_id, relation;
```
Covered by `warden_rel_rev`. Verify with `EXPLAIN QUERY PLAN` in a test that each query
uses an index (no full `SCAN warden_relationships`).

---

## 7. Schema storage and safe changes

- `write_schema(source)`: compile → if hash equals current, no-op returning current version.
- Else compute the diff of removed relations / allowed types / types, then inside one write
  transaction run `count_by_relation`-style queries at head:
  ```sql
  SELECT resource_type, relation, subject_type, subject_relation,
         subject_id = '*' AS wildcard, COUNT(*)
  FROM warden_relationships WHERE deleted_rev IS NULL
  GROUP BY resource_type, relation, subject_type, subject_relation, wildcard;
  ```
  and refuse with `E_SCHEMA_CHANGE_ORPHANS` if any removed element has rows.
- Insert a `warden_revisions` row (`kind='schema'`) and the `warden_schema` row with that
  `created_rev`.
- The engine caches the compiled schema in an `ArcSwap` (or `RwLock<Arc<_>>`); readers grab
  an `Arc` at request start. Multi-process deployments (several gunicorn workers on one file)
  must detect schema changes made by other processes: at request start, compare the latest
  `warden_schema.version` (cheap query, or piggyback on the head-revision query) and reload
  if it changed.
- MVP: checks always evaluate with the **current** schema, even for `Exact(old_rev)` reads.

---

## 8. Garbage collection

- `Engine::gc(keep_from: Revision) -> GcStats`: 
  ```sql
  DELETE FROM warden_relationships WHERE deleted_rev IS NOT NULL AND deleted_rev <= ?keep_from;
  UPDATE warden_meta SET value = ?keep_from WHERE key = 'gc_horizon';
  ```
  in batches of 10 000 rows per transaction to avoid long write locks.
- After GC, `Exact(r)` with `r < keep_from` → `Err(RevisionExpired)`.
- Optionally prune `warden_revisions` rows below the horizon except the newest (keep head).
- No automatic GC in the MVP. Document a recommended cron (e.g. keep 24h of history).

---

## 9. Store conformance suite

`warden-testkit::store_conformance::run::<S: WritableStore>(factory)` — a generic test
suite every store must pass, invoked from each store crate's tests. Covers:
visibility at revisions, Touch/Create/Delete semantics, duplicates, preconditions,
monotonic revisions, read_reverse filters, pagination ordering, GC horizon errors,
schema version storage, concurrent writer + readers (threads) never observing a partial batch,
reads outside `gc_horizon..=head` failing. Cases drive the store only through
`&dyn WritableStore`; `run` gives each case a fresh store and reports every failing case.

---

## 10. Foreign-transaction mode (Python, SQLite)

`engine.write(updates, conn=sqlite3_connection)`:

1. `warden-py` wraps the Python connection in `PyDbApiTxn: WriteTxn`:
   - `execute` → `cursor.execute(sql, params)`; return `cursor.rowcount`.
   - `query` → `cursor.execute(...)` then `fetchall()`.
   - `last_insert_id` → `cursor.lastrowid`.
   - Map `sqlite3.IntegrityError` → `StoreError::UniqueViolation`; other DB-API errors →
     `StoreError::Foreign{ message }`.
2. Runs the shared write algorithm (§5) with the GIL **held** (it calls Python).
3. Does **not** commit or roll back. Returns the revision.
4. The caller commits. Until then, readers (engine connections) do not see the revision;
   `AtLeast(rev)` before commit → `Err(RevisionNotYetAvailable)`.

Guard rails:
- The engine must verify the foreign connection points at the **same database file**
  (`PRAGMA database_list` on both, compare `file` paths after canonicalisation). Mismatch →
  `Err(ForeignConnectionMismatch)`. Do this once per connection object (cache by `id(conn)`).
- If the foreign connection is in autocommit mode (`conn.isolation_level is None` and no
  open transaction / `conn.in_transaction == False`), raise `ValueError` explaining that the
  write would not be atomic with app data — unless `allow_autocommit=True` is passed.

Postgres later: same pattern with psycopg (`%s` placeholders via dialect) and revision order
needing a transaction-level advisory lock or an `xid8`-snapshot scheme — open question.
