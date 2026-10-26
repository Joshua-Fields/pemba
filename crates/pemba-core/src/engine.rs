//! The `Engine`: schema management, writes and reads over a store (`docs/06-api.md` §1).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use pemba_schema::ir::CompiledSchema;
use pemba_schema::{ErrorCode, SchemaError};

use crate::error::{Error, OrphanReport};
use crate::eval::{
    self, CheckItem, CheckRequest, CheckResponse, EvalStats, Evaluator, Limits, Memo,
};
use crate::lookup::{self, Candidates, LookupResourcesRequest, LookupResourcesResponse};
use crate::model::{
    Consistency, ObjectId, ObjectRef, Precondition, Relationship, RelationshipFilter, Revision,
    SubjectRef, TypeName, WriteRequest,
};
use crate::store::{
    DeleteResult, GcStats, ReadStore, RelationCount, SqlStore, StoreError, StoredSchema,
    WritableStore, WriteTxn,
};
use crate::subjects::{self, Gatherer, LookupSubjectsRequest, LookupSubjectsResponse, SubjectSet};
use crate::trace::{CheckTrace, Recorder, Tracer};
use crate::validate::{subject_kind, validate_filter, validate_ops, validate_relationship};

/// The span of one `check` (`docs/04-evaluation.md` §1): request fields now, the rest once
/// evaluated. A macro because `info_span!` needs a literal name.
macro_rules! check_span {
    ($name:literal, $req:expr) => {
        tracing::info_span!(
            $name,
            resource = %$req.resource,
            permission = %$req.permission,
            subject = %$req.subject,
            revision = tracing::field::Empty,
            allowed = tracing::field::Empty,
            store_reads = tracing::field::Empty,
            tuples_read = tracing::field::Empty,
            memo_hits = tracing::field::Empty,
            max_depth_reached = tracing::field::Empty,
            cycle_passes = tracing::field::Empty,
            elapsed_us = tracing::field::Empty,
        )
    };
}

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
    /// Maximum nodes [`Engine::check_with_trace`] records; past it the trace is marked
    /// truncated and the answer is unaffected (`docs/04-evaluation.md` §3.8).
    pub max_trace_nodes: usize,
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
            max_trace_nodes: 10_000,
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
        if c.max_write_batch == 0 || c.max_lookup_results == 0 || c.max_trace_nodes == 0 {
            return Err(Error::InvalidRequest {
                message: "max_write_batch, max_lookup_results and max_trace_nodes must be at \
                          least 1"
                    .to_owned(),
            });
        }
        let schema = match store.current_schema()? {
            Some(stored) => Some(Arc::new(Loaded::compile(&stored)?)),
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
    /// The newest schema this engine has seen, with its stored version. A cache only:
    /// reads use the version in their own snapshot (`docs/05-storage.md` §7), and writes
    /// commit only if the store is still at the version they were validated against.
    schema: RwLock<Option<Arc<Loaded>>>,
}

/// A stored schema, compiled.
#[derive(Debug)]
pub(crate) struct Loaded {
    version: u64,
    created_rev: Revision,
    compiled: Arc<CompiledSchema>,
}

impl Loaded {
    fn compile(stored: &StoredSchema) -> Result<Self, Error> {
        Ok(Self {
            version: stored.version,
            created_rev: stored.created_rev,
            compiled: Arc::new(
                pemba_schema::compile(&stored.source)
                    .map_err(Error::Schema)?
                    .schema,
            ),
        })
    }

    /// Whether a read at `rev` can meet tuples this schema rejects: only below the
    /// revision it was written at, since `write_schema` refuses to orphan live tuples and
    /// every later write is validated against it (`docs/05-storage.md` §7).
    fn stale_possible(&self, rev: Revision) -> bool {
        rev < self.created_rev
    }
}

/// How many times a write is validated and attempted when another process keeps changing
/// the schema underneath it.
const SCHEMA_RETRIES: usize = 3;

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
    /// A change that would orphan live relationships (leave a stored tuple that the new
    /// schema does not allow) is refused, inside the same store transaction as the write
    /// (`docs/05-storage.md` §7).
    ///
    /// # Errors
    ///
    /// [`Error::Schema`] with every compile error; [`Error::SchemaChangeOrphans`] listing
    /// each orphaned group of live relationships; [`Error::Store`] if the store fails.
    pub fn write_schema(&self, source: &str) -> Result<SchemaWriteResult, Error> {
        let _span = tracing::info_span!("write_schema", bytes = source.len()).entered();
        let compiled = pemba_schema::compile(source).map_err(Error::Schema)?;
        if let Some(stored) = self.store.current_schema()?
            && stored.hash == hex(&compiled.schema.hash)
        {
            return Ok(SchemaWriteResult {
                version: stored.version,
                revision: stored.created_rev,
                changed: false,
                warnings: compiled.warnings,
            });
        }
        let new = &compiled.schema;
        let check = |counts: &[RelationCount]| {
            let orphans = orphans(new, counts);
            if orphans.is_empty() {
                Ok(())
            } else {
                Err(Error::SchemaChangeOrphans(orphans))
            }
        };
        let stored = self
            .store
            .write_schema(source, &hex(&compiled.schema.hash), &check)?;
        self.offer(Arc::new(Loaded {
            version: stored.version,
            created_rev: stored.created_rev,
            compiled: Arc::new(compiled.schema),
        }));
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
        let rev = self.validated(&req, |req, version| self.store.write(req, Some(version)))?;
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
        self.store.check_same_database(txn)?;
        let rev = self.validated(&req, |req, version| {
            crate::write::write(txn, self.store.dialect(), req, Some(version))
        })?;
        tracing::debug!(
            revision = rev.0,
            "batch written, pending the caller's commit"
        );
        Ok(rev)
    }

    /// Checks batch size, schema presence, ops and precondition filters against the newest
    /// schema known, then runs `apply` with that schema's version, which commits only if the
    /// store is still at it. If another process changed the schema meanwhile, reloads it and
    /// validates again, up to [`SCHEMA_RETRIES`] times.
    fn validated(
        &self,
        req: &WriteRequest,
        mut apply: impl FnMut(&WriteRequest, u64) -> Result<Revision, Error>,
    ) -> Result<Revision, Error> {
        if req.ops.len() > self.config.max_write_batch {
            return Err(Error::BatchTooLarge {
                max: self.config.max_write_batch,
                got: req.ops.len(),
            });
        }
        let mut loaded = self.newest_schema(false)?;
        let mut attempt = 1;
        loop {
            if let Err(e) = validate(&loaded.compiled, req) {
                // The cached schema may be older than the store's (another process changed
                // it): judge the batch by the store's current schema, not a stale copy.
                if self.store.schema_version()? == loaded.version {
                    return Err(e);
                }
                loaded = self.newest_schema(true)?;
                validate(&loaded.compiled, req)?;
            }
            match apply(req, loaded.version) {
                Err(Error::Store(StoreError::SchemaChanged { .. })) if attempt < SCHEMA_RETRIES => {
                    attempt += 1;
                    loaded = self.newest_schema(true)?;
                }
                r => return r,
            }
        }
    }

    /// Marks deleted every live relationship matching `filter`, at most `limit` of them
    /// (`None`: [`EngineConfig::max_delete_by_filter`], also the maximum), in storage
    /// order, at one new revision, atomically. Call again while
    /// [`DeleteResult::more`] is `true` to delete the rest.
    ///
    /// # Errors
    ///
    /// [`Error::NoSchema`]; [`Error::InvalidRequest`] for a limit of 0 or above the
    /// maximum, or a filter naming something the schema does not define; [`Error::Store`]
    /// if the store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn delete_relationships(
        &self,
        filter: RelationshipFilter,
        limit: Option<usize>,
    ) -> Result<DeleteResult, Error> {
        let _span = tracing::info_span!("delete_relationships", filter = %filter).entered();
        let max = self.config.max_delete_by_filter;
        let limit = limit.unwrap_or(max);
        if limit == 0 || limit > max {
            return Err(Error::InvalidRequest {
                message: format!("limit must be between 1 and {max}, got {limit}"),
            });
        }
        let loaded = self.newest_schema(false)?;
        if let Err(e) = validate_filter(&loaded.compiled, &filter) {
            if self.store.schema_version()? == loaded.version {
                return Err(e);
            }
            validate_filter(&self.newest_schema(true)?.compiled, &filter)?;
        }
        let result = self.store.delete_matching(&filter, limit)?;
        tracing::debug!(
            deleted = result.deleted,
            more = result.more,
            revision = result.revision.0,
            "relationships deleted"
        );
        Ok(result)
    }

    /// Garbage-collects history below `keep_from` (`docs/05-storage.md` §8): raises the GC
    /// horizon first, in its own transaction, then removes tuples deleted at or before it
    /// in batches. From the moment the horizon moves, reads at `Exact(r)` with
    /// `r < keep_from` fail with [`Error::RevisionExpired`]; a request that started before
    /// still answers from its snapshot.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] if `keep_from` is above head; [`Error::Store`] if the
    /// store fails (the horizon may already be raised; a later GC removes the rest).
    pub fn gc(&self, keep_from: Revision) -> Result<GcStats, Error> {
        let _span = tracing::info_span!("gc", keep_from = keep_from.0).entered();
        let stats = self.store.gc(keep_from)?;
        tracing::debug!(
            rows_removed = stats.rows_removed,
            batches = stats.batches,
            horizon = stats.horizon.0,
            "gc done"
        );
        Ok(stats)
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
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        let loaded = self.schema_at(&*snapshot)?;
        validate_filter(&loaded.compiled, &req.filter)?;
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
    /// [`Error::FanoutExceeded`] / [`Error::ReadLimitExceeded`] if evaluation hits a limit
    /// or an undefined cycle;
    /// [`Error::StaleRelationship`] if it meets a stored subject set the schema no longer
    /// allows; [`Error::Store`] if the store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn check(&self, req: CheckRequest) -> Result<CheckResponse, Error> {
        // The hot path keeps its own body rather than sharing `run_check` with
        // `check_with_trace`: the generic version measured 1-2% slower here (T4.1).
        let span = check_span!("check", req);
        let _entered = span.enter();
        let start = Instant::now();
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        span.record("revision", revision.0);
        let loaded = self.schema_at(&*snapshot)?;
        let schema = &loaded.compiled;
        let (root, subject) = eval::resolve(schema, &req.resource, &req.permission, &req.subject)?;
        let mut memo = Memo::new();
        let mut evaluator = Evaluator::new(
            schema,
            &*snapshot,
            revision,
            self.limits(loaded.stale_possible(revision)),
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

    /// [`check`](Self::check), also returning the tree of steps the evaluation took
    /// (`docs/04-evaluation.md` §3.8): which members it evaluated, which stored
    /// relationships matched, what it reused, skipped or iterated.
    ///
    /// Same evaluator, same answer, same errors: on an error there is no trace. At most
    /// [`EngineConfig::max_trace_nodes`] nodes are recorded; past that the trace is marked
    /// [`truncated`](CheckTrace::truncated) and the answer is still complete.
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check).
    pub fn check_with_trace(
        &self,
        req: CheckRequest,
    ) -> Result<(CheckResponse, CheckTrace), Error> {
        self.check_with_trace_limit(req, self.config.max_trace_nodes)
    }

    /// [`check_with_trace`](Self::check_with_trace) recording at most `max_trace_nodes`
    /// nodes instead of [`EngineConfig::max_trace_nodes`], for one call.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] if `max_trace_nodes` is 0; otherwise as
    /// [`check`](Self::check).
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn check_with_trace_limit(
        &self,
        req: CheckRequest,
        max_trace_nodes: usize,
    ) -> Result<(CheckResponse, CheckTrace), Error> {
        if max_trace_nodes == 0 {
            return Err(Error::InvalidRequest {
                message: "max_trace_nodes must be at least 1".to_owned(),
            });
        }
        let span = check_span!("check_with_trace", req);
        let _entered = span.enter();
        let max = max_trace_nodes;
        let (response, recorder) =
            self.run_check(&span, &req, |schema| Recorder::new(Arc::clone(schema), max))?;
        let (root, truncated) = recorder.into_tree().ok_or_else(|| Error::InvalidRequest {
            message: "internal: check_with_trace recorded no root".to_owned(),
        })?;
        if root.result != Some(response.allowed) {
            return Err(Error::InvalidRequest {
                message: "internal: the trace's root disagrees with the answer".to_owned(),
            });
        }
        let trace = CheckTrace {
            resource: req.resource,
            permission: req.permission,
            subject: req.subject,
            allowed: response.allowed,
            revision: response.revision,
            stats: response.stats,
            truncated,
            root,
        };
        Ok((response, trace))
    }

    /// One `check` reporting to the trace `make` builds for the request's schema: the body
    /// of [`check`](Self::check), for `check_with_trace`.
    fn run_check<T: Tracer>(
        &self,
        span: &tracing::Span,
        req: &CheckRequest,
        make: impl FnOnce(&Arc<CompiledSchema>) -> T,
    ) -> Result<(CheckResponse, T), Error> {
        let start = Instant::now();
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        span.record("revision", revision.0);
        let loaded = self.schema_at(&*snapshot)?;
        let schema = &loaded.compiled;
        let (root, subject) = eval::resolve(schema, &req.resource, &req.permission, &req.subject)?;
        let mut memo = Memo::new();
        let mut evaluator = Evaluator::with_trace(
            schema,
            &*snapshot,
            revision,
            self.limits(loaded.stale_possible(revision)),
            &subject,
            &mut memo,
            make(schema),
        );
        let result = evaluator.check(&root);
        let mut stats = evaluator.stats();
        stats.elapsed = start.elapsed();
        record_stats(span, &stats);
        let allowed = result?;
        span.record("allowed", allowed);
        Ok((
            CheckResponse {
                allowed,
                revision,
                stats,
            },
            evaluator.into_trace(),
        ))
    }

    /// One page of the objects of `req.resource_type` on which `req.subject` has
    /// `req.permission` (`docs/04-evaluation.md` §5), ascending by id.
    ///
    /// Every page is its own request on its own read snapshot. To page through one
    /// consistent answer, pass `Consistency::Exact(first_response.revision)` with the
    /// cursor for every later page; otherwise a later page may read a newer revision than
    /// the one its cursor came from.
    ///
    /// # Errors
    ///
    /// [`Error::NoSchema`]; [`Error::InvalidRequest`] for a limit of 0 or above
    /// [`EngineConfig::max_lookup_results`], an undefined resource type, permission,
    /// subject type or subject relation, or a wildcard subject;
    /// [`Error::RevisionNotYetAvailable`] / [`Error::RevisionExpired`] if the consistency
    /// cannot be met; [`Error::LookupTooLarge`] past [`EngineConfig::max_lookup_nodes`]
    /// candidate nodes; any error [`check`](Self::check) can return for a candidate;
    /// [`Error::Store`] if the store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn lookup_resources(
        &self,
        req: LookupResourcesRequest,
    ) -> Result<LookupResourcesResponse, Error> {
        let span = tracing::info_span!(
            "lookup_resources",
            resource_type = %req.resource_type,
            permission = %req.permission,
            subject = %req.subject,
            revision = tracing::field::Empty,
            nodes_visited = tracing::field::Empty,
            candidates = tracing::field::Empty,
            results = tracing::field::Empty,
            phase1_reads = tracing::field::Empty,
            store_reads = tracing::field::Empty,
            tuples_read = tracing::field::Empty,
            memo_hits = tracing::field::Empty,
            max_depth_reached = tracing::field::Empty,
            cycle_passes = tracing::field::Empty,
            elapsed_us = tracing::field::Empty,
        );
        let _entered = span.enter();
        let start = Instant::now();
        let max = self.config.max_lookup_results;
        let limit = req.limit.unwrap_or(max);
        if limit == 0 || limit > max {
            return Err(Error::InvalidRequest {
                message: format!("limit must be between 1 and {max}, got {limit}"),
            });
        }
        // One snapshot for both phases; dropped on every return below.
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        span.record("revision", revision.0);
        let loaded = self.schema_at(&*snapshot)?;
        let schema = &loaded.compiled;
        let target = lookup::resolve_target(schema, &req.resource_type, &req.permission)?;
        let subject = eval::resolve_subject(schema, &req.subject)?;

        let (candidates, nodes_visited, phase1_reads) = Candidates::new(
            schema,
            &*snapshot,
            revision,
            self.config.max_lookup_nodes,
            target,
        )
        .run(&subject)?;
        span.record("nodes_visited", nodes_visited);
        span.record("candidates", candidates.len());
        span.record("phase1_reads", phase1_reads);

        let (result, mut stats) = lookup::filter(
            schema,
            &*snapshot,
            revision,
            self.limits(loaded.stale_possible(revision)),
            &subject,
            target,
            &candidates,
            req.cursor.as_ref(),
            limit as usize,
        );
        stats.elapsed = start.elapsed();
        record_stats(&span, &stats);
        let (resource_ids, more) = result?;
        span.record("results", resource_ids.len());
        let next_cursor = if more {
            resource_ids.last().cloned()
        } else {
            None
        };
        Ok(LookupResourcesResponse {
            resource_ids,
            next_cursor,
            revision,
        })
    }

    /// Every object of `req.subject_type` that has `req.permission` on `req.resource`
    /// (`docs/04-evaluation.md` §6): a list of ids, or, when a wildcard grants it, every
    /// object except a list of ids.
    ///
    /// Object subjects only; never `false` (or `true`) in place of an error. The whole
    /// request is one evaluation, bounded like one [`check`](Self::check): store reads
    /// count against [`EngineConfig::max_check_reads`].
    ///
    /// # Errors
    ///
    /// [`Error::NoSchema`]; [`Error::InvalidRequest`] if the resource type, permission or
    /// subject type is not defined; [`Error::RevisionNotYetAvailable`] /
    /// [`Error::RevisionExpired`] if the consistency cannot be met;
    /// [`Error::MaxDepthExceeded`], [`Error::NonMonotonicCycle`],
    /// [`Error::FanoutExceeded`] or [`Error::ReadLimitExceeded`] if evaluation hits a
    /// limit or an undefined cycle; [`Error::StaleRelationship`] if it meets a stored
    /// subject set the schema no longer allows; [`Error::Store`] if the store fails.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "signature fixed by docs/06-api.md §1"
    )]
    pub fn lookup_subjects(
        &self,
        req: LookupSubjectsRequest,
    ) -> Result<LookupSubjectsResponse, Error> {
        let span = tracing::info_span!(
            "lookup_subjects",
            resource = %req.resource,
            permission = %req.permission,
            subject_type = %req.subject_type,
            revision = tracing::field::Empty,
            results = tracing::field::Empty,
            wildcard = tracing::field::Empty,
            store_reads = tracing::field::Empty,
            tuples_read = tracing::field::Empty,
            memo_hits = tracing::field::Empty,
            max_depth_reached = tracing::field::Empty,
            cycle_passes = tracing::field::Empty,
            elapsed_us = tracing::field::Empty,
        );
        let _entered = span.enter();
        let start = Instant::now();
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, req.consistency)?;
        span.record("revision", revision.0);
        let loaded = self.schema_at(&*snapshot)?;
        let schema = &loaded.compiled;
        let (root, subject_type) = subjects::resolve(schema, &req)?;
        let mut gatherer = Gatherer::new(
            schema,
            &*snapshot,
            revision,
            self.limits(loaded.stale_possible(revision)),
            (subject_type, &req.subject_type),
        );
        let result = gatherer.gather(&root);
        let mut stats = gatherer.stats();
        stats.elapsed = start.elapsed();
        record_stats(&span, &stats);
        let (subject_ids, wildcard, excluded_ids) = match result? {
            SubjectSet::Ids(ids) => (ids.into_iter().collect::<Vec<_>>(), false, Vec::new()),
            SubjectSet::AllExcept(x) => (Vec::new(), true, x.into_iter().collect()),
        };
        span.record("results", subject_ids.len());
        span.record("wildcard", wildcard);
        Ok(LookupSubjectsResponse {
            subject_ids,
            wildcard,
            excluded_ids,
            revision,
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
        // One snapshot for the whole call: every item reads the same state.
        let snapshot = self.store.snapshot()?;
        let revision = resolve(&*snapshot, c)?;
        span.record("revision", revision.0);
        let loaded = self.schema_at(&*snapshot)?;
        let schema = &*loaded.compiled;
        let limits = self.limits(loaded.stale_possible(revision));
        // Memoised values are settled, so they hold for any check with the same subject
        // at the same revision.
        let mut memos: HashMap<SubjectRef, Memo> = HashMap::new();
        let mut stats = EvalStats::default();
        let mut results = Vec::with_capacity(items.len());
        let mut failure = None;
        for item in items {
            let result =
                match eval::resolve(schema, &item.resource, &item.permission, &item.subject) {
                    Ok((root, subject)) => {
                        let memo = memos.entry(item.subject).or_default();
                        let mut evaluator =
                            Evaluator::new(schema, &*snapshot, revision, limits, &subject, memo);
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

    fn limits(&self, stale_possible: bool) -> Limits {
        Limits {
            max_depth: self.config.max_depth,
            max_fanout: self.config.max_check_fanout,
            max_reads: self.config.max_check_reads,
            stale_possible,
        }
    }

    /// The schema a request reading through `snapshot` evaluates with: the version stored
    /// as of that snapshot, so schema and data come from the same moment even when another
    /// process changed the schema (`docs/05-storage.md` §7). Reuses the cached compile when
    /// the versions match; otherwise compiles the snapshot's schema and caches it if newer.
    fn schema_at(&self, snapshot: &dyn ReadStore) -> Result<Arc<Loaded>, Error> {
        let version = snapshot.schema_version()?;
        if version == 0 {
            return Err(Error::NoSchema);
        }
        if let Some(cached) = self.schema_read().clone()
            && cached.version == version
        {
            return Ok(cached);
        }
        let stored = snapshot.current_schema()?.ok_or(Error::NoSchema)?;
        let loaded = Arc::new(Loaded::compile(&stored)?);
        self.offer(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// The newest schema this engine knows, for validating a write; with `reload`, or if it
    /// knows none, read from the store first.
    fn newest_schema(&self, reload: bool) -> Result<Arc<Loaded>, Error> {
        if !reload && let Some(cached) = self.schema_read().clone() {
            return Ok(cached);
        }
        let stored = self.store.current_schema()?.ok_or(Error::NoSchema)?;
        let loaded = Arc::new(Loaded::compile(&stored)?);
        self.offer(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Caches `loaded` unless the cache already holds a newer version.
    fn offer(&self, loaded: Arc<Loaded>) {
        let mut current = self.schema_write();
        if current.as_ref().is_none_or(|c| c.version < loaded.version) {
            *current = Some(loaded);
        }
    }

    // The lock only guards an `Option<Arc<_>>` that is replaced in one assignment, so a
    // panic elsewhere while it was held cannot leave it half-updated; recovering from
    // poisoning is safe.
    fn schema_read(&self) -> RwLockReadGuard<'_, Option<Arc<Loaded>>> {
        self.schema.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn schema_write(&self) -> RwLockWriteGuard<'_, Option<Arc<Loaded>>> {
        self.schema.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Step 1 of a write: every op and precondition filter against `schema`.
fn validate(schema: &CompiledSchema, req: &WriteRequest) -> Result<(), Error> {
    validate_ops(schema, &req.ops)?;
    for p in &req.preconditions {
        let (Precondition::MustExist(f) | Precondition::MustNotExist(f)) = p;
        validate_filter(schema, f)?;
    }
    Ok(())
}

/// Live tuple groups at head that would fail validation against `schema`
/// (`docs/03-schema-language.md` §10, `docs/05-storage.md` §7): a group is orphaned
/// exactly when a tuple of its shape could not be written under the new schema, so after
/// the change no live tuple contradicts the schema. That covers a removed type (as
/// resource or subject), a removed relation, a relation that became a permission, a
/// removed allowed type, subject set (including one whose relation was deleted from its
/// type) or wildcard.
fn orphans(schema: &CompiledSchema, counts: &[RelationCount]) -> Vec<OrphanReport> {
    counts
        .iter()
        .filter_map(|c| {
            let subject = match (&c.subject_relation, c.wildcard) {
                (_, true) => SubjectRef::Wildcard {
                    object_type: c.subject_type.clone(),
                },
                (Some(rel), false) => SubjectRef::Set {
                    object: placeholder(&c.subject_type),
                    relation: rel.clone(),
                },
                (None, false) => SubjectRef::Object(placeholder(&c.subject_type)),
            };
            let shape = Relationship {
                resource: placeholder(&c.resource_type),
                relation: c.relation.clone(),
                subject,
            };
            match validate_relationship(schema, &shape) {
                Ok(()) => None,
                Err(e) => {
                    let (code, message) = match e {
                        Error::InvalidRelationship { code, message, .. } => (code, message),
                        e => (ErrorCode::UnknownType, e.to_string()),
                    };
                    Some(OrphanReport {
                        resource_type: c.resource_type.clone(),
                        relation: c.relation.clone(),
                        subject: subject_kind(&shape.subject),
                        count: c.count,
                        code,
                        message,
                    })
                }
            }
        })
        .collect()
}

/// A stand-in object of type `t` for validating a tuple's shape; ids are never checked.
fn placeholder(t: &TypeName) -> ObjectRef {
    ObjectRef {
        object_type: t.clone(),
        object_id: ObjectId::placeholder(),
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
