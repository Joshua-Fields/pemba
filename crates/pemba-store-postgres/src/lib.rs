//! Postgres relationship store for pemba.
//!
//! Implements `docs/10-postgres.md` §2.1 (a synchronous client and a hand-rolled pool), §3
//! (tables, migrations, schema qualification), §1.6.2 (reads: [`ReadStore`] with a
//! `REPEATABLE READ` snapshot) and §1.6.1 (native writes: [`WritableStore`], each call
//! its own READ COMMITTED transaction, the revision assigned by a trigger at commit).
//! A write can also join the app's own transaction: `Engine::write_in` with a [`PgTxn`],
//! then `Engine::resolve` after the commit (§1.6.4).
//!
//! [`ReadStore`]: pemba_core::store::ReadStore
//! [`WritableStore`]: pemba_core::store::WritableStore
//!
//! # Retrying
//!
//! A write that fails with SQLSTATE `40001` (serialization failure) or `40P01`
//! (deadlock) returns the retryable [`StoreError::Conflict`] (code
//! `TRANSACTION_CONFLICT`): roll back and run the whole transaction again, with
//! exponential backoff and jitter (`docs/06-api.md` §1). Every other database error is
//! [`StoreError::Backend`] and is not retryable.
//!
//! # Schemas and `search_path`
//!
//! Every store lives in one Postgres schema ([`PostgresConfig::schema`]), which must exist
//! before migrating. Every statement names its tables with that schema, quoted, and the
//! store's own connections set `search_path = pg_catalog, pg_temp`, so neither the
//! database's nor the role's nor the DSN's `search_path` can redirect pemba's SQL. Several
//! stores can share a database, one schema each.
//!
//! # Async callers
//!
//! **From async code, call through `spawn_blocking`.** The store is blocking: the
//! `postgres` crate drives its own internal runtime, and it panics if called on a thread
//! that is running async code (inside a tokio task or a `block_on` body). That is the
//! same rule as `postgres` and `reqwest::blocking`. Inside a tokio task the panic fails
//! that one request, not the process, but it is a bug in the caller. Run every call on
//! the store, and on an engine over it, in `tokio::task::spawn_blocking`:
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use pemba_store_postgres::{PostgresConfig, PostgresStore};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let store = tokio::task::spawn_blocking(|| {
//!     PostgresStore::open("postgres://app@db/app", PostgresConfig::default())
//! })
//! .await??;
//! let store = Arc::new(store);
//!
//! // Each later call: clone the Arc into the closure.
//! let s = Arc::clone(&store);
//! tokio::task::spawn_blocking(move || s.check_storage_version()).await??;
//! # Ok(())
//! # }
//! ```
//!
//! An async API that joins the app's async transactions is planned (T5.13); Python is not
//! affected, since its calls never run on a tokio thread.

#![forbid(unsafe_code)]

mod dialect;
mod migrate;
mod pool;
mod read;
mod sql;
#[cfg(test)]
mod tests;
mod tls;
mod txn;
mod write;

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use pemba_core::Error;
use pemba_core::model::Revision;
use pemba_core::store::{ReadStore, StoreError};
use postgres::error::SqlState;

pub use dialect::{PostgresDialect, check_schema_name};
pub use migrate::{MIN_SERVER_VERSION_NUM, STORAGE_VERSION};
/// The `postgres` crate this store is built against.
pub use postgres;
pub use txn::PgTxn;

/// The first key of every two-key advisory lock pemba takes (`docs/10-postgres.md` §3.3):
/// `0x70656D62`, "pemb". Applications using two-key advisory locks should avoid it.
pub const PEMBA_LOCK_CLASS: i32 = 0x7065_6D62;

/// How a Postgres store connects and what it checks. `Default` gives the spec defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PostgresConfig {
    /// The schema holding the store's tables, unquoted (e.g. `app`, or `My Schema`).
    /// `None` (the default) takes the first connection's `current_schema()` and keeps it.
    pub schema: Option<String>,
    /// Most connections the store opens; at least 1. Default 8. Connections are opened as
    /// needed, not all at once.
    pub pool_size: usize,
    /// Whether opening the store migrates its tables to [`STORAGE_VERSION`]. Default
    /// `false`, unlike SQLite's `true` (OQ-047): a Postgres database is shared, migrating
    /// needs `CREATE` on the schema, and app instances starting together would all run
    /// DDL. Run [`PostgresStore::migrate`] as a deploy step instead.
    pub auto_migrate: bool,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            schema: None,
            pool_size: 8,
            auto_migrate: false,
        }
    }
}

impl PostgresConfig {
    /// Sets [`schema`](Self::schema).
    #[must_use]
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    /// Sets [`pool_size`](Self::pool_size).
    #[must_use]
    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        self.pool_size = pool_size;
        self
    }

    /// Sets [`auto_migrate`](Self::auto_migrate).
    #[must_use]
    pub fn with_auto_migrate(mut self, auto_migrate: bool) -> Self {
        self.auto_migrate = auto_migrate;
        self
    }
}

/// A relationship store in a Postgres schema.
///
/// Blocking: from async code, call it through `tokio::task::spawn_blocking` (see the
/// [crate docs](crate#async-callers)); a direct call on a runtime thread panics.
pub struct PostgresStore {
    pool: pool::Pool,
    dialect: PostgresDialect,
    cache: Cache,
    /// Whether the early-firing warning (`docs/10-postgres.md` §1.6.7) was given.
    early_warned: AtomicBool,
    /// The cluster's `system_identifier` and the database name, which a caller's
    /// transaction must match (`docs/10-postgres.md` §2.3).
    identity: Identity,
}

/// Which database a connection is on: the cluster (`pg_control_system()`'s
/// `system_identifier`) and the database in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    system: i64,
    database: String,
}

/// Selects [`Identity`]; executable by `PUBLIC`, tested as an unprivileged role.
const IDENTITY_SQL: &str = "SELECT (pg_catalog.pg_control_system()).system_identifier, \
                            pg_catalog.current_database()::pg_catalog.text";

/// Head and GC horizon as last observed (`docs/05-storage.md` §2 "Revision checks"),
/// for range checks on reads made on the store rather than a snapshot. Refreshed by
/// `head_revision()` / `gc_horizon()` and by each snapshot; both only move up.
#[derive(Debug, Default)]
struct Cache {
    head: AtomicU64,
    horizon: AtomicU64,
}

impl Cache {
    fn observe(&self, head: Revision, horizon: Revision) {
        self.head.fetch_max(head.0, Ordering::AcqRel);
        self.horizon.fetch_max(horizon.0, Ordering::AcqRel);
    }

    /// After this store's own commit at `head`.
    fn observe_head(&self, head: Revision) {
        self.head.fetch_max(head.0, Ordering::AcqRel);
    }

    fn get(&self) -> (Revision, Revision) {
        (
            Revision(self.head.load(Ordering::Acquire)),
            Revision(self.horizon.load(Ordering::Acquire)),
        )
    }
}

impl fmt::Debug for PostgresStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Not the connection config: the DSN may carry a password.
        f.debug_struct("PostgresStore")
            .field("schema", &self.dialect.schema())
            .finish_non_exhaustive()
    }
}

impl PostgresStore {
    /// Connects to the database `dsn` names (a `postgres://` URL or `key=value` pairs, as
    /// libpq accepts) and opens the store in [`PostgresConfig::schema`].
    ///
    /// With [`PostgresConfig::auto_migrate`] the tables are created or upgraded; without
    /// it they must already be at [`STORAGE_VERSION`].
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidRequest`] if `dsn` does not parse, `pool_size` is 0, the schema
    ///   name is unusable ([`check_schema_name`]) or the schema does not exist.
    /// - [`Error::NotMigrated`] if `auto_migrate` is off and the tables are missing or older.
    /// - [`Error::StorageVersionTooNew`] if a newer library migrated them.
    /// - [`Error::Store`] if connecting or a query fails, the server is older than
    ///   Postgres 15, the database encoding is not `UTF8`, or the stored version is
    ///   unreadable.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "by value like `SqliteStore::open`, so callers can pass `PostgresConfig::default()`"
    )]
    pub fn open(dsn: &str, config: PostgresConfig) -> Result<Self, Error> {
        let first = First::connect(dsn, &config)?;
        let First {
            pg_config,
            connector,
            mut client,
            dialect,
        } = first;
        if config.auto_migrate {
            migrate::migrate(&mut client, &dialect)?;
        } else {
            migrate::check(&mut client, &dialect)?;
        }
        let row = client
            .query_one(IDENTITY_SQL, &[])
            .map_err(|e| Error::Store(store_err(&e)))?;
        let identity = Identity {
            system: row.try_get(0).map_err(|e| Error::Store(store_err(&e)))?,
            database: row.try_get(1).map_err(|e| Error::Store(store_err(&e)))?,
        };
        let store = Self {
            pool: pool::Pool::new(pg_config, connector, config.pool_size, client),
            dialect,
            cache: Cache::default(),
            early_warned: AtomicBool::new(false),
            identity,
        };
        // Seed the cache so reads on the store work before anyone asks for head.
        store.head_revision()?;
        Ok(store)
    }

    /// Creates or upgrades the store's tables to [`STORAGE_VERSION`] in one transaction,
    /// and returns without keeping a connection. Tables already at that version are left
    /// untouched; concurrent calls (several app instances deploying at once) serialise.
    /// The role needs `CREATE` on the schema.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open), except never [`Error::NotMigrated`].
    pub fn migrate(dsn: &str, config: &PostgresConfig) -> Result<(), Error> {
        let mut first = First::connect(dsn, config)?;
        migrate::migrate(&mut first.client, &first.dialect)
    }

    /// The schema holding this store's tables, unquoted.
    pub fn schema(&self) -> &str {
        self.dialect.schema()
    }

    /// The SQL dialect of this store's statements.
    pub fn dialect(&self) -> &PostgresDialect {
        &self.dialect
    }

    /// Checks out a connection and checks that the store is still at [`STORAGE_VERSION`].
    ///
    /// # Errors
    ///
    /// As the checks of [`open`](Self::open).
    pub fn check_storage_version(&self) -> Result<(), Error> {
        let mut lease = self.pool.lease()?;
        migrate::check(lease.client()?, &self.dialect)
    }
}

/// The first connection of a store, checked and set up, and what the pool needs to open
/// more.
struct First {
    pg_config: postgres::Config,
    connector: tls::Connector,
    client: postgres::Client,
    dialect: PostgresDialect,
}

impl First {
    fn connect(dsn: &str, config: &PostgresConfig) -> Result<Self, Error> {
        if config.pool_size == 0 {
            return Err(Error::InvalidRequest {
                message: "PostgresConfig::pool_size must be at least 1".to_owned(),
            });
        }
        if let Some(s) = &config.schema {
            check_schema_name(s)?;
        }
        // The parser's messages name the bad option, never echo the DSN (or its password).
        let pg_config = postgres::Config::from_str(dsn).map_err(|e| Error::InvalidRequest {
            message: format!("invalid Postgres connection string: {e}"),
        })?;
        let connector = tls::Connector::new()?;
        let mut client = pool::connect_raw(&pg_config, &connector)?;
        migrate::check_server(&mut client)?;
        let schema = if let Some(s) = &config.schema {
            s.clone()
        } else {
            let current: Option<String> = client
                .query_one("SELECT pg_catalog.current_schema()", &[])
                .and_then(|row| row.try_get(0))
                .map_err(|e| Error::Store(store_err(&e)))?;
            let s = current.ok_or_else(|| Error::InvalidRequest {
                message: "no schema configured and the connection's search_path names \
                          no existing schema; set PostgresConfig::schema"
                    .to_owned(),
            })?;
            tracing::info!(schema = %s, "pemba: no schema configured, using current_schema()");
            s
        };
        let dialect = PostgresDialect::new(&schema)?;
        pool::setup(&mut client)?;
        migrate::check_schema_exists(&mut client, &schema)?;
        Ok(Self {
            pg_config,
            connector,
            client,
            dialect,
        })
    }
}

/// Classifies a driver error: unique violations and serialisation failures get their own
/// variants because the write algorithm and callers act on them. SQLSTATE 40001 and 40P01
/// become the retryable [`StoreError::Conflict`], so callers can tell "retry with backoff"
/// from "fail" by [`StoreError::is_retryable`] or the code `TRANSACTION_CONFLICT`.
pub(crate) fn store_err(e: &postgres::Error) -> StoreError {
    let Some(db) = e.as_db_error() else {
        return StoreError::Backend {
            message: e.to_string(),
        };
    };
    // `postgres::Error`'s own Display is only "db error"; the server's message is the
    // useful part.
    let mut message = format!("{}: {} ({})", db.severity(), db.message(), db.code().code());
    if let Some(detail) = db.detail() {
        message.push_str(": ");
        message.push_str(detail);
    }
    match *db.code() {
        SqlState::UNIQUE_VIOLATION => StoreError::UniqueViolation { message },
        SqlState::T_R_SERIALIZATION_FAILURE | SqlState::T_R_DEADLOCK_DETECTED => {
            StoreError::Conflict { message }
        }
        _ => StoreError::Backend { message },
    }
}
