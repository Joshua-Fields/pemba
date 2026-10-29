# Changelog

All notable changes to pemba are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While pemba is an alpha
(0.1.0 pre-releases), any release may break the API.

The Rust crates and the Python package share one version: `0.1.0-alpha.4` on crates.io is
`0.1.0a4` on PyPI.

## [Unreleased]

### Breaking

- Rust public API audit (T3.9/T7.2): extensible request/filter, response/statistics,
  store result and fixture report structs are now `#[non_exhaustive]`. Migrate request
  literals to `CheckRequest::new`, `CheckItem::new`, `WriteRequest::new` (or `Default`),
  and the existing lookup/read/filter constructors, then assign optional public fields.
  Match outputs with `..`; custom stores use the new `ReverseFilter`, `RelationCount`,
  `StoredSchema`, `GcStats` and `DeleteResult` constructors. Fixture producers use
  `Failure::new` and `Report::default`. See `docs/PUBLIC_API_AUDIT.md` for every decision.
  Existing config defaults/setters, stable tuple/revision value types and storage formats
  are unchanged.

### Added

- Rust `Engine::check_str(resource, permission, subject, consistency) -> Result<bool, Error>`:
  canonical parsing, then the existing typed check. The specified `Error::Parse` is now
  implemented with code `PARSE`; other check errors propagate unchanged. Python preserves
  built-in `ValueError` for malformed arguments; its stubs/signatures are unchanged.
- OQ-044 is complete: only `check_str` is implemented. Use typed write/delete APIs and
  `pemba_schema::compile` instead of the removed promises of Rust `write_str`, `delete_str`
  and static `Engine::validate_schema`. Python schema validation remains available.

## [0.1.0-alpha.4] - 2026-10-28

The first release with the browser playground.

### Breaking

- Python: removed the deprecated `PendingRevision.__int__` and `__index__`
  conversions announced in alpha.3. `int(pending)` and `operator.index(pending)`
  now raise `TypeError`. Migration: call `pending.revision()` after commit when
  you need the revision number, or pass `pending` directly as `at_least=`.

### Added

- **Browser playground** at <https://joshua-fields.github.io/pemba/>: the real
  engine compiled to WebAssembly, running locally in a Web Worker. Edit schemas
  with syntax highlighting and inline diagnostics, run checks and both lookups,
  run fixture assertions, and inspect a collapsible explain tree or copy its text.
- Playground examples for document sharing, GitHub-like permissions and agent
  authorisation; versioned share links and fixture YAML import/export. A version
  mismatch banner links to the original release when available.
- Immutable `playground-<version>.tar.gz` assets attached to GitHub Releases.
  Pages serves the newest playground release at the root, each release at
  `/v/<version>/`, and development builds at `/next/`, with version and commit
  shown in the footer. Tag builds create or update the GitHub Release using
  these changelog notes before attaching the playground.
- Playground CI checks the release WASM against native transcripts for all
  fixtures and 1,000 generated cases, checks generated cases against the reference
  evaluator, and runs frontend unit and browser tests with a 700 KB gzip budget.
- `pemba_fixtures::parse_str` splits fixture YAML into its schema, its relationships
  text and its assertion sections, without running anything (the playground's import;
  `docs/07-testing.md` §3). `FixtureError` gains `Malformed`.

### Changed

- `pemba-core`, `pemba-schema`, `pemba-store-memory` and `pemba-fixtures` build and run on
  `wasm32-unknown-unknown`, for the browser playground (ADR-016). It is not a supported
  WASM package. On wasm32 only, `pemba-core` takes `Instant` from `web-time`, because
  std's `Instant::now` traps there. Other targets gain no dependency.

### Fixed

- The scheduled Postgres property-test workflow parses correctly after quoting
  its test command.

## [0.1.0-alpha.3] - 2026-10-28

The first release with the Postgres store.

### Breaking

- A write inside your own transaction returns a `PendingRevision` instead of a
  `Revision` (ADR-015): Python `engine.write(..., conn=)` and Rust `Engine::write_in`.
  Native writes still return `Revision`. On SQLite it is resolved at once.
  Migration: pass it as `at_least=pending`, or call `pending.revision()` after commit
  (Python); in Rust, `tx.commit()?` then `engine.resolve(&pending)?`.
  `int(pending)` and `operator.index(pending)` keep working for one release with a
  `DeprecationWarning`; comparisons and arithmetic do not. Rust has no transition aid.
- For store implementers only: `pemba_core::write::delete_matching` returns `Deleted` and
  `pemba_core::write::write_schema` returns `SchemaWritten`, which carry an `Applied`
  (a revision, or a batch a commit trigger maps to one). Migration for stores that
  allocate revisions inside the write: `.into_result()` and `.into_stored(source, hash)`.
  `SqlDialect` gains `schema_lock`, defaulting to none.
- `PendingRevision::resolved()` is no longer a `const fn`. `WritableStore` gains
  `resolve_batch`, with a default for stores that number writes inside them.

### Added

- **Postgres store** (`pemba-store-postgres`, Postgres 15+): relationships in a schema of
  your database, revisions assigned at commit so concurrent writers stay in commit order,
  writes that join your transaction (`Engine::write_in` with `PgTxn`), GC that compacts
  and prunes, TLS through rustls. Wheels include it. See the README's Postgres section;
  benchmarks in `benches/RESULTS.md`.
- `PendingRevision` (Rust and Python) and `Engine::resolve`. Python's `at_least=`
  accepts a `PendingRevision`.
- Error codes `REVISION_PENDING` and `WRITE_ROLLED_BACK`, from `Engine::resolve` on a
  store that assigns revisions at commit (Python: `RevisionPending` and
  `WriteRolledBack`, both `ConsistencyError`s).
- Python: `Engine.postgres(dsn, *, schema=None, pool_size=8, auto_migrate=False)`, whose
  calls release the GIL; `write(conn=)` on it joins a psycopg 3 or psycopg2 transaction,
  a SQLAlchemy `Session` / `Connection`, or `django.db.connection`, and works from async
  code through `AsyncSession.run_sync`. `pemba.PembaWarning`. `Engine::store()` in Rust.
- Postgres: a prepared-statement cache per pooled connection (up to 256 statements,
  least recently used evicted and closed on the server). It is emptied on reconnect,
  after a schema write, and on every connection when the server reports a stale plan
  after DDL. It is not used inside your own transaction.
- Postgres: batched writes. A write sends one statement per kind of operation (creates,
  touches, deletes), however many tuples it holds. If a create hits an existing tuple, the
  write is undone to pemba's savepoint and rerun one operation at a time, so
  `AlreadyExists` names the same tuple as before and your transaction stays usable. For
  store implementers: `SqlDialect` gains `bulk_rows`, defaulting to none (one statement
  per operation, as before; SQLite keeps that).
- `pemba check --color <auto|always|never>`. `auto`, the default, is the previous
  behaviour: colour only on a terminal, and not when `NO_COLOR` is set. `always` colours
  even a pipe and overrides `NO_COLOR` (for `less -R`); `never` turns colour off.

### Changed

- `--explain` and `CheckTrace::render()`: when an exclusion denies because its subtracted
  side removed a granted base, both sides are now on the granting path, so the tree shows
  where the permission was granted and where it was removed. The base line is no longer
  marked `(not needed)`.
- `--explain` colour (`CheckTrace::render_ansi()`): evaluated nodes off the granting path,
  `false` ones included, are drawn in the terminal's normal colour instead of dimmed. Only
  skipped nodes and memo references are dimmed. `(not needed)` is printed in colour too,
  so the coloured text is the plain text plus styling.
- `--explain` colour: a line on the granting path is coloured as a whole, not just its
  `✓`/`✗` mark: green where it granted, red where it denied (so the root of a denial is
  red), still bold. The `denied:` header is red. `--color never` output is unchanged.
- Test-only: the `cycle_passes` tests read `check_bulk` stats from one global, always-on
  span recorder filtered to the calling thread, instead of a per-call scoped subscriber,
  the same capture as the `lookup` tests (#42, #43).
- The README is written for newcomers: no internal task or decision numbers, the
  specification linked once under "Design documents", and a diagram comparing a separate
  authorisation service (two databases, dual writes) with pemba (one transaction).

### Performance

- On Postgres, latency is round trips. A direct check costs about 5 round trips. Each
  level of hierarchy it walks adds about 3, so a document 30 folders deep costs 99. A
  write costs 10 whatever its size. On a Linux CI runner where one round trip took about
  150 µs, p50 was 0.76 ms for a direct check (1.58 ms without the statement cache),
  16 ms for 30 folders (47 ms) and 19 ms for a 1,000-tuple write (407 ms). Full figures,
  with round trips for every scenario: `benches/RESULTS.md`. Fewer round trips for deep
  hierarchies is planned.

### Fixed

- Postgres: paging `read_relationships` at an older revision no longer sorts every tuple
  of the type for each page. The first 500-tuple page over 100,000 tuples went from
  106 ms to under 1 ms, because the planner had misjudged how many rows the
  older-revision filter keeps. Answers are unchanged.

## [0.1.0-alpha.2] - 2026-10-27

### Added

- `Engine::check_with_trace`: the same answer as `check`, plus the tree of steps that
  reached it: which members were evaluated, which stored relationship matched, what was
  skipped, reused from the memo, or settled by cycle iteration. Errors return no trace
  (fail closed). `Engine::check_with_trace_limit` caps the recorded nodes for one call.
  Tracing costs nothing when you call plain `check`.
- `CheckTrace`, `TraceNode`, `TraceKind`, `TraceSource` and `TraceRole`, with
  `CheckTrace::render()` (the `--explain` text), `render_ansi()` and `to_json()`, a public
  JSON format (`"format": "pemba.check_trace"`, version 1) documented in `docs/06-api.md`
  §3.1.
- `EngineConfig::max_trace_nodes` (default 10 000; `with_max_trace_nodes`). Past it the
  trace is marked truncated; the answer is unaffected.
- `pemba check <resource>#<permission>@<subject>` in the CLI, against a fixture file
  (`--fixture`) or an existing SQLite database opened read-only (`--db`), with `--explain`
  (the trace as a tree, coloured on a terminal unless `NO_COLOR` is set), `--json` and
  `--at-exactly REV`. Exits 0 if allowed, 1 if denied, 2 on any error.
- `SqliteStore::open_read_only`: opens an existing, migrated database without the right to
  change it. It never creates or migrates the file, and every write fails.
- Python: `Engine.check_with_trace(...)`, taking both call forms of `check` and
  `max_trace_nodes=10_000`, returning a `CheckTrace` with `.allowed`, `.revision`, `.root`,
  `.truncated`, `.to_dict()` (the `--json` shape) and `.render()`; `TraceNode`. Type stubs
  included.
- `pemba_fixtures::load`, which reads a fixture file's schema and relationships, and
  `pemba_schema::ir::TypeDef::expr_source`, which prints one permission expression.

### Changed

- `EngineConfig` is now `#[non_exhaustive]`, with a `with_*` setter per field (and a new
  field, `max_trace_nodes`). It can no longer be built with a struct literal outside
  `pemba-core`, `..EngineConfig::default()` included; fields stay readable:

  ```rust
  // before: EngineConfig { max_depth: 100, ..EngineConfig::default() }
  // after:  EngineConfig::default().with_max_depth(100)
  ```
- `EngineBuilder::build` rejects `max_trace_nodes = 0` with `InvalidRequest`.

### Fixed

- `check_with_trace`, before release: with a subject-set subject, a memo hit could refer
  to a later, unsettled evaluation of the same member under an exclusion's subtracted side
  instead of the node that settled its value, so a `✓` hit pointed at a `✗` node. Found by
  the trace property test; answers were never affected.
- The Python package's README (`crates/pemba-py/README.md`) is in sync with the
  repository README again.

## [0.1.0-alpha.1] - 2026-10-20

First public pre-release.

### Added

- A schema language that is a strict subset of SpiceDB's: definitions, relations with
  object, wildcard (`user:*`) and subject-set (`group#member`) types, and permissions over
  union, intersection, exclusion and arrows (`->`, `.any`, `.all`). Diagnostics are
  rustc-style. Caveats, `with expiration`, `use` and `self` are recognised and rejected
  with `E_UNSUPPORTED_FEATURE`.
- The Rust `Engine` (`pemba` crate): `write_schema` (refusing changes that would orphan
  live relationships), `write` with preconditions, `write_in` to join the application's
  own SQLite transaction, `check`, `check_bulk`, paged `lookup_resources`,
  `lookup_subjects`, `read_relationships`, `delete_relationships` and `gc`. Every read is
  at one revision (`Latest`, `AtLeast` or `Exact`). Evaluation fails closed: an error is
  never turned into an answer.
- Data-dependent cycles are evaluated to their least fixed point; cycles through negation
  are refused with `NonMonotonicCycle`. Limits on depth, fan-out, store reads per check
  and lookup size.
- Stores: in-memory (`MemoryStore`) and SQLite (`SqliteStore`, WAL, migrations).
- The Python package `pemba`: `Engine.sqlite()` on CPython's own `sqlite3` module (one
  SQLite library per process), `Engine.memory()`, writes inside the application's
  transaction (`write(..., conn=...)`), a `PembaError` exception hierarchy, and type
  stubs. abi3 wheels for CPython 3.11+ on Linux (x86_64, aarch64), macOS (arm64, x86_64)
  and Windows (x86_64).
- The `pemba` CLI (`pemba-cli`): `schema validate`, `schema fmt` (experimental; keeps
  comments) and `test`, which runs YAML fixture files.

[Unreleased]: https://github.com/Joshua-Fields/pemba/compare/v0.1.0-alpha.4...HEAD
[0.1.0-alpha.4]: https://github.com/Joshua-Fields/pemba/compare/v0.1.0-alpha.3...v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/Joshua-Fields/pemba/compare/v0.1.0-alpha.2...v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/Joshua-Fields/pemba/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/Joshua-Fields/pemba/releases/tag/v0.1.0-alpha.1
