# Commit app data and permissions together

This tutorial creates a document and grants Anne permission to view it in **one
application-owned database transaction**. It then inserts another document and its
permission, forces a duplicate-primary-key error, and rolls the entire transaction
back. Every example asserts that only the committed document and permission survive.

Run from the repository root, using the [installation guide](installation.md) for
the pinned Rust toolchain and Python >=3.11/source bindings. These examples exercise
the current checkout, including its post-alpha.4 Rust constructors/check helper.
They add no supported API or new package. Use fresh disposable databases/schemas;
the scripts create tables and write a schema, and are intentionally not rerunnable
against populated tutorial data.

## SQLite: Rust and Python

Run each command with a different fresh file path (parent directory must exist):

```sh
cargo run -p pemba-store-sqlite --example transactional_sqlite -- tutorial-rust.db
python examples/transactional_sqlite.py tutorial-python.db
```

Read the executable sources:
[SQLite Rust](../../examples/transactional_sqlite.rs) and
[SQLite Python](../../examples/transactional_sqlite.py).
Rust uses `pemba_store_sqlite::rusqlite` and `SqliteTxn`, ensuring the app and
store use the same SQLite library. Python uses `sqlite3` for both engine and app.
Do not load another SQLite copy into the same process against these files; see
[the SQLite rules](transactions.md#sqlite).

The sequence is deliberately explicit:

1. Open/migrate the engine, write the document schema, and create the app table.
2. Begin an app transaction (`BEGIN IMMEDIATE` on SQLite), insert `committed`, and
   call `write_in` / `write(conn=app)` to grant its `owner` relation to Anne.
3. SQLite's pending token already knows the number. Reading with `AtLeast` before
   commit raises `RevisionNotYetAvailable` (`ConsistencyError` in Python), while
   `Latest` still denies. These are different outcomes: an error is never `false`.
4. Commit using the app connection. Resolve the pending token, then check using
   `AtLeast(revision)` / `at_least=revision`: Anne is allowed and Bob is denied.
5. Begin another transaction, insert `rolled_back`, write its permission, then force
   an app SQL error by inserting the same primary key again. Roll back the **whole**
   app transaction. Assert the app row is absent and Anne is denied on that document.

After SQLite rollback the token's number is void and may be reused. Resolution
cannot detect that rollback. Discard the token; portable application code resolves
only after a successful commit. No example treats an uncommitted check as an app
transaction read: checks always use engine-owned connections.

## Postgres: Rust and Python

Use a local disposable Postgres >=15 UTF8 server. Set `PEMBA_PG_URL` in your
environment to its connection URL; keep real credentials out of source and logs.
For Python install psycopg 3 into the example environment (`python -m pip install
"psycopg[binary]"`); the project's separate tool-only check venv stays unchanged.
The Rust app connection uses `NoTls` for this local tutorial. Remote applications
must configure TLS for their app driver too; the engine's rustls configuration does
not configure the app's connection. See [TLS and migrations](transactions.md).

Create a separate empty schema for each run, using `psql` with the configured URL:

```sh
psql "$PEMBA_PG_URL" -c 'CREATE SCHEMA tutorial_rust'
PEMBA_TUTORIAL_SCHEMA=tutorial_rust cargo run -p pemba-store-postgres --example transactional_postgres
psql "$PEMBA_PG_URL" -c 'CREATE SCHEMA tutorial_python'
PEMBA_TUTORIAL_SCHEMA=tutorial_python python examples/transactional_postgres.py
```

In PowerShell set `$env:PEMBA_TUTORIAL_SCHEMA='tutorial_rust'` before the Cargo
command, then set it to `tutorial_python` before the Python command. Pass
`$env:PEMBA_PG_URL` to `psql` instead of the shell variable above.

Read [Postgres Rust](../../examples/transactional_postgres.rs) and
[Postgres Python](../../examples/transactional_postgres.py). The Rust example uses
`PgTxn`; Python uses psycopg 3's `transaction()` context, which commits on normal
exit and rolls back on an exception. Only the deliberate duplicate app-row error
is handled as the expected rollback case; other failures propagate.

The commit/rollback assertions match SQLite, with one key store distinction:
Postgres assigns the revision at commit. Before commit the token is unresolved
and `resolve` / `.revision()` raises `RevisionPending`. After commit resolution
succeeds and the `AtLeast` check allows Anne. After forced rollback resolution
raises `WriteRolledBack`; neither app row nor permission remains. Pending and
rolled-back errors are not permission denials and must not be swallowed.

These are single-writer examples with default READ COMMITTED isolation. Production
code must handle commit failures and retry the **entire** transaction on a retryable
conflict, re-reading app data and preconditions. Early constraints, SERIALIZABLE
retries, supported drivers and sync/async limits are covered in the
[operations guide](transactions.md); no native async adapter is introduced here.

## Regression checks and related examples

The Rust `write_in` and `foreign` integration suites and the Python example suites
execute these same source files against temporary SQLite files or fresh Postgres
schemas. They reuse existing temporary-database helpers. The Postgres schema
fixtures clean up on successful runs; release/service runs set both
`PEMBA_PG_URL` and `PEMBA_PG_REQUIRED=1` so an absent service fails the run.

```sh
cargo test -p pemba-store-sqlite --test write_in
cargo test -p pemba-store-postgres --test foreign
# In the Python crate's configured environment, with all supported drivers installed:
cd crates/pemba-py
pytest -q tests/test_examples.py tests/test_foreign_txn.py tests/test_postgres.py
```

Existing suites retain mismatched-database, savepoint, autocommit and driver-guard
coverage; this task adds the tutorial execution paths rather than reproducing them.
The Rust examples are registered on their existing store crates and use their
existing dependencies. The separate service CI jobs run the Postgres tutorial on
Postgres 15/18 alongside the supported driver tests. A skipped Postgres suite is
not release evidence.

For richer policies, see the [agent script](../../examples/agent_authz.py), its
[agent fixture](../../testdata/fixtures/agent_authz.yaml),
[document-sharing fixture](../../testdata/fixtures/google_docs.yaml) and
[GitHub-like fixture](../../testdata/fixtures/github_like.yaml).
The [playground](https://joshua-fields.github.io/pemba/) runs a memory store in
WASM. It can demonstrate schema/check/lookup semantics, but has no app database or
foreign database connection and **cannot demonstrate a real database transaction**.
