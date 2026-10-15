//! The `Engine`: schema management, writes and reads over a store (`docs/06-api.md` §1).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use warden_schema::SchemaError;
use warden_schema::ir::CompiledSchema;

use crate::error::Error;
use crate::eval::{
    self, CheckItem, CheckRequest, CheckResponse, EvalStats, Evaluator, Limits, Memo,
};
use crate::model::{
    Consistency, Precondition, Relationship, RelationshipFilter, Revision, SubjectRef, WriteRequest,
};
use crate::store::{ReadStore, SqlStore, StoreError, WritableStore, WriteTxn};
use crate::validate::{validate_filter, validate_ops};

/// Limits and switches (`docs/02-data-model.md` §7). `Default` gives the spec defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// Maximum evaluation depth before `check` fails.
    pub max_depth: u32,
    /// Maximum ops in one [`WriteRequest`].
    pub max_write_batch: usize,
    /// Maximum rows one `delete_relationships` call may delete.
    pub max_delete_by_filter: usize,
    /// Maximum results per page of a lookup or of [`Engine::read_relationships`].
    pub max_lookup_results: u32,
    /// Maximum nodes `lookup_resources` may visit in phase 1.
    pub max_lookup_nodes: usize,
    /// Maximum tuples read per node during `check` before erroring.
    pub max_check_fanout: usize,
    /// Maximum store reads one `check` (or `check_bulk` item) may make before erroring:
    /// a fail-closed backstop on total work (`docs/04-evaluation.md` §3.6).
    pub max_check_reads: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_depth: 50,
            max_write_batch: 1000,
            max_delete_by_filter: 10_000,
            max_lookup_results: 1000,
            max_lookup_nodes: 200_000,
            max_check_fanout: 100_000,
            max_check_reads: 200_000,
        }
    }
}

/// Builds an [`Engine`]; see [`Engine::builder`].
#[derive(Debug)]
pub struct EngineBuilder<S> {
    store: Option<S>,
    config: EngineConfig,
}

impl<S: WritableStore> EngineBuilder<S> {
    /// The store the engine reads and writes. Required.
    #[must_use]
    pub fn store(mut self, store: S) -> Self {
        self.store = Some(store);
        self
    }

    /// Limits; [`EngineConfig::default`] if not set.
    #[must_use]
    pub fn config(mut self, config: EngineConfig) -> Self {
        self.config = config;
        self
    }

    /// Builds the engine and loads the store's current schema, if it has one.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] if no store was given or a limit that must be positive is
    /// zero; [`Error::Schema`] if the stored schema no longer compiles; [`Error::Store`]
    /// if the store fails.
    pub fn build(self) -> Result<Engine<S>, Error> {
        let store = self.store.ok_or_else(|| Error::InvalidRequest {
            message: "Engine::builder() needs a store".to_owned(),
        })?;
        let c = &self.config;
        if c.max_write_batch == 0 || c.max_lookup_results == 0 {
            return Err(Error::InvalidRequest {
                message: "max_write_batch and max_lookup_results must be at least 1".to_owned(),
            });
        }
        let schema = match store.current_schema()? {
            Some(stored) => Some(Arc::new(
                warden_schema::compile(&stored.source)
                    .map_err(Error::Schema)?
                    .schema,
            )),
            None => None,
        };
        Ok(Engine {
            store,
            config: self.config,
            schema: RwLock::new(schema),
        })
    }
}

/// Outcome of [`Engine::write_schema`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaWriteResult {
    /// The schema version now current: 1 for the first schema, then increasing by one.
    pub version: u64,
    /// The revision that version was written at.
    pub revision: Revision,
    /// `false` if the source hashed the same as the current schema, so nothing was
    /// written and `version`/`revision` are the existing ones.
    pub changed: bool,
    /// Compiler warnings (`W_...`), never fatal.
    pub warnings: Vec<SchemaError>,
}

/// A filtered, paginated read of stored relationships.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadRelationshipsRequest {
    /// Which relationships to return.
    pub filter: RelationshipFilter,
    /// Which revision to read at. To page through a consistent snapshot, pass
    /// `Consistency::Exact(first_response.revision)` for later pages.
    pub consistency: Consistency,
    /// Page size; `None` means [`EngineConfig::max_lookup_results`], which is also the
    /// maximum.
    pub limit: Option<u32>,
    /// Start strictly after this relationship: the previous page's
    /// [`ReadRelationshipsResponse::next_cursor`].
    pub cursor: Option<Relationship>,
}

impl ReadRelationshipsRequest {
    /// The first page of `filter` at the latest revision, with the default page size.
    pub fn new(filter: RelationshipFilter) -> Self {
        Self {
            filter,
            consistency: Consistency::Latest,
            limit: None,
            cursor: None,
        }
    }
}

/// One page of [`Engine::read_relationships`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadRelationshipsResponse {
    /// Matching relationships ordered by their storage columns, byte-wise
    /// ([`crate::store::relationship_columns`]).
    pub relationships: Vec<Relationship>,
    /// The revision the read was evaluated at.
    pub revision: Revision,
    /// Cursor for the next page, or `None` if this is the last one.
    pub next_cursor: Option<Relationship>,
}

/// An embedded authorisation engine over store `S`. Thread-safe; share one per process.
#[derive(Debug)]
pub struct Engine<S> {
    store: S,
    config: EngineConfig,
    /// The current compiled schema. Writes hold the read guard for the whole store write,
    /// and `write_schema` the write guard, so no batch is validated against one schema
    /// and committed after another has replaced it.
    schema: RwLock<Option<Arc<CompiledSchema>>>,
}

impl<S: WritableStore> Engine<S> {
    /// Starts building an engine: `Engine::builder().store(store).config(cfg).build()`.
    pub fn builder() -> EngineBuilder<S> {
        EngineBuilder {
            store: None,
            config: EngineConfig::default(),
        }
    }

    /// Compiles `source` and makes it the current schema.
    ///
    /// If it hashes the same as the current schema (differences only in formatting, plain
    /// comments or member order), nothing is written and the current version is returned
    /// with `changed: false`. Otherwise the new version is stored at a new revision.
    ///
    /// Not yet checked: whether the change would orphan stored relationships
    /// (`docs/05-storage.md` §7, task T2.6).
    ///
    /// # Errors
    ///
    /// [`Error::Schema`] with every compile error; [`Error::Store`] if the store fails.
    pub fn write_schema(&self, source: &str) -> Result<SchemaWriteResult, Error> {
        let _span = tracing::info_span!("write_schema", bytes = source.len()).entered();
        let compiled = warden_schema::compile(source).map_err(Error::Schema)?;
        let mut current = self.schema_write();
        if current
            .as_ref()
            .is_some_and(|s| s.hash == compiled.schema.hash)
            && let Some(stored) = self.store.current_schema()?
        {
            return Ok(SchemaWriteResult {
                version: stored.version,
                revision: stored.created_rev,
                changed: false,
                warnings: compiled.warnings,
            });
        }
        let stored = self
            .store
            .write_schema(source, &hex(&compiled.schema.hash))?;
        *current = Some(Arc::new(compiled.schema));
        tracing::debug!(
            version = stored.version,
            revision = stored.created_rev.0,
            "schema written"
        );
        Ok(SchemaWriteResult {
            version: stored.version,
            revision: stored.created_rev,
            changed: true,
            warnings: compiled.warnings,
        })
    }

    /// The current schema source exactly as written, or `None` before the first
    /// [`write_schema`](Self::write_schema).
    ///
    /// # Errors
    ///
    /// [`Error::Store`] if the store fails.
    pub fn read_schema(&self) -> Result<Option<String>, Error> {
        Ok(self.store.current_schema()?.map(|s| s.source))
    }

    /// Applies a batch atomically at one new revision and returns it
    /// (`docs/02-data-model.md` §5). On any error nothing is written.
    ///
    /// # Errors
    ///
    /// Checked in this order: [`Error::BatchTooLarge`] above
    /// [`EngineConfig::max_write_batch`] ops; [`Error::NoSchema`];
    /// [`Error::InvalidRelationship`] for the first op that does not fit the schema;
    /// [`Error::InvalidRequest`] for a precondition filter naming something the schema
    /// does not define; then, from the store, [`Error::DuplicateInBatch`],
    /// [`Error::PreconditionFailed`] and [`Error::AlreadyExists`]. [`Error::Store`] if the
    /// store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn write(&self, req: WriteRequest) -> Result<Revision, Error> {
        let _span = tracing::info_span!(
            "write",
            ops = req.ops.len(),
            preconditions = req.preconditions.len()
        )
        .entered();
        let rev = self.validated(&req, |req| self.store.write(req))?;
        tracing::debug!(revision = rev.0, "batch committed");
        Ok(rev)
    }

    /// Applies a batch inside a transaction the caller owns, e.g. one that also writes the
    /// application's own rows, and returns the revision it will have once committed
    /// (ADR-006, `docs/05-storage.md` §10).
    ///
    /// The engine neither commits nor rolls back `txn`. Until the caller commits, the
    /// revision is invisible: reads at it fail with [`Error::RevisionNotYetAvailable`].
    /// If the caller rolls back, the revision is **void** and a later write may be given
    /// the same number. On error the batch's own statements are undone (to a savepoint),
    /// but the caller should still roll back, since it decided on a write that did not
    /// happen. Nothing in the engine changes because of this call.
    ///
    /// On SQLite, open `txn` with `BEGIN IMMEDIATE`: a deferred transaction that read
    /// before another connection committed cannot take the write lock and fails with a
    /// retryable [`StoreError::Conflict`] (`docs/05-storage.md` §5.1).
    ///
    /// # Errors
    ///
    /// As [`write`](Self::write), plus [`Error::ForeignConnectionMismatch`] if `txn` is not
    /// on the store's database.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn write_in<T: WriteTxn>(&self, txn: &mut T, req: WriteRequest) -> Result<Revision, Error>
    where
        S: SqlStore,
    {
        let _span = tracing::info_span!(
            "write_in",
            ops = req.ops.len(),
            preconditions = req.preconditions.len()
        )
        .entered();
        let rev = self.validated(&req, |req| {
            self.store.check_same_database(txn)?;
            crate::write::write(txn, self.store.dialect(), req)
        })?;
        tracing::debug!(
            revision = rev.0,
            "batch written, pending the caller's commit"
        );
        Ok(rev)
    }

    /// Checks batch size, schema presence, ops and precondition filters, then runs `apply`
    /// while holding the schema read guard so the schema cannot change underneath it.
    fn validated(
        &self,
        req: &WriteRequest,
        apply: impl FnOnce(&WriteRequest) -> Result<Revision, Error>,
    ) -> Result<Revision, Error> {
        if req.ops.len() > self.config.max_write_batch {
            return Err(Error::BatchTooLarge {
                max: self.config.max_write_batch,
                got: req.ops.len(),
            });
        }
        let guard = self.schema_read();
        let schema = guard.as_ref().ok_or(Error::NoSchema)?;
        validate_ops(schema, &req.ops)?;
        for p in &req.preconditions {
            let (Precondition::MustExist(f) | Precondition::MustNotExist(f)) = p;
            validate_filter(schema, f)?;
        }
        let rev = apply(req)?;
        drop(guard);
        Ok(rev)
    }

    /// The highest committed revision; `Revision(0)` for an empty store.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] if the store fails.
    pub fn head_revision(&self) -> Result<Revision, Error> {
        Ok(self.store.head_revision()?)
    }

    /// One page of stored relationships matching the request's filter.
    ///
    /// # Errors
    ///
    /// [`Error::NoSchema`]; [`Error::InvalidRequest`] for a limit of 0 or above
    /// [`EngineConfig::max_lookup_results`], or a filter naming something the schema does
    /// not define; [`Error::RevisionNotYetAvailable`] / [`Error::RevisionExpired`] if the
    /// consistency cannot be met; [`Error::Store`] if the store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn read_relationships(
        &self,
        req: ReadRelationshipsRequest,
    ) -> Result<ReadRelationshipsResponse, Error> {
        let max = self.config.max_lookup_results;
        let limit = req.limit.unwrap_or(max);
        if limit == 0 || limit > max {
            return Err(Error::InvalidRequest {
                message: format!("limit must be between 1 and {max}, got {limit}"),
            });
        }
        let schema = self.current_schema()?;
        validate_filter(&schema, &req.filter)?;
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        // One extra row tells whether another page exists without a second query.
        let mut relationships = snapshot
            .read_relationships(
                revision,
                &req.filter,
                limit.saturating_add(1),
                req.cursor.as_ref(),
            )
            .map_err(read_error)?;
        let more = relationships.len() > limit as usize;
        relationships.truncate(limit as usize);
        let next_cursor = if more {
            relationships.last().cloned()
        } else {
            None
        };
        Ok(ReadRelationshipsResponse {
            relationships,
            revision,
            next_cursor,
        })
    }

    /// Does `req.subject` have `req.permission` on `req.resource`
    /// (`docs/04-evaluation.md` §3)?
    ///
    /// Never answers `false` (or `true`) in place of an error: if anything goes wrong
    /// during evaluation the whole check fails.
    ///
    /// # Errors
    ///
    /// [`Error::NoSchema`]; [`Error::InvalidRequest`] if the resource type, permission,
    /// subject type or subject relation is not defined, or the subject is a wildcard;
    /// [`Error::RevisionNotYetAvailable`] / [`Error::RevisionExpired`] if the consistency
    /// cannot be met; [`Error::MaxDepthExceeded`], [`Error::NonMonotonicCycle`] or
    /// [`Error::FanoutExceeded`] / [`Error::CheckReadsExceeded`] if evaluation hits a limit
    /// or an undefined cycle;
    /// [`Error::StaleRelationship`] if it meets a stored subject set the schema no longer
    /// allows; [`Error::Store`] if the store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn check(&self, req: CheckRequest) -> Result<CheckResponse, Error> {
        let span = tracing::info_span!(
            "check",
            resource = %req.resource,
            permission = %req.permission,
            subject = %req.subject,
            revision = tracing::field::Empty,
            allowed = tracing::field::Empty,
            store_reads = tracing::field::Empty,
            tuples_read = tracing::field::Empty,
            memo_hits = tracing::field::Empty,
            max_depth_reached = tracing::field::Empty,
            cycle_passes = tracing::field::Empty,
            elapsed_us = tracing::field::Empty,
        );
        let _entered = span.enter();
        let start = Instant::now();
        let schema = self.current_schema()?;
        let (root, subject) = eval::resolve(&schema, &req.resource, &req.permission, &req.subject)?;
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        span.record("revision", revision.0);
        let mut memo = Memo::new();
        let mut evaluator = Evaluator::new(
            &schema,
            &*snapshot,
            revision,
            self.limits(),
            &subject,
            &mut memo,
        );
        let result = evaluator.check(&root);
        let mut stats = evaluator.stats();
        stats.elapsed = start.elapsed();
        record_stats(&span, &stats);
        let allowed = result?;
        span.record("allowed", allowed);
        Ok(CheckResponse {
            allowed,
            revision,
            stats,
        })
    }

    /// Runs every item as a [`check`](Self::check) at one revision resolved from `c`,
    /// returning one result per item in order. Items with the same subject share a memo.
    ///
    /// An item that is invalid or hits an evaluation limit fails on its own; the others
    /// still run.
    ///
    /// # Errors
    ///
    /// The whole call fails with [`Error::NoSchema`]; [`Error::RevisionNotYetAvailable`] /
    /// [`Error::RevisionExpired`] if the consistency cannot be met (or the revision is
    /// garbage-collected mid-call); [`Error::Store`] if the store fails on any item.
    /// Per item: as [`check`](Self::check).
    pub fn check_bulk(
        &self,
        items: Vec<CheckItem>,
        c: Consistency,
    ) -> Result<Vec<Result<bool, Error>>, Error> {
        let span = tracing::info_span!(
            "check_bulk",
            items = items.len(),
            revision = tracing::field::Empty,
            store_reads = tracing::field::Empty,
            tuples_read = tracing::field::Empty,
            memo_hits = tracing::field::Empty,
            max_depth_reached = tracing::field::Empty,
            cycle_passes = tracing::field::Empty,
            elapsed_us = tracing::field::Empty,
        );
        let _entered = span.enter();
        let start = Instant::now();
        let schema = self.current_schema()?;
        // One snapshot for the whole call: every item reads the same state.
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, c)?;
        span.record("revision", revision.0);
        let limits = self.limits();
        // Memoised values are settled, so they hold for any check with the same subject
        // at the same revision.
        let mut memos: HashMap<SubjectRef, Memo> = HashMap::new();
        let mut stats = EvalStats::default();
        let mut results = Vec::with_capacity(items.len());
        let mut failure = None;
        for item in items {
            let result =
                match eval::resolve(&schema, &item.resource, &item.permission, &item.subject) {
                    Ok((root, subject)) => {
                        let memo = memos.entry(item.subject).or_default();
                        let mut evaluator =
                            Evaluator::new(&schema, &*snapshot, revision, limits, &subject, memo);
                        let result = evaluator.check(&root);
                        stats.absorb(&evaluator.stats());
                        result
                    }
                    Err(e) => Err(e),
                };
            match result {
                Err(e @ (Error::Store(_) | Error::RevisionExpired { .. })) => {
                    failure = Some(e);
                    break;
                }
                r => results.push(r),
            }
        }
        stats.elapsed = start.elapsed();
        record_stats(&span, &stats);
        match failure {
            Some(e) => Err(e),
            None => Ok(results),
        }
    }

    fn limits(&self) -> Limits {
        Limits {
            max_depth: self.config.max_depth,
            max_fanout: self.config.max_check_fanout,
            max_reads: self.config.max_check_reads,
        }
    }

    fn current_schema(&self) -> Result<Arc<CompiledSchema>, Error> {
        self.schema_read().clone().ok_or(Error::NoSchema)
    }

    // The lock only guards an `Option<Arc<_>>` that is replaced in one assignment, so a
    // panic elsewhere while it was held cannot leave it half-updated; recovering from
    // poisoning is safe.
    fn schema_read(&self) -> RwLockReadGuard<'_, Option<Arc<CompiledSchema>>> {
        self.schema.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn schema_write(&self) -> RwLockWriteGuard<'_, Option<Arc<CompiledSchema>>> {
        self.schema.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Resolves `c` to the single revision a request reads at, checked once against the
/// snapshot's head and GC horizon (`docs/02-data-model.md` §4, `docs/05-storage.md` §2).
/// Every read of the request must go through the same snapshot, so a GC committed by
/// another process after this check cannot hide rows from those reads.
fn resolve(snapshot: &dyn ReadStore, c: Consistency) -> Result<Revision, Error> {
    let head = snapshot.head_revision()?;
    let requested = match c {
        Consistency::Latest => return Ok(head),
        Consistency::AtLeast(r) | Consistency::Exact(r) => r,
    };
    if requested > head {
        return Err(Error::RevisionNotYetAvailable { requested, head });
    }
    if matches!(c, Consistency::AtLeast(_)) {
        return Ok(head);
    }
    let horizon = snapshot.gc_horizon()?;
    if requested < horizon {
        return Err(Error::RevisionExpired { requested, horizon });
    }
    Ok(requested)
}

fn record_stats(span: &tracing::Span, stats: &EvalStats) {
    span.record("store_reads", stats.store_reads);
    span.record("tuples_read", stats.tuples_read);
    span.record("memo_hits", stats.memo_hits);
    span.record("max_depth_reached", stats.max_depth_reached);
    span.record("cycle_passes", stats.cycle_passes);
    span.record(
        "elapsed_us",
        u64::try_from(stats.elapsed.as_micros()).unwrap_or(u64::MAX),
    );
}

/// A GC between resolving the revision and reading can move the horizon past it; report
/// that as the expiry it is rather than as a store failure.
pub(crate) fn read_error(e: StoreError) -> Error {
    match e {
        StoreError::RevisionOutOfRange {
            requested, horizon, ..
        } if requested < horizon => Error::RevisionExpired { requested, horizon },
        e => Error::Store(e),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a `String` cannot fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}
