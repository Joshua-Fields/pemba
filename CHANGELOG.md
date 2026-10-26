# Changelog

All notable changes to pemba are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While pemba is an alpha
(0.1.0 pre-releases), any release may break the API.

The Rust crates and the Python package share one version: `0.1.0-alpha.2` on crates.io is
`0.1.0a2` on PyPI.

## [Unreleased]

## [0.1.0-alpha.2] - 2026-10-26

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

[Unreleased]: https://github.com/Joshua-Fields/pemba/compare/v0.1.0-alpha.2...HEAD
[0.1.0-alpha.2]: https://github.com/Joshua-Fields/pemba/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/Joshua-Fields/pemba/releases/tag/v0.1.0-alpha.1
