# Transactions and Postgres operations

The [README quick start](../../README.md#python-quick-start) already demonstrates a
SQLite application row and permission committing together. Foreign writes use
`engine.write(..., conn=conn)` in Python, `Engine::write_in` with `SqliteTxn` or `PgTxn`
in Rust. The application owns commit and rollback. Native writes run an independent
engine transaction and cannot atomically commit an unrelated app row.

Open and migrate the store before the app transaction starts. Use one database file
(SQLite), or the same Postgres cluster and database plus the engine's configured schema.
A mismatched connection raises `ForeignConnectionMismatch`, not denial. All checks,
lookups and pending resolution use engine-owned connections. There is **no promise of
reading uncommitted app writes** through the engine. Commit successfully, then resolve
and read with `AtLeast`; if commit fails, roll back and discard the pending token.
See [store-specific pending behavior](consistency.md).

The batch's SQL runs inside a savepoint so ordinary batch failures undo its statements.
A savepoint cannot guarantee recovery from every driver/database failure. Treat a failed
authorisation write as an application-transaction failure: roll back and discard its
pending token; handle commit errors too. No open transaction normally means
`ValueError`. `allow_autocommit=True` explicitly opts out of atomicity with app data
and is supported for SQLite/raw psycopg, not SQLAlchemy or Django.

Run the [transactional-write tutorial](transactional-tutorial.md) for executable
Rust/Python examples on both stores, including forced rollback and regression checks.

## SQLite

Use a file-backed database and a `sqlite3.Connection` with an explicit transaction
(`BEGIN IMMEDIATE` is suitable for writes). In Rust obtain `rusqlite` from
`pemba_store_sqlite::rusqlite` and wrap its transaction with `SqliteTxn::new(&tx)`.
This ensures the driver's SQLite matches the store's. SQLite auto-migrates by default;
production can disable auto-migration and run migrations as a deployment step.

In Python the engine shares **CPython's own SQLite**, minimum 3.26.0 with thread support.
`sqlite3`, SQLAlchemy's sqlite/pysqlite dialect, Django and aiosqlite use that library;
the `conn=` SQLite API itself accepts `sqlite3.Connection`, so pass the underlying
supported sync connection. Do not load apsw, pysqlite3-binary, sqlean.py, another bundled
SQLite, or a ctypes-loaded SQLite against the same file in the process. On POSIX,
closing one SQLite copy's descriptor can release the other copy's locks and lose a
committed write. This restriction applies on all platforms for portable applications.
Use `sqlite3.Connection.backup` for live backups; do not directly open/close the database,
WAL or SHM files. See [the API's detailed rule](../06-api.md#25-sqlite-in-the-same-process).
Python SQLite engine requests are serialised and hold the GIL; moving them to threads
does not make that engine execute requests in parallel.

## Postgres setup and migrations

Use Postgres >=15 with a UTF8 database. Create the target schema before opening the
store: a typo must fail rather than create another store. Tables are schema-qualified;
foreign `search_path` cannot redirect them. Multiple stores use separate schemas.
`PostgresConfig::default().with_schema("authz")` configures Rust;
Python takes `Engine.postgres(dsn, schema="authz")`. Keep credentials in application
configuration, not guide files, command history or logs.

`auto_migrate` defaults to **false**. Run `PostgresStore::migrate` in Rust, or open once
with `auto_migrate=True` in Python, as the schema owner during deployment. Normal app
startup uses `auto_migrate=False`. An unmigrated store raises `NotMigrated`; a future
storage version raises `StorageVersionTooNew`. Do not clear the version to bypass it.
Migrations are ordered and transactional, with an advisory lock serialising migrators.

The migration role needs CREATE on the schema and ownership of objects it alters.
The runtime role needs USAGE on the schema, SELECT/INSERT/UPDATE on pemba tables,
USAGE on both sequences and EXECUTE on the revision function (PUBLIC by default).
GC additionally needs DELETE on `pemba_relationships`, `pemba_pending` and
`pemba_revisions`. No superuser or extension is required. Hardened installations must
also permit the identity check through `pg_control_system()`. The trigger runs as the
committing role; grants must cover foreign writes too. Avoid renaming the schema:
qualified names are embedded in the trigger function. The design's `pemba migrate
--repair` is not shipped by the SQLite/fixture CLI; do not assume it exists. This spec/implementation gap is recorded as
[OQ-062](../OPEN_QUESTIONS.md).

## TLS and pooling

Rust Postgres defaults enable the `tls` feature. Python includes Postgres with rustls;
there is no OpenSSL dependency in the extension. Certificate validation uses platform
trust roots and the DSN's `sslmode`. Use `sslmode=require` when transport encryption is
required; an untrusted certificate fails the handshake. There is no exposed custom-CA
option. Install the appropriate CA in the platform trust store; `sslmode=disable` is
only appropriate for an explicitly trusted local/test transport, not a workaround for
production certificate validation. Foreign application drivers configure their own TLS.

The engine owns a connection pool (default 8) and caches prepared statements per
connection. PgBouncer session pooling works; transaction pooling requires PgBouncer
>=1.21 and configured `max_prepared_statements`. Do not assume statement pooling support.
Keep the database close to the app: deep graph walks cost network round trips; see
[measured context](../../benches/RESULTS.md), not a universal latency guarantee.

## Commit-time revisions, early constraints and 2PC

Postgres's deferred trigger allocates revisions at commit under a store-wide lock.
`SET CONSTRAINTS ALL IMMEDIATE` fires it early and holds that lock until the transaction
ends, blocking other pemba commits. Name only your own constraints instead of ALL;
setting DEFERRED again does not release the lock. Correctness remains intact and the
server warns; pending resolution emits `PembaWarning` / a tracing warning once per engine.
Django `check_constraints()` and `TestCase` can trigger this behavior.

Two-phase commit allocates the revision at `PREPARE TRANSACTION`; the prepared transaction
holds its locks until `COMMIT PREPARED` or `ROLLBACK PREPARED`. Keep that window short:
other pemba commits wait. App deferred constraints also run while pemba's lock is held
and can create deadlocks with concurrent commits.

## Isolation and retries

Native writes use READ COMMITTED. App SERIALIZABLE and REPEATABLE READ transactions
may fail at any statement or at COMMIT. Retry the **whole application transaction**
after rollback, not just the pemba call. For SQLSTATE `40001` (serialization failure)
and `40P01` (deadlock), use bounded exponential backoff with jitter, for example random
0..min(1 second, 10 milliseconds * 2^attempt), up to 10 attempts, then surface failure.
The pemba adapter also maps `55P03` (lock unavailable) to retryable conflict. Catch both
`TransactionConflictError` and the application driver's commit/statement exceptions;
for psycopg use `SerializationFailure`/`DeadlockDetected`, for SQLAlchemy inspect the
underlying `OperationalError.orig.sqlstate`. A retry is a new transaction, with a new
pending token, and must not repeat external side effects unintentionally. A `23505`
unique violation is not a blanket serialization retry; an already-aborted transaction
(`25P02`) needs rollback. The [README retry example](../../README.md#postgres) shows the
psycopg pattern.

## Sync and async integration

The shipped core is synchronous. Rust Postgres uses a blocking client with its own
runtime: construct/use it through `tokio::task::spawn_blocking` in async applications,
not directly on a Tokio runtime thread. Python Postgres engine calls release the GIL;
reads can use `asyncio.to_thread`. Foreign writes accept synchronous psycopg 3,
psycopg2, SQLAlchemy 2 Session/Connection, and Django's connection inside `atomic()`.
Use Rust's `pemba_store_postgres::postgres` re-export and `PgTxn::new(&mut tx)` to keep
the driver version aligned.

For an async application, a worker thread may own a sync connection and the complete
app transaction. SQLAlchemy `AsyncSession.run_sync` is also supported: inside
`session.begin()`, await `session.run_sync(lambda s: engine.write(..., conn=s))` and
resolve after the enclosing commit. Direct asyncpg/psycopg async connections are
rejected; native async adapters (`write_async`, `pemba.aio`, T5.13) remain deferred until
after stable release. Cancelling a task awaiting a worker does not stop that worker;
the application must manage transaction completion and rollback.

Next: [troubleshooting](troubleshooting.md), [schema](schema.md),
[installation](installation.md). The separate T7.5 task adds runnable commit/rollback
tutorials; this guide does not claim those examples have already shipped.
