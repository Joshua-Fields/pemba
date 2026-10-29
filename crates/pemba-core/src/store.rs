//! Store traits (`docs/05-storage.md` §2).
//!
//! The evaluator talks only to [`ReadStore`]. Writes, schema versions and GC go through
//! [`WritableStore`]. SQL-backed stores additionally implement [`SqlStore`], which hands the
//! shared write logic a [`WriteTxn`] so the same SQL can run on the engine's connection or
//! inside the application's own transaction (ADR-006).
//!
//! # Object safety
//!
//! [`ReadStore`] and [`WritableStore`] are object-safe: every method is concrete, so the
//! evaluator and tooling can take `&dyn ReadStore` / `&dyn WritableStore` where that keeps
//! code size down. [`SqlStore`] is deliberately **not** object-safe: `with_write_txn` is
//! generic over the closure's result so callers can return whatever the transaction
//! produces (a revision, a schema version, GC stats) without boxing. Nothing needs a
//! `dyn SqlStore`: `Engine<S>` is generic over its store, and foreign-transaction writes
//! use a [`WriteTxn`] directly. If a `dyn SqlStore` is ever needed, fix the closure's
//! result to `Result<Revision, Error>` as `docs/05-storage.md` §2 allows.
//!
//! # Revisions
//!
//! Every read takes an explicit revision `rev` and sees exactly the tuples visible at it:
//! `created_rev <= rev && (deleted_rev is none || deleted_rev > rev)`
//! (`docs/02-data-model.md` §4). `rev` must lie in `gc_horizon()..=head_revision()`;
//! outside that range a read fails with [`StoreError::RevisionOutOfRange`] rather than
//! returning an answer from incomplete history (below the horizon) or one that a later
//! commit could change (above head).
//!
//! # Snapshots
//!
//! A request reads through [`ReadStore::snapshot`]: one consistent view of the store for
//! the whole request, so its revision check and all its reads see the same state even if
//! another process commits or garbage-collects meanwhile (`docs/05-storage.md` §2,
//! "Revision checks").
//!
//! # Ordering
//!
//! Results are ordered by their storage columns compared byte-wise, matching SQLite's
//! `BINARY` collation: a wildcard subject has id `"*"` and an absent subject relation is
//! `""` (see [`subject_columns`] and [`relationship_columns`]). The order is part of the
//! contract: the evaluator visits results in it and short-circuits, so it decides which
//! error a `check` reports and how many reads it makes; every store must return the same
//! order (`docs/05-storage.md` §2, conformance case `read_order_bytewise`).

use std::cmp::Ordering;

use crate::error::Error;
use crate::model::{
    ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, TypeName,
    WriteRequest,
};

/// A store failure, as opposed to a rejected request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A read named a revision outside `horizon..=head`.
    #[error("revision {requested} is outside the readable range {horizon}..={head}")]
    RevisionOutOfRange {
        /// The revision asked for.
        requested: Revision,
        /// The GC horizon at the time of the read.
        horizon: Revision,
        /// The head revision at the time of the read.
        head: Revision,
    },
    /// The database rejected a row because a unique index already had it.
    #[error("unique constraint violated: {message}")]
    UniqueViolation {
        /// Driver message.
        message: String,
    },
    /// An error from a caller-supplied (foreign) connection.
    #[error("foreign connection error: {message}")]
    Foreign {
        /// Driver message.
        message: String,
    },
    /// A thread panicked while holding the store's lock, so its state cannot be trusted.
    #[error("store lock poisoned by a panic in another thread")]
    Poisoned,
    /// The revision counter is at `u64::MAX`; no further revision can be allocated.
    #[error("revision counter exhausted")]
    RevisionsExhausted,
    /// The database driver failed (I/O, locking, SQL error).
    #[error("database error: {message}")]
    Backend {
        /// Driver message.
        message: String,
    },
    /// Another writer got in first: the transaction lost a lock upgrade because its
    /// snapshot is stale (SQLite `SQLITE_BUSY_SNAPSHOT`), or waited on a lock past its
    /// busy timeout. Nothing was written by the failed statement. Retryable: roll back the
    /// whole transaction and run it again (re-reading whatever it decided on).
    #[error("write conflict, retry the transaction: {message}")]
    Conflict {
        /// Driver message.
        message: String,
    },
    /// The schema changed between the engine validating a write and the write's
    /// transaction: the batch was checked against version `expected`, the store is at
    /// `found`. Nothing was written. Retryable: validate against the new schema and write
    /// again (the engine does this itself).
    #[error("schema changed from version {expected} to {found} during the write")]
    SchemaChanged {
        /// The schema version the batch was validated against.
        expected: u64,
        /// The store's current schema version.
        found: u64,
    },
    /// The engine's own tables hold a value the engine cannot interpret, e.g. a
    /// non-numeric `storage_version`; reading on would mean guessing.
    #[error("corrupt store metadata: {message}")]
    Corrupt {
        /// What was found and where.
        message: String,
    },
}

impl StoreError {
    /// Whether running the whole transaction again may succeed
    /// ([`StoreError::Conflict`], [`StoreError::SchemaChanged`]).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Conflict { .. } | Self::SchemaChanged { .. })
    }
}

/// Which stored subject relations a [`ReverseFilter`] matches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SubjectRelMatch {
    /// Only subjects without a relation (objects and wildcards).
    None,
    /// Only subject sets with exactly this relation.
    Exact(RelName),
    /// Any subject relation, or none.
    Any,
}

/// Reverse-index read: which `(resource, relation)` pairs have this subject?
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ReverseFilter {
    /// Subject type.
    pub subject_type: TypeName,
    /// Subject id; `"*"` matches wildcard subjects.
    pub subject_id: String,
    /// Subject relation to match.
    pub subject_relation: SubjectRelMatch,
    /// Restrict to resources of this type.
    pub resource_type: Option<TypeName>,
    /// Restrict to this resource relation.
    pub relation: Option<RelName>,
}

/// Live-tuple count for one `(resource_type, relation, subject kind)` group, used by
/// schema-change safety checks (`docs/05-storage.md` §7).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct RelationCount {
    /// Resource type.
    pub resource_type: TypeName,
    /// Resource relation.
    pub relation: RelName,
    /// Subject type.
    pub subject_type: TypeName,
    /// Subject relation, if the tuples are subject sets.
    pub subject_relation: Option<RelName>,
    /// Whether the tuples are wildcards (`t:*`), counted apart from objects of `t` so
    /// that removing `t:*` from a relation's allowed types is caught.
    pub wildcard: bool,
    /// Number of visible tuples in the group.
    pub count: u64,
}

/// One stored schema version.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct StoredSchema {
    /// 1 for the first schema written, then increasing by one.
    pub version: u64,
    /// Schema source exactly as written.
    pub source: String,
    /// Hex SHA-256 of the normalised source (`CompiledSchema::hash`).
    pub hash: String,
    /// The revision the schema write committed at.
    pub created_rev: Revision,
}

/// Outcome of [`WritableStore::gc`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct GcStats {
    /// Deleted tuple rows physically removed.
    pub rows_removed: u64,
    /// The GC horizon after the call.
    pub horizon: Revision,
    /// Delete transactions run, each removing at most [`GC_BATCH_ROWS`] rows; the last
    /// one removes none.
    pub batches: u64,
}

/// Rows one GC delete transaction removes at most (`docs/05-storage.md` §8), so no GC
/// holds the write lock for long.
pub const GC_BATCH_ROWS: usize = 10_000;

/// Outcome of a delete-by-filter (`docs/02-data-model.md` §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DeleteResult {
    /// Live tuples deleted (at `revision`).
    pub deleted: u64,
    /// Whether live tuples matching the filter remain (the limit cut the delete short).
    pub more: bool,
    /// The revision the delete committed at.
    pub revision: Revision,
}

/// Checks the live tuple counts at head (grouped as [`ReadStore::count_by_relation`])
/// inside a schema write's transaction; an `Err` aborts the write.
pub type SchemaCheck<'a> = &'a dyn Fn(&[RelationCount]) -> Result<(), Error>;

/// Read access at explicit revisions. All the evaluator ever sees of a store.
pub trait ReadStore: Send + Sync {
    /// Highest committed revision; `Revision(0)` for an empty store.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the backend fails.
    fn head_revision(&self) -> Result<Revision, StoreError>;

    /// Lowest readable revision; `Revision(0)` until the first GC.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the backend fails.
    fn gc_horizon(&self) -> Result<Revision, StoreError>;

    /// Is this exact tuple visible at `rev`?
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError>;

    /// Visible subject *sets* on `resource#relation`, ordered by
    /// (subject type, subject id, subject relation).
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError>;

    /// Distinct objects related to `resource#relation`, ordered by (type, id). Subject
    /// relations are dropped and wildcards excluded; used by arrows.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError>;

    /// Distinct `(resource, relation)` pairs with a visible tuple matching `f`, ordered by
    /// (resource type, resource id, relation).
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError>;

    /// Up to `limit` visible relationships matching `f`, in [`relationship_columns`] order,
    /// starting strictly after `after` when given (keyset pagination).
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn read_relationships(
        &self,
        rev: Revision,
        f: &RelationshipFilter,
        limit: u32,
        after: Option<&Relationship>,
    ) -> Result<Vec<Relationship>, StoreError>;

    /// All visible subjects on `resource#relation`, ordered by [`subject_columns`].
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn read_tuples(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<SubjectRef>, StoreError>;

    /// Visible tuple counts grouped as in [`RelationCount`], in its `Ord` order; groups
    /// with no tuples are omitted.
    ///
    /// # Errors
    ///
    /// [`StoreError::RevisionOutOfRange`] if `rev` is outside `gc_horizon..=head`; any
    /// other [`StoreError`] if the backend fails.
    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError>;

    /// The latest stored schema, if any has been written.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the backend fails.
    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError>;

    /// The latest stored schema's version, `0` if none. Through a
    /// [`snapshot`](Self::snapshot), the version as of that snapshot, so a request can
    /// evaluate with the schema that matches its data. The default reads
    /// [`current_schema`](Self::current_schema); stores override it with a cheaper query.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the backend fails.
    fn schema_version(&self) -> Result<u64, StoreError> {
        Ok(self.current_schema()?.map_or(0, |s| s.version))
    }

    /// A view of the store in which every call, [`head_revision`](Self::head_revision)
    /// and [`gc_horizon`](Self::gc_horizon) included, sees one fixed state: no commit or
    /// GC that happens after the snapshot is taken becomes visible through it, whichever
    /// process makes it. The engine takes one per request, resolves the request's
    /// revision against it and makes every read through it. Dropping the snapshot
    /// releases whatever it holds (on SQLite, a read transaction).
    ///
    /// The default returns the store itself, which is right for a store whose reads at a
    /// revision cannot change while it is readable and whose range check is authoritative
    /// (the memory store: a GC is seen at once and fails the read closed). A store that
    /// can be changed by another process, or caches its range, must override it.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] if the backend fails to start the snapshot.
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        Ok(Box::new(self))
    }
}

/// A shared reference reads like the store itself, so a store can be its own snapshot.
impl<T: ReadStore + ?Sized> ReadStore for &T {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        (**self).head_revision()
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        (**self).gc_horizon()
    }

    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
        (**self).exists(rev, rel)
    }

    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        (**self).read_subject_sets(rev, resource, relation)
    }

    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError> {
        (**self).read_related_objects(rev, resource, relation)
    }

    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        (**self).read_reverse(rev, f)
    }

    fn read_relationships(
        &self,
        rev: Revision,
        f: &RelationshipFilter,
        limit: u32,
        after: Option<&Relationship>,
    ) -> Result<Vec<Relationship>, StoreError> {
        (**self).read_relationships(rev, f, limit, after)
    }

    fn read_tuples(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<SubjectRef>, StoreError> {
        (**self).read_tuples(rev, resource, relation)
    }

    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
        (**self).count_by_relation(rev)
    }

    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
        (**self).current_schema()
    }

    fn schema_version(&self) -> Result<u64, StoreError> {
        (**self).schema_version()
    }

    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        (**self).snapshot()
    }
}

/// Mutations. Each method is one atomic transaction that commits at a new revision
/// (except the GC methods, which commit none).
pub trait WritableStore: ReadStore {
    /// Applies a batch: steps 2–6 of the write algorithm (`docs/05-storage.md` §5).
    ///
    /// Validating the ops against the schema (step 1) is the caller's job. If
    /// `schema_version` is given, the transaction first checks that it is still the
    /// store's current schema version, so a batch validated against one schema never
    /// commits after another replaced it (`docs/05-storage.md` §7). Allocates the next
    /// revision even if no op changed anything, and returns it.
    ///
    /// # Errors
    ///
    /// Checked in this order, and on any error nothing is written and no revision is
    /// committed: a relationship appearing twice → [`Error::DuplicateInBatch`]; the schema
    /// version moved → [`StoreError::SchemaChanged`]; a precondition not holding over live
    /// tuples → [`Error::PreconditionFailed`]; a `Create` of a live tuple →
    /// [`Error::AlreadyExists`]. Backend failures are [`Error::Store`].
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error>;

    /// Stores a new schema version at a new revision. Inside the same transaction, first
    /// passes the live tuple counts at head to `check`, which can refuse the change
    /// (orphaned tuples, `docs/05-storage.md` §7); no write can slip in between. Skipping
    /// an unchanged hash is the engine's job.
    ///
    /// # Errors
    ///
    /// Whatever `check` returns; [`Error::Store`] if the backend fails.
    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error>;

    /// Marks every live tuple matching `filter` deleted, at most `limit` of them in storage
    /// order, at one new revision, atomically (`docs/02-data-model.md` §5).
    ///
    /// # Errors
    ///
    /// [`Error::Store`] if the backend fails.
    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error>;

    /// GC step 1 (`docs/05-storage.md` §8): raises the GC horizon to `keep_from` in its own
    /// committed transaction, before any row is removed, so no reader can pass the horizon
    /// check and then read half-collected history. Never lowers it. Returns the horizon
    /// after the call.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] if `keep_from` is above head; [`Error::Store`] if the
    /// backend fails.
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error>;

    /// GC step 2: physically removes at most `max_rows` tuples deleted at or before the
    /// current horizon, in one transaction; returns how many. They are invisible at every
    /// readable revision, so removing them changes no answer.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] if the backend fails.
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error>;

    /// The revision the commit gave `batch`, which transaction `xid` wrote inside a
    /// caller's transaction ([`RevisionStrategy::AssignAtCommit`],
    /// `docs/10-postgres.md` §1.6.4). Runs on the store's own connection, never inside the
    /// caller's transaction.
    ///
    /// # Errors
    ///
    /// [`Error::RevisionPending`] while `xid` is still open; [`Error::WriteRolledBack`] if
    /// it rolled back or the batch was rolled back to a savepoint; [`Error::Store`] if the
    /// backend fails. The default, for stores that number writes inside them and so never
    /// hand out a batch, fails closed with [`Error::Store`].
    fn resolve_batch(&self, batch: u64, xid: u64) -> Result<Revision, Error> {
        let _ = xid;
        Err(Error::Store(StoreError::Backend {
            message: format!("this store does not assign revisions at commit (batch {batch})"),
        }))
    }

    /// Garbage-collects history below `keep_from`: [`raise_gc_horizon`](Self::raise_gc_horizon),
    /// then [`gc_batch`](Self::gc_batch) of [`GC_BATCH_ROWS`] until a batch removes none
    /// (so the last batch is always empty).
    /// After it, reads at `Exact(r)` with `r < keep_from` fail with `RevisionExpired`.
    ///
    /// # Errors
    ///
    /// As the two steps. If a batch fails, the horizon is already raised and the rows it
    /// did not reach stay until the next GC; no answer changes.
    fn gc(&self, keep_from: Revision) -> Result<GcStats, Error> {
        let horizon = self.raise_gc_horizon(keep_from)?;
        let mut stats = GcStats {
            rows_removed: 0,
            horizon,
            batches: 0,
        };
        loop {
            let n = self.gc_batch(GC_BATCH_ROWS)?;
            stats.rows_removed += n;
            stats.batches += 1;
            if n == 0 {
                return Ok(stats);
            }
        }
    }
}

/// A parameter or result value in the portable SQL subset.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SqlValue {
    /// SQL `NULL`.
    Null,
    /// A 64-bit integer.
    Integer(i64),
    /// Text.
    Text(String),
}

/// A transaction the shared write logic runs inside: the engine's own connection or a
/// caller-supplied one (ADR-006).
pub trait WriteTxn {
    /// Runs a statement; returns the number of rows affected.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] from the database or connection.
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError>;
    /// Runs a query; returns all rows.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] from the database or connection.
    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError>;
    /// Row id of the last `INSERT` on this transaction.
    ///
    /// # Errors
    ///
    /// A [`StoreError`] from the database or connection.
    fn last_insert_id(&mut self) -> Result<i64, StoreError>;
}

/// How a SQL store numbers a write (ADR-015, `docs/10-postgres.md` §1.6.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RevisionStrategy {
    /// The write inserts a `pemba_revisions` row whose id is the revision. Correct only
    /// where one writer holds the write lock from that insert to commit (SQLite, `05` §5.1).
    AllocateNow,
    /// The write marks its rows with a batch id and queues a `pemba_pending` row; a commit
    /// trigger maps the batch to the next revision, so revisions follow commit order with
    /// concurrent writers (Postgres).
    AssignAtCommit,
}

/// How a driver spells positional parameters.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlaceholderStyle {
    /// `?`, bound in order: SQLite, Python `sqlite3` (qmark).
    Question,
    /// `$1`, `$2`, …: the Rust `postgres` crate, asyncpg.
    Dollar,
    /// `%s`, bound in order: psycopg, Django. Statement text must then contain no other
    /// `%`, which the driver would read as a placeholder.
    Format,
}

impl PlaceholderStyle {
    /// The `n`th (1-based) placeholder in this style.
    #[must_use]
    pub fn placeholder(self, n: usize) -> String {
        match self {
            Self::Question => "?".to_owned(),
            Self::Dollar => format!("${n}"),
            Self::Format => "%s".to_owned(),
        }
    }
}

/// The SQL differences between databases that the shared statements depend on.
pub trait SqlDialect: Send + Sync {
    /// How this dialect's driver spells parameters.
    fn placeholder_style(&self) -> PlaceholderStyle;
    /// The `n`th (1-based) positional placeholder, in [`placeholder_style`](Self::placeholder_style).
    fn placeholder(&self, n: usize) -> String {
        self.placeholder_style().placeholder(n)
    }
    /// How writes get their revision.
    fn revision_strategy(&self) -> RevisionStrategy;
    /// A table name with the table prefix and optional schema applied.
    fn table(&self, name: &str) -> String;
    /// The column naming a physical row, for batched deletes and updates
    /// (`DELETE ... WHERE <row_id> IN (SELECT <row_id> ... LIMIT n)`): `rowid` for SQLite,
    /// `ctid` for Postgres.
    fn row_id(&self) -> &'static str;
    /// A statement that takes, until the transaction ends, the lock ordering schema writes
    /// (`exclusive`) against writes that check the schema version (shared), or `None` where
    /// the database's own write lock already serialises every write (SQLite).
    ///
    /// Without it, under READ COMMITTED a schema write's orphan check and a concurrent
    /// write's version check could each pass before the other commits (OQ-049).
    fn schema_lock(&self, exclusive: bool) -> Option<String> {
        let _ = exclusive;
        None
    }
    /// A `FROM` item, aliased `v`, that reads many tuples from one parameter, so a write
    /// sends one statement per op kind instead of one per op (OQ-056); `None` (the
    /// default) where per-op statements are cheap enough (SQLite, in process).
    ///
    /// The `n`th placeholder holds a JSON array with one element per tuple: an array of
    /// its six [`relationship_columns`], in that order, as strings. The item yields one
    /// row per element, with those six columns named as in the relationships table
    /// (`resource_type`, `resource_id`, `relation`, `subject_type`, `subject_id`,
    /// `subject_relation`).
    fn bulk_rows(&self, n: usize) -> Option<String> {
        let _ = n;
        None
    }
}

/// A store whose writes are SQL, so the shared write algorithm can drive it.
/// Not object-safe; see the [module docs](self#object-safety).
pub trait SqlStore: WritableStore {
    /// Engine-managed transaction: begin, run `f`, commit; roll back if `f` fails.
    ///
    /// # Errors
    ///
    /// Whatever `f` returns, or [`Error::Store`] if begin or commit fails.
    fn with_write_txn<T>(
        &self,
        f: &mut dyn FnMut(&mut dyn WriteTxn) -> Result<T, Error>,
    ) -> Result<T, Error>;

    /// The SQL dialect for statements run through [`with_write_txn`](Self::with_write_txn).
    fn dialect(&self) -> &dyn SqlDialect;

    /// Verifies that a caller-supplied transaction runs on this store's database, so a
    /// foreign write lands where the engine reads (`docs/05-storage.md` §10).
    ///
    /// # Errors
    ///
    /// [`Error::ForeignConnectionMismatch`] if it does not, or cannot be shown to;
    /// [`Error::Store`] if asking the connection fails.
    fn check_same_database(&self, txn: &mut dyn WriteTxn) -> Result<(), Error>;
}

/// A subject's storage columns `(subject_type, subject_id, subject_relation)`: `"*"` is the
/// id of a wildcard, `""` an absent relation. Store results order subjects by these,
/// compared byte-wise.
pub fn subject_columns(s: &SubjectRef) -> (&str, &str, &str) {
    match s {
        SubjectRef::Object(o) => (o.object_type.as_str(), o.object_id.as_str(), ""),
        SubjectRef::Set { object, relation } => (
            object.object_type.as_str(),
            object.object_id.as_str(),
            relation.as_str(),
        ),
        SubjectRef::Wildcard { object_type } => (object_type.as_str(), "*", ""),
    }
}

/// A relationship's six storage columns in index order: resource type, resource id,
/// relation, then [`subject_columns`]. [`ReadStore::read_relationships`] orders by these.
pub fn relationship_columns(r: &Relationship) -> [&str; 6] {
    let (st, sid, srel) = subject_columns(&r.subject);
    [
        r.resource.object_type.as_str(),
        r.resource.object_id.as_str(),
        r.relation.as_str(),
        st,
        sid,
        srel,
    ]
}

/// Compares relationships in [`relationship_columns`] order.
pub fn cmp_relationships(a: &Relationship, b: &Relationship) -> Ordering {
    relationship_columns(a).cmp(&relationship_columns(b))
}

impl ReverseFilter {
    /// Matches this subject; resource type and relation restrictions default to `None`.
    pub fn new(
        subject_type: TypeName,
        subject_id: String,
        subject_relation: SubjectRelMatch,
    ) -> Self {
        Self {
            subject_type,
            subject_id,
            subject_relation,
            resource_type: None,
            relation: None,
        }
    }
}

impl RelationCount {
    /// Constructs this store value from all its fields.
    pub fn new(
        resource_type: TypeName,
        relation: RelName,
        subject_type: TypeName,
        subject_relation: Option<RelName>,
        wildcard: bool,
        count: u64,
    ) -> Self {
        Self {
            resource_type,
            relation,
            subject_type,
            subject_relation,
            wildcard,
            count,
        }
    }
}

impl StoredSchema {
    /// Constructs this store value from all its fields.
    pub fn new(version: u64, source: String, hash: String, created_rev: Revision) -> Self {
        Self {
            version,
            source,
            hash,
            created_rev,
        }
    }
}

impl GcStats {
    /// Constructs this store value from all its fields.
    pub fn new(rows_removed: u64, horizon: Revision, batches: u64) -> Self {
        Self {
            rows_removed,
            horizon,
            batches,
        }
    }
}

impl DeleteResult {
    /// Constructs this store value from all its fields.
    pub fn new(deleted: u64, more: bool, revision: Revision) -> Self {
        Self {
            deleted,
            more,
            revision,
        }
    }
}
