//! The `Engine`: schema management, writes and reads over a store (`docs/06-api.md` §1).

use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use warden_schema::SchemaError;
use warden_schema::ir::CompiledSchema;

use crate::error::Error;
use crate::model::{
    Consistency, Precondition, Relationship, RelationshipFilter, Revision, WriteRequest,
};
use crate::store::{StoreError, WritableStore};
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
    /// Run storage migrations when the engine is built.
    pub auto_migrate: bool,
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
            auto_migrate: true,
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
        let rev = self.store.write(&req)?;
        drop(guard);
        tracing::debug!(revision = rev.0, "batch committed");
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
        let revision = self.resolve(req.consistency)?;
        // One extra row tells whether another page exists without a second query.
        let mut relationships = self
            .store
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

    /// Resolves `c` to the single revision a request reads at, checked once against the
    /// store's head and GC horizon (`docs/02-data-model.md` §4, `docs/05-storage.md` §2).
    fn resolve(&self, c: Consistency) -> Result<Revision, Error> {
        let head = self.store.head_revision()?;
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
        let horizon = self.store.gc_horizon()?;
        if requested < horizon {
            return Err(Error::RevisionExpired { requested, horizon });
        }
        Ok(requested)
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

/// A GC between resolving the revision and reading can move the horizon past it; report
/// that as the expiry it is rather than as a store failure.
fn read_error(e: StoreError) -> Error {
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
