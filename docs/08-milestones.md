# 08 — Milestones and tasks

Work strictly in order unless a task says it can run in parallel. Each task: goal, spec refs,
acceptance criteria (AC). A task is done only when every AC holds **and** the local green
check in `CLAUDE.md` §5 passes.

Durations are a budget for part-time work: M0 weeks 1–2, M1 weeks 3–4, M2 weeks 5–6,
M3 weeks 7–8. Gate to 0.1 at the end of M3.

---

## M0 — Foundations

### T0.1 Workspace and CI
Refs: `09` all.
AC:
- Cargo workspace with crates from `09` §1 (empty lib crates compile).
- `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`/workspace lints, `deny.toml`.
- GitHub Actions: fmt, clippy `-D warnings`, test (Linux/macOS/Windows), `cargo deny check`.
- `LICENSE` (Apache-2.0), `NOTICE` (project name only for now), `README.md` stub crediting
  Zanzibar and SpiceDB as design references.

### T0.2 Core model types and tuple parser
Refs: `02` §1–3.
AC:
- `ObjectRef`, `SubjectRef`, `Relationship`, `Revision`, `Consistency`, `WriteOp`,
  `RelationshipFilter` implemented, all with `Display`.
- `FromStr` (canonical syntax, `02` §3) for `ObjectRef`, `SubjectRef`, `Relationship` and
  `Revision` only; the name newtypes `TypeName`, `RelName`, `ObjectId` also implement it as
  their validating constructors. `Consistency`, `WriteOp` and `RelationshipFilter` have no
  `FromStr`: their `Display` is a debugging aid, not a stable format (OQ-009).
- `Revision` text form is decimal digits with no sign and no leading zeros (`0` itself is
  allowed), so every revision has exactly one form.
- Identifier rules (`02` §1.1) are implemented once, in `pemba-schema`, and reused by
  `pemba-core`.
- All examples in `02` §3 table parse; all invalid examples in `02` §1 rejected with
  positioned `ParseError`.
- Round-trip property test passes for every `FromStr` type.

### T0.3 Schema lexer and parser → AST
Refs: `03` §2–4, §7.
AC:
- Parses `03` §1 example. Doc comments attached to the right node.
- Every unsupported feature in `03` §7 yields `E_UNSUPPORTED_FEATURE` naming the feature.
- Mixing rule enforced (`E_AMBIGUOUS_OPERATORS`) with fix-it text.
- `E_ARROW_CHAIN`, semicolon hint, unterminated comment, unexpected char all covered.
- Diagnostics rendered rustc-style; snapshot tests with `insta`.

### T0.4 Semantic validation and compiled IR
Refs: `03` §5–6.
AC:
- Every error code in `03` §5 has a failing test; all errors collected, sorted by span.
- Warnings emitted as specified.
- `CompiledSchema` with `TypeId`/`RelId` interning, per-arrow target maps, and the derived
  indexes `used_by`, `arrows_into`, `subject_set_uses` (unit-tested on the §1 example).

### T0.5 Pretty printer and `pemba schema validate|fmt`
Refs: `03` §9, `06` §3.
AC:
- Canonical printing; round-trip property test.
- `CompiledSchema::hash` (sha-256 of the normalised source, `03` §6), deferred from T0.4
  because it depends on the normaliser.
- `pemba-cli` binary with `schema validate` and `schema fmt [--check]`; correct exit codes.

---

## M1 — Check engine

### T1.1 Store traits + memory store
Refs: `05` §1–3.
AC:
- Traits as specified (document the object-safety choice).
- `MemoryStore` implements all reads with revision visibility and the write algorithm natively.
- First version of the store conformance suite (`05` §9) passing for `MemoryStore`.

### T1.2 Engine skeleton: schema + writes
Refs: `02` §4–6, `06` §1.
AC:
- `Engine::builder`, `write_schema`, `read_schema`, `write`, `head_revision`, `read_relationships`.
- Relationship validation against schema with exact error codes.
- Batch atomicity, duplicates, preconditions, batch size limit tested.

### T1.3 `check` evaluator
Refs: `04` §1–4.
AC:
- Implements §3.2–3.5 exactly, including self membership, wildcard rules, arrow subject-relation
  ignoring, `.all` over zero objects = false.
- Cycle handling per §3.6 with tests for: tainted result not memoised (the `A = B + C`,
  `B = A` example), nested-group cycles, `NonMonotonicCycle`.
- `MaxDepthExceeded` carries a readable path. `check_bulk` implemented.
- `tracing` span per check with stats fields.

### T1.4 Reference evaluator + property tests
Refs: `07` §2.
AC:
- Reference evaluator independent of `pemba-core::eval`.
- Generators per §2.2; properties 1 and 6 from §2.3 passing at ≥ 1 000 cases.

### T1.4c Bound check cost on cyclic data
Refs: `04` §3.6, `06` §1, OQ-021. After T1.4, before T1.4b.
AC:
- Taint is lowlink-style: each result carries the lowest stack index of an in-progress node
  whose value it used.
- SCC fixed-point iteration per `04` §3.6 (planned rule, T1.4c): cycle hits return the
  previous pass's provisional value; each SCC member is evaluated at most once per pass
  (pass memo); the head re-runs until no provisional value changes; then every member
  evaluated in the final pass is memoised. `NonMonotonicCycle` behaviour is unchanged.
- New per-request cap on total store reads, `EngineConfig::max_check_reads` (default per
  `04` §3.6); exceeding it returns a new error (fail closed), documented in `06`.
- Every adversarial test in `crates/pemba-testkit/tests/check_cost.rs` passes with its
  `#[ignore]` removed.
- Property 1 at ≥ 1 000 cases passes in under 60 s optimised, on several seeds.

### T1.4b Stratified negation for cycles
Refs: `04` §3.6, OQ-018. After T1.4c (needs lowlink-style taint).
AC:
- A cycle is `NonMonotonicCycle` only if the path from the in-progress node (including its own
  expression) to the closing edge crosses the subtracted side of an exclusion.
- A monotone cycle wholly inside a subtracted side is evaluated as a least fixed point, then
  negated: `any_cycle_beneath_subtracted_side_is_refused` becomes a test of the correct
  answer (nested banned groups that contain each other).
- The `A = B + C`, `B = A` tests still pass.
- Every existing `NonMonotonicCycle` test (self-loop and two-node cycles through
  `view = viewer - parent->view`) still errors.
- Property 1 (`07` §2.3) passes at ≥ 1 000 cases with a generator variant that allows
  recursive members (monotone cycles) under the subtracted side, compared against the
  reference evaluator extended with stratified evaluation of the `sub` side.

### T1.5 Fixture runner + corpus (part 1)
Refs: `07` §3.
AC:
- Runner + `pemba test` CLI command.
- Fixtures from §3.1 that need only `check`: `direct`, `wildcard`, `subject_sets`,
  `nested_groups_cycle`, `arrows_folders`, `arrows_ignore_subject_relation`, `intersection`,
  `exclusion_banned`, `all_arrow`, `any_alias`, `self_membership`, `google_docs`,
  `github_like`, `multi_tenant`, `prefixed_types`, `depth_limit`, `non_monotone_cycle`,
  `schema_errors/*`. All pass on the memory store.

---

## M2 — Persistence and lookups

### T2.1 SQLite store: tables, migrations, connections
Refs: `05` §4.
AC:
- DDL exactly as specified; migration runner; version checks with the specified errors.
- Writer mutex + reader pool; pragmas applied; `EXPLAIN QUERY PLAN` tests prove index use
  for every query in `05` §6.

### T2.2 Shared write algorithm over `WriteTxn`
Refs: `05` §5, ADR-006.
AC:
- Write logic lives in `pemba-core::write`, generic over `WriteTxn` + `SqlDialect`.
- SQLite native `WriteTxn`; `Engine::write_in` works with a caller-owned `rusqlite::Transaction`
  (Rust-side proof of the foreign-transaction design).
- Rollback test: app insert + `write_in` + rollback → neither visible.
- Conformance suite passes for `SqliteStore`; property 5 (memory ≡ SQLite) passes.

### T2.3 Consistency and MVCC
Refs: `02` §4, `05` §5.1–5.2.
AC:
- `Latest`/`AtLeast`/`Exact` resolution with exact error variants.
- Property 4 (historical reads) passes on both stores.
- Concurrency test: N reader threads + 1 writer, readers never see a partial batch.

### T2.4 `lookup_resources`
Refs: `04` §5.
AC:
- Two-phase algorithm, shared memo, pagination by cursor, `LookupTooLarge` limit.
- Property 2 passes on both stores. `expectedLookups.resources` supported in fixtures.

### T2.5 `lookup_subjects`
Refs: `04` §6.
AC:
- `SubjectSet` algebra with all 12 cells unit-tested + model property test.
- Property 3 passes. `expectedLookups.subjects` supported in fixtures.

### T2.6 Schema change safety, delete-by-filter, GC
Refs: `03` §10, `05` §7–8.
AC:
- `SCHEMA_CHANGE_ORPHANS` with per-relation counts; permission removal always allowed.
- `write_schema` makes stale relationships (OQ-019 (e) and the two remaining cases)
  impossible **at head**: a test proves they cannot arise at head after any accepted
  `write_schema`. Historical reads (`Exact(r)` before a schema change) can still meet tuples
  the current schema rejects, so `StaleRelationship` stays as the fail-closed guard; a test
  proves an `Exact(old)` read over a since-removed allowed type returns
  `StaleRelationship`, not an answer.
- Multi-process schema reload test (two engines on one SQLite file).
- `delete_relationships` with limit; `gc` in batches, horizon raised first in its own
  transaction (`05` §8); `RevisionExpired` after GC; a reader mid-GC gets the full answer
  or `RevisionExpired`, never part of one.
- Property 4 extended to include GC (reads at `Exact(r)` below the horizon fail with
  `RevisionExpired`).

---

## M3 — Python, verification, release

### T3.1 `pemba-py` bindings
Refs: `06` §2.
AC:
- All signatures in `06` §2.2 with `.pyi` stubs; `mypy --strict` passes on a usage sample.
- GIL released for engine calls; exception hierarchy mapped by `Error::code()`.
- pytest suite per `07` §6 (except foreign txn) passes.

### T3.2a Python SQLite store (single SQLite copy)
Refs: ADR-006 (amended, OQ-032), `05` §4.3–§4.4, `06` §2.
AC:
- `pemba-store-pysqlite`: every pemba statement (reads, native writes, migrations, GC,
  schema writes) runs on Python `sqlite3` connections the engine owns, reusing
  `SqlDialect` / `WriteTxn` / `ReadStore`; `Engine.sqlite()` uses it.
- `pemba-py` depends on neither rusqlite nor a bundled SQLite; CI fails if the built
  extension or the release wheel contains SQLite symbols.
- Same-snapshot rules as T2.3 and GC order as T2.6.
- `MIN_SQLITE_VERSION` derived from the SQL features used; `Engine.sqlite()` refuses an
  older `sqlite3` with a `StorageError` naming both versions; CI job on the oldest
  claimed Python/SQLite combination.
- The two-copies probe is a permanent regression test that passes.
- Conformance suite and property 5 (memory ≡ `PySqliteStore`) pass.
- `06` §2 and the README say the engine shares CPython's SQLite and which libraries are
  safe in-process.

### T3.2b Foreign transaction mode (headline feature)
Refs: `05` §10, ADR-006.
AC:
- `engine.write(..., conn=sqlite3_conn)` via `PyDbApiTxn`.
- Same-file verification, autocommit guard, error mapping.
- pytest: commit/rollback/uncommitted-revision/mismatch tests pass.
- README example from `06` §2.1 runs verbatim in CI.

### T3.3 Wheels and CI release pipeline
Refs: ADR-014, `09` §4.
AC:
- maturin-built abi3 wheels for the platforms in ADR-014; smoke-tested on each.
- `cargo publish --dry-run` passes for publishable crates. (Actual publishing: human only.)
- Fixture format and engine-only runner moved from `pemba-testkit` into a small publishable
  crate that `pemba-cli` depends on; the oracle cross-check stays in `pemba-testkit` as an
  optional hook for internal tests (OQ-024). The CLI no longer depends on `pemba-testkit`.

### T3.4 Differential testing against SpiceDB
Refs: `07` §4.
AC:
- `pemba-difftest` harness; all fixtures + ≥ 500 generated cases agree with SpiceDB, or each
  divergence is listed in `KNOWN_DIFFERENCES.md` with a reason and an ADR update if semantics
  changed (e.g. `.all` over zero objects, self membership, arrow over wildcard).
- CI job (manual trigger + weekly) running it in Docker.

### T3.4b KD-3 fails closed: a wildcard on a subtracted side vs a subject-set subject
Refs: `04` §2, §3.3; `07` §2.3; `crates/pemba-difftest/KNOWN_DIFFERENCES.md` KD-3.
AC:
- A `check` or `lookup_resources` whose subject is a subject set of type `t`, and whose
  evaluation reaches a stored `t:*` under the subtracted side of an exclusion, returns the
  new error `AMBIGUOUS_WILDCARD_EXCLUSION` instead of an answer. The grant side is
  unchanged: a wildcard never matches a subject set.
- Rationale in `04` §2 and §3.3: pemba and SpiceDB disagree there. An error can later
  become an answer without breaking callers; an answer cannot change silently.
- The oracle and properties 1 and 2 accept the error only where the oracle shows the case
  applies; the condition is specified in `07` §2.3. Mutation check: removing the error
  makes a test fail.
- Python exception mapping (`AmbiguousWildcardExclusionError`) and the stubs.

### T3.5 Benchmarks
Refs: `07` §7.
AC:
- Criterion + pytest-benchmark scenarios implemented; results table generated by script into
  `benches/RESULTS.md` with hardware/version metadata.
- SpiceDB comparison scripts runnable end-to-end.
- Open question to answer with numbers: should `lookup_resources` cache phase 1 candidates
  per (revision, query) across pages (OQ-028 (d))? Each page re-runs phase 1 today.

### T3.6 Fuzzing and hardening
Refs: `07` §5.
AC:
- Three fuzz targets build and run ≥ 10 minutes each with no crashes; corpus committed.
- `cargo-mutants` on `eval` and schema validator: surviving mutants reviewed and either killed
  by new tests or justified in `docs/MUTANTS.md`.

### T3.8 Comment-preserving formatter (print from the AST with comment trivia)
Refs: `03` §9, `06` §3, OQ-013. Lands before the T3.7 gate.
AC:
- Plain comments (`//` and `/* */`) survive `pemba schema fmt`.
- `pemba schema fmt --check` passes on canonical files containing comments.

### T3.9 Audit public structs for `#[non_exhaustive]` + constructors before 0.1.0
Refs: `06` (frozen once 0.1 ships), `09` §3 ("`#[non_exhaustive]` where future fields are
likely"; "builders for configs"), OQ-044. Lands before the T3.7 gate.
AC:
- Every public struct and enum in the published crates is listed with a decision: made
  `#[non_exhaustive]` with a constructor or setters (as `EngineConfig` was in alpha.2), or
  left exhaustive with a one-line reason. The request structs (`CheckRequest`,
  `LookupResourcesRequest`, `LookupSubjectsRequest`, `ReadRelationshipsRequest`,
  `WriteRequest`, `CheckItem`, ...) are the main question: users build them with struct
  literals today.
- Each change is in `06` and the CHANGELOG with a migration line.
- OQ-044 (the `check_str` / `write_str` / `validate_schema` helpers) is resolved: built or
  removed from `06`.

### T3.7 Documentation and 0.1 gate
AC (release gate — human signs off):
- README: pitch, install, quick start (Python + Rust), the transactional-write example,
  schema guide (supported subset + differences from SpiceDB), consistency guide, limits,
  "when to use SpiceDB instead", credits.
- `docs.rs`-ready rustdoc on all public items; Python docstrings on all public methods.
- All CI jobs green, differential tests green, no open `OPEN_QUESTIONS.md` items marked
  `blocking`.
- Final name chosen (ADR-012) and rename completed.
- T3.9 (public-struct audit) done.

---

## M4 — Stretch

### T4.1 `check_with_trace` + `pemba check --explain`
Refs: `04` §3.6, §3.8; `06` §1, §2.2, §3; `07` §2.
AC:
- `Engine::check_with_trace(CheckRequest) -> Result<(CheckResponse, CheckTrace), Error>`
  in `pemba-core`, re-exported from `pemba`. Same evaluator as `check` with a trace sink;
  `check` uses a no-op sink that compiles away, and the `check` benchmarks in `benches/`
  do not regress beyond noise (numbers before and after in the summary).
- `CheckTrace` is the tree specified in `04` §3.8: per node `node`, `kind`, `result`,
  `source` (evaluated, memo hit, cycle with its pass count, cycle hit, skipped),
  `children`, `tuple` (the matched relationship on every direct `true`), `duration`.
- Memo hits are references to the one node shown in full; subtrees are never duplicated.
- Cycles: the trace shows the final, settled pass and records the pass count; no
  intermediate pass's value is shown as an answer.
- Fail closed: an evaluation error returns the `Err` and no trace.
- `EngineConfig::max_trace_nodes` caps the nodes recorded; reaching it returns the
  correct answer with the trace marked `truncated`, never an error.
- Property test (`pemba-testkit`): for every generated case, cyclic data included, on
  both stores, `check_with_trace(..).0.allowed == check(..) == reference`, the root's
  result equals `allowed`, and the trace invariants of `04` §3.8 hold; 1 000+ cases in the
  ignored variant.
- insta snapshots of rendered traces for `google_docs`, `nested_groups_cycle`,
  `exclusion_banned` and `all_arrow`.
- CLI: `pemba check <resource>#<permission>@<subject> (--fixture f.yaml | --db app.db)
  [--explain] [--json] [--at-exactly REV]`; `--db` opens the database read-only; exit 0
  allowed, 1 denied, 2 error (`06` §3). `--explain` renders the tree with colour only on a
  TTY and never with `NO_COLOR`; `--json` emits the public trace format (`06` §3.1).
- Python: `Engine.check_with_trace(...)`, both call forms of `check`, returning a
  `CheckTrace` with `.allowed`, `.revision`, `.root`, `.to_dict()` (the CLI JSON) and
  `.render()` (the `--explain` text without colour); stubs, docstrings, `mypy --strict`, and
  a test that `check_with_trace(..).allowed == check(..)` over every fixture.
- README: a short "Debugging permissions" section with one rendered `--explain` example.

---

## M5 — Postgres store

Design: `docs/10-postgres.md` (accepted 2026-10-27; OQ-002, OQ-003 and OQ-045 resolved).
Each task ends green: the §5 checks of `CLAUDE.md`, plus the Postgres CI job once it exists.
"First Postgres release" means the release that contains T5.0–T5.12.

### T5.0 Accept the design (docs only)
Refs: `10` §0.1, §1.6, §2; OQ-002, OQ-003, OQ-045.
AC:
- The maintainer records the OQ-002, OQ-003 and OQ-045 resolutions.
- Add ADR-015 (Postgres revisions assigned at commit; `PendingRevision` on both stores).
- Amend ADR-003 (sync; async adapters later), ADR-005 (consequences; skipped numbers)
  and ADR-006 (Postgres drivers and placeholders).
- Update `02` §4, `05` (§10, new §11), `06` (`PendingRevision`, `Engine.postgres`) and
  README (`10` §1.6.3 wording, F5, the `SET CONSTRAINTS` warning, F13's "one write call,
  one revision", and SERIALIZABLE retry-with-backoff guidance).
- CHANGELOG "Breaking" entry with the `10` §1.6.4 migration table.
- Remove Postgres from `CLAUDE.md` §6.

### T5.1 `PendingRevision` on SQLite (the breaking change, shipped first)
Refs: `10` §1.6.4; `06` §1, §2.1, §2.2; OQ-045.
AC:
- Core `PendingRevision` and `Engine::resolve`.
- `write_in` and Python `write(conn=)` return it; `at_least=` accepts it.
- The deprecated `__int__` / `__index__`, plus `.pyi`, docs and examples.
- Done when the whole workspace and pytest are green with no Postgres code.

### T5.2 Core: revision strategy in `pemba-core::write`
Refs: `10` §1.6.1.
AC:
- `SqlDialect::revision_strategy()` is `AllocateNow` (SQLite) or `AssignAtCommit`.
- Dialect-driven placeholder style; `RETURNING ctid` through `WriteTxn::query`.
- SQLite behaviour unchanged; done when green.

### T5.3 `pemba-store-postgres` skeleton and migrations
Refs: `10` §2.1, §3.
AC:
- Crate, config, pool, rustls TLS feature.
- Migration v1: tables, sequences, the batch-map trigger (one INSERT plus early-firing
  detection), and batch 0 → revision 0.
- `migrate` / `open` / `NotMigrated` / `StorageVersionTooNew`, version and `UTF8`
  checks, schema qualification.
- Postgres CI job (service container, 15 and 18).
- Tests: idempotent and concurrent `migrate`, non-default schema, hostile `search_path`,
  and the `spawn_blocking` check.

### T5.4 Reads
Refs: `10` §1.6.2; `05` §6.
AC:
- `ReadStore`, head/horizon (head = highest committed), the REPEATABLE READ
  `snapshot()`, and the `05` §6 queries in two forms: `deleted_batch IS NULL` at head,
  and the map join below head (`10` §1.6.2).
- `COLLATE "C"` ordering.
- The conformance read cases pass.

### T5.5 Native writes
Refs: `10` §1.6.1, §1.6.7, §4.1.
AC:
- `write`, `write_schema` and `delete_matching` through the commit trigger, the
  post-commit lookup, and SQLSTATE mapping.
- The full conformance suite passes (the `10` §4.1 additions included).
- Store equivalence memory ≡ SQLite ≡ Postgres.
- Early-firing detection: the `early` column, the WARNING, and the warning from
  `resolve()` (`tracing` in Rust, `PembaWarning` in Python) (`10` §1.6.7).

### T5.6 Permanent revision-order concurrency test
Refs: `10` §4.4, §1.6.8.
AC:
- The `10` §4.4 test at all three isolation levels, plus the `SET CONSTRAINTS`
  variants, the commit-cost check and the negative control.
- SERIALIZABLE with backoff: assert 0 transactions failing all retries (`10` §1.6.8).

### T5.7 GC
Refs: `10` §1.6.3, §1.6.5; `05` §8.
AC:
- Horizon first, then deletes through the map in ctid batches, and pruning of pending
  rows.
- Map pruning: measure the rebase against keeping the map, then pick one (`10` §1.6.5).
- The mid-GC reader test, and GC at skipped numbers.
- GC compacts batch ids into rows outside the app transaction and prunes the batch map.
- Property test: GC + compaction never changes an answer.
- Real-engine bench of `at_exactly` reads (lookup paging) before and after compaction;
  the design spike measured +34% at p50 for a 500-row lookup at an old revision using
  SQL approximations only.

### T5.8 Rust foreign mode
Refs: `10` §1.6.4, §2.1.
AC:
- `PgTxn` over `postgres::Transaction`, `write_in` → `PendingRevision`, `resolve` with
  `pg_xact_status`.
- Same-database check, tested as an unprivileged role.

### T5.9 Python native engine and sync foreign mode
Refs: `10` §2.2, §2.3, §2.4.
AC:
- `Engine.postgres(...)` with the GIL released, and `.pyi`.
- Adapters for psycopg 3, psycopg2, SQLAlchemy `Session` / `Connection`, and Django:
  placeholders, error mapping, guard rails, `TypeError` for async connections.
- Verify Django's `SET CONSTRAINTS` use and try the `run_sync` route (`10` §2.4).

### T5.10 Wheels with Postgres support
AC:
- Wheels build with Postgres support (rustls, no OpenSSL).
- A CI smoke test against the service container.

### T5.11 Property tests and difftest on Postgres in CI
AC:
- `proptest.yml` and `difftest.yml` run a Postgres matrix entry.

### T5.12 Benchmarks and documentation, then the first Postgres release
Refs: `07` §7; `10` §0.1 (F12).
AC:
- Benchmarks: write throughput against SQLite, check latency, 1,000-tuple commit
  cost, and the read cost below head through the map (F12), on the `07` §7 shapes.
- README: a Postgres section, privileges, PgBouncer, autovacuum, 2PC, the
  `SET CONSTRAINTS` warning, and SERIALIZABLE retry guidance.

### T5.13 Async foreign mode (after the first Postgres release)
Refs: `10` §2.4; OQ-003.
AC:
- `write_async` with the thread bridge (`10` §2.4) for psycopg `AsyncConnection`, asyncpg
  and SQLAlchemy `AsyncSession`, and the `pemba.aio` read helpers.
- Benchmark the per-statement hop, which decides whether the sans-IO follow-up is
  needed.

---

## M6 — Playground

Design: `docs/11-playground.md` (accepted 2026-10-28; D1–D4 resolved by the maintainer).
Each task ends green: the §5 checks
of `CLAUDE.md`, plus the `wasm` CI job from T6.1 and `playground.yml` from T6.3 once they
exist. Evidence: the unmerged spike branch `t6.0-playground-spike`.

### T6.0 Accept the design (docs only) — done in #67
Refs: `11` §0, §2.
AC:
- The maintainer records D1–D4 in `11` §0 and sets its status to accepted.
- ADR-016 (WASM for the playground build only; no published WASM package).
- `CLAUDE.md` §6 amended with the `11` §2 wording.
- `09` §2: `web-time` (wasm32 only, `pemba-core`) and `wasm-bindgen` (playground crate
  only), plus the npm toolchain list; `09` §1 layout gains `playground/`.

### T6.1 wasm32 portability of the engine crates
Refs: `11` §1.1–§1.2, §2 item 1, §5.
AC:
- `pemba-core` takes `Instant` from `web-time` on wasm32 (or the D2 alternative); native
  code paths unchanged (`std::time` re-export).
- `ci.yml` `wasm` job: `cargo clippy --target wasm32-unknown-unknown` on `pemba-core`,
  `pemba-schema`, `pemba-store-memory`, `pemba-fixtures` with `-D warnings`.
- A wasm32 smoke test (`wasm-bindgen-test` under Node) runs one check, one
  `check_with_trace` and one lookup, so a trap such as `Instant::now` fails CI, not
  just a build.

### T6.2 `pemba-playground` crate (the WASM API)
Refs: `11` §3, §3.1.
AC:
- `playground/wasm`, workspace member, `publish = false`, `#![forbid(unsafe_code)]`:
  `compile`, `run`, `run_assertions`, `parse_fixture` as JSON in/out over the library,
  no semantics of its own; every engine error carries its `Error::code()`.
- Diagnostic and relationship-line spans in UTF-16 offsets, tested with astral
  characters.
- `pemba-fixtures` gains a public parse-only entry point returning the schema,
  relationships and assertion sections (`07` §3 updated); `run_assertions` is
  `pemba test` semantics exactly.
- Native unit tests for every function, including fail-closed error paths.

### T6.3 Conformance: WASM transcript equals native transcript
Refs: `11` §6 layers 1–2, §5.
AC:
- `playground/conformance` writes the transcript (`11` §6) for a corpus, natively and
  inside the release `.wasm` under Node; CI diffs them byte for byte.
- Corpus: `testdata/fixtures/**` plus 1 000 `pemba-testkit` generated cases (cyclic
  data included), which also run through the testkit runner against the oracle.
- New `playground.yml` (PRs touching `crates/`, `playground/`, `testdata/`) runs it;
  a deliberately broken WASM answer (local test only) makes it fail.

### T6.4 Frontend skeleton
Refs: `11` §3, §4 (stack), §5.
AC:
- `playground/web`: TypeScript strict, Vite, CodeMirror 6; three editors, the query bar
  (check / lookup resources / lookup subjects), results with revision; engine in a Web
  Worker with the `11` §3 limits and 5 s timeout; errors shown with code, never as
  denied.
- `playground.yml` builds the site, enforces the 700 KB gzip budget, runs Vitest and a
  Playwright smoke test (load, run a check, see the answer); Dependabot for npm;
  `npm audit --omit=dev`.

### T6.5 Schema editing: highlighting and inline diagnostics
Refs: `11` §4; `03` §7.
AC:
- `StreamLanguage` mode for the DSL; errors and warnings inline with code, message,
  `help`; relationship lines that do not parse or do not fit the schema marked.
- Playwright: each `testdata/fixtures/schema_errors` case shows its expected code at its
  span.

### T6.6 Assertions editor
Refs: `11` §4; `07` §3, §3.2.
AC:
- Assertions YAML assembled with schema and relationships into a fixture and run
  through `run_assertions`; per-line pass/fail with the runner's message; unknown keys
  are failures, as in `pemba test`.
- Playwright: every corpus fixture's assertions pass in the page (`11` §6 layer 3).

### T6.7 Explain tree
Refs: `11` §4; `04` §3.8; `06` §3.1.
AC:
- Collapsible tree from the trace JSON: result per node, matched tuple on direct hits,
  memo hits link to the full node, cycle pass counts, `truncated` banner; per-node
  durations hidden (`11` §1.2); "copy as text" gives `CheckTrace::render`.
- Vitest: for the four T4.1 snapshot fixtures the tree's text content matches the
  `render()` snapshot node for node.

### T6.8 Examples, shareable links, YAML import/export
Refs: `11` §4.
AC:
- Example menu: `google_docs`, `github_like`, and a new `testdata/fixtures/agent_authz.yaml`
  mirroring `examples/agent_authz.py` (run by `cargo test` like every fixture).
- Share links: deflate-raw + base64url state in the fragment, always carrying the pemba
  version (`11` §4); Vitest round-trip; a malformed fragment shows an error, never
  partial state.
- Version mismatch banner naming both versions, linking to `/v/<link version>/` with the
  same fragment when that release exists, and saying the answers may differ when it
  does not; never switches version on its own. Playwright: a link stamped with another
  version shows the banner with the right target.
- Import/export: every corpus fixture survives export → import → run unchanged.

### T6.9 Deploy and document
Refs: `11` §5.
AC:
- The tag workflow creates the GitHub Release itself, or updates it if it exists. The
  release notes are the version's `CHANGELOG.md` section, and the release is marked
  pre-release. Only then does it attach `playground-<version>.tar.gz`. No workflow
  creates releases today.
- Pages deploy per D3 and `11` §5.1: each `v*` tag attaches `playground-<version>.tar.gz`
  (built with `base` `/pemba/v/<version>/`) to its GitHub Release; every deploy (tag or
  push to `main`) assembles the root (newest release), `/v/<version>/` for every release
  asset, and `/next/` (main); footer shows version and commit.
- A workflow test: the assembled site has a `/v/<version>/` for every playground release
  asset, and the root's footer version equals the newest release.
- `09` §4 lists the new jobs; `09` §4.1 release checklist gains the Pages check (root and
  `/v/<version>/` both load).
- README: a "Try it in the browser" link; "Debugging permissions" links the playground.

---

## M7 — Stable 0.1.0 and public launch

Goal: satisfy T3.7, ship the stable Rust and Python packages, and give new users a
verified path from installation to transactional permission writes. This is release
preparation, not a new evaluator, storage design or supported WASM distribution.
Refs use the numbered specifications above; `CLAUDE.md` golden rules apply throughout.

### Baseline inspected for T7.0 (2026-10-29)

- Planning starts at main `10ad07a`, after release PR #78. Tags alpha.1–alpha.4 exist;
  GitHub Releases list alpha.1, alpha.3 and alpha.4 (a tag alone is not publication
  evidence). Alpha.4's wheel/tag runs succeeded; there is no stable release yet.
- ADR-012 and OQ-001 already chose pemba and completed the rename. Do not rename again.
- Alpha.1 delivered SQLite foreign writes, Python bindings and packaging; alpha.2 added
  traces and `EngineConfig` setters; alpha.3 delivered Postgres and `PendingRevision`;
  alpha.4 delivered the playground and removed Python's deprecated integer conversions.
  Use `CHANGELOG.md` as migration history, not the old pre-alpha examples.
- README already has every topic named by T3.7, plus Postgres, limits and credits.
  `test_readme_example.py` runs its Python example and checks it against `06` §2.1;
  `test_examples.py` exercises `examples/agent_authz.py`. Improve and verify these;
  do not recreate their tests or rewrite guides merely to satisfy a checklist.
- `ci.yml`, `wheels.yml`, `difftest.yml`, `proptest.yml` and `playground.yml` already
  provide package dry runs, licence-copy checks, platform wheels, TLS/SQLite probes,
  property/differential tests and versioned Pages deployment. Extend only proven gaps.
  Docs-only PRs intentionally skip heavy CI jobs; skipped jobs are not release evidence.
- T3.9 has not been completed: request structs still use literals and OQ-044 is open.
  T7.2 below completes T3.9 once; it is not a second independent audit.
- No documentation-site source, CONTRIBUTING, SECURITY, issue templates or public roadmap
  exists on this main. PR #79 separately proposes Pages deployment fixes; inspect its
  eventual disposition before T7.6/T7.11, and do not copy or assume unmerged changes.

### Ordering, evidence and approval boundaries

Execute **T7.0 → T7.17 strictly in order**, one reviewable task/PR at a time. Each task
requires the preceding task's acceptance criteria; no task authorises later actions.
T7.1 and T7.14–T7.15/T7.17 are explicitly human actions. An agent may prepare evidence
but must not approve, merge, tag, upload packages or post launch announcements for them.
T7.16 is read-only verification after publication. Implementation PRs end with the local
green checks in `CLAUDE.md` §5; use relevant CI/service jobs for platform-only checks and
record failures or unavailable environments without calling them passes.

Use `docs/RELEASE-0.1.0.md` (created in T7.2) as the release evidence index: requirement,
source reference, completed task/PR, command or workflow URL, exact commit, result and
human approval where required. Keep credentials and generated build output out of Git.
Any change after final verification invalidates affected evidence and requires a rerun
before approval; a new release candidate needs a new approval. No unresolved blocker,
missing required job, skipped backend suite or unexplained regression is a waiver.

### T7.0 Plan M7 (documentation only; this task)
Refs: T3.7, T3.9; `09` §4–§6; `CLAUDE.md` §2, §8.
AC:
- Record the inspected implementation/release baseline, ordered tasks, acceptance criteria,
  spec references, verification requirements and separate human actions in this milestone.
- Record genuine decisions in OPEN_QUESTIONS; retain existing resolved decisions.
- Check task numbering, dependencies, referenced files/sections, Markdown links and whitespace;
  run the repository's documentation checks and report any local-green limitations.
- Commit on `t7.0-plan`, push and open a PR; stop. Do not start T7.1.

### T7.1 Confirm release scope and publication decisions (human only)
Refs: T3.9; OQ-044, OQ-058–OQ-060; ADR-003, ADR-014, ADR-016; `10` §2.4.
AC:
- Maintainer records whether the already shipped sync Postgres scope is sufficient for
  0.1.0, leaving T5.13 async adapters for later, or explicitly revises this ordering.
- Resolve OQ-044's helper choice and approve the public-surface audit approach before
  changing APIs. No convenience helpers are silently added or deleted by this plan.
- Choose docs-site generator, location and stable/development content policy (OQ-059),
  preserving the existing playground routes unless an ADR change is explicitly approved.
- Choose a private security-reporting route, responsible maintainer and support/triage
  commitments (OQ-060). Record decisions, not credentials or invented service guarantees.

Maintainer decisions approved on 2026-10-29 and recorded in OQ-044 and OQ-058–OQ-060:
sync PostgreSQL is sufficient for 0.1.0; T5.13 follows stable release; only `check_str`
is to be implemented in T7.2 under the approved T3.9 audit approach. VitePress uses
`docs/site/`, `/pemba/docs/` for latest stable and `/pemba/docs/next/` for labelled
development content in the existing Pages deployment, preserving playground routes.
GitHub private vulnerability reporting requires human activation/verification; the
repository owner handles security and triage under the approved best-effort policy.
This records approval only; later implementation and activation gates remain in force.

### T7.2 Complete the existing T3.9 public API audit
Refs: T3.9 (all AC); `06` §1–§2; `09` §3; OQ-044.
Files: published crates' public types; `docs/06-api.md`, `CHANGELOG.md`,
`docs/PUBLIC_API_AUDIT.md`, `docs/RELEASE-0.1.0.md` (new evidence index).
AC:
- Inventory every public struct/enum in every publishable crate, including facade
  re-exports and Postgres types; list exhaustive/non-exhaustive decision and reason for
  each. Reuse existing `EngineConfig` constructors/setters rather than reworking them.
- Apply the approved decisions and OQ-044 resolution with spec and migration entries;
  keep request construction usable from a downstream crate. Add focused downstream
  compilation/doctest coverage for changed construction and helper behaviour.
- Update affected stubs/examples and exception/parse-error tests if the chosen helpers
  change them. No on-disk format change. Record T3.9 satisfied in the evidence index.

### T7.3 Close public API documentation gaps
Refs: T3.7 rustdoc/docstrings AC; `06` §1–§3; `09` §3–§4.
Files: public Rust doc comments, `crates/pemba-py/src/`, Python stubs/docstrings.
AC:
- Audit public items and Python methods against `06` and the T7.2 inventory; fix missing
  docs, broken links and stale signatures, documenting error, limit and transaction
  behaviour. Verify Python help/docstrings on the built extension, not stubs alone.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features` and
  workspace doctests pass; docs.rs metadata builds the intended features. A downstream
  Rust quick start compiles with published-facing imports and explicit feature choices.
- Reconcile the removed alpha.4 integer conversions wherever examples still use them;
  spec, docstrings and stubs agree. Record audited coverage, not only warning counts.

### T7.4 Verify installation, quick starts and user guides
Refs: T3.7 README AC; ADR-006, ADR-014–ADR-015; `05` §4.3–§4.4, §10;
`06` §2.1, §2.5; `10` §1.6.4, §1.6.7–§1.6.8, §2–§3.
Files: `README.md`, mirrored `crates/pemba-py/README.md`, `docs/guides/` (new).
AC:
- Retain the concise README and link deeper install, schema, consistency, transaction
  and troubleshooting guides. Cover Python ≥3.11, SQLite ≥3.26.0, pinned Rust toolchain,
  Postgres ≥15, wheel platforms, source-build prerequisites and CLI installation.
  Any support-floor change requires its own explicit decision/spec update.
- Run Python and Rust quick starts in clean directories/environments. Explain features,
  migrations, caller-owned commit/rollback, reads on engine connections, `PendingRevision`,
  no read-your-uncommitted-write promise, and same-database/one-SQLite-copy restrictions.
- Cover supported DSL and known differences, fail-closed errors vs denial, bounds,
  GC/historical schema caveats, sync/async integration limits and when to use SpiceDB.
  Postgres instructions include privileges, TLS, PgBouncer, 2PC, early constraints and
  SERIALIZABLE retries; do not promise capabilities deferred in T7.1.
- Existing verbatim README/spec test remains green; mirrored README is byte-identical.
  Add checks only for newly executable guide snippets; stable install text switches in T7.12.

### T7.5 Add a focused transactional-write tutorial and regression examples
Refs: ADR-006, ADR-008, ADR-015; `05` §10; `06` §1, §2.1–§2.3;
`07` §3, §6; `10` §1.6.4, §2.3, §4.1.
Files: `examples/`, `docs/guides/`, existing Rust integration/Python example tests.
AC:
- Add runnable app-row + permission examples in Rust and Python for SQLite and Postgres,
  reusing current foreign-write APIs and test helpers. Each demonstrates successful
  commit and forced rollback: both app row and relationship persist, or neither does.
- Assert pending-before-commit behaviour as specified per store, resolution after commit,
  read-after-write with `AtLeast`, and denial/error distinction. Reuse existing rollback,
  mismatched-database, savepoint and driver guard tests; fill only uncovered tutorial paths.
- Tests execute examples against temporary databases (Postgres service required, never
  silently skipped in release runs). No live credentials in examples or snippets.
- Link existing agent, document-sharing and GitHub-like examples/fixtures and explain the
  playground's memory-store limit: it cannot demonstrate a real database transaction.

### T7.6 Build and integrate the documentation site
Refs: T3.7; OQ-059 resolution; ADR-016; `11` §5–§5.1; `09` §4.
Files: `docs/site/` (site source/config), selected generator lock/config files,
`.github/workflows/playground.yml`, existing `playground/web/scripts/check-site.mjs`,
`docs/09-repo-and-tooling.md`, `docs/11-playground.md` if the approved layout needs it.
AC:
- Implement only the T7.1-approved generator/content policy; justify every new dependency.
  Reuse T7.4–T7.5 guides as source; navigation exposes install, quick starts, schema,
  transactions, consistency, limits, API links and troubleshooting. Add contributor, security
  and roadmap navigation only when their content lands in T7.7–T7.9; no dead links.
- Build with warnings/broken local links as failures, correct project base paths, readable
  narrow-screen navigation and keyboard access. Show which release the content describes
  and distinguish development content under the approved policy.
- Integrate with the existing single Pages assembly/deployer so deploying docs preserves
  root playground, `/next/`, immutable `/v/<version>/` assets and shared fragments.
  Do not create competing whole-site deployments. Inspect PR #79's merged state first.
- Extend existing site/release tests to prove docs routes/assets and old playground routes
  coexist, including stable vs prerelease selection and a deploy from an older tag.
  Reuse stable-release classification from OQ-057; do not implement it again.

### T7.7 Document and exercise developer onboarding
Refs: `CLAUDE.md` §2–§6; `07` §1–§6; `09` §1–§5.
Files: `CONTRIBUTING.md` (new), docs-site contributor navigation, existing CI as needed.
AC:
- Document clean checkout, pinned Rust/Node/Python prerequisites, isolated test venv,
  workspace layout, fixture/regression workflow, focused checks and full local-green
  commands, separate fuzz workspace and service-backed test environment variables.
- Distinguish required pre-PR checks from long release checks; include one-task branches,
  spec/API-change rules, dependency justification and original-code/attribution rules.
- Follow the guide from a clean environment to compile Rust, import Python, run one
  fixture, build the docs and run focused tests. Record commands/results; reuse CI setup
  rather than introducing a second conflicting toolchain or hidden global dependencies.

### T7.8 Establish security reporting and review release safeguards
Refs: `CLAUDE.md` §2; ADR-008, ADR-011; `07` §5; `09` §4;
`10` §3.3; OQ-060 resolution.
Files: `SECURITY.md` (new), docs-site security navigation, `docs/RELEASE-0.1.0.md`,
workflows only for identified gaps.
AC:
- Publish the approved private reporting route, supported-version policy and maintainer
  handling process; explain public issues must not contain exploit details or secrets.
  Human enables any account/repository security feature; instructions alone are not proof.
- Review token permissions, fork behaviour, trusted publishing, required `pypi` reviewer,
  secret handling, package ownership/access and licence/NOTICE copies. Record findings
  and remediation; no credentials or automated publishing added to normal PR runs.
- Run `cargo deny check` and the shipping npm audit; review fail-closed/limit tests,
  SQLite-symbol and TLS probes and privileged vs normal-role Postgres paths. Carry genuine
  release-blocking findings to OPEN_QUESTIONS and require regression tests for fixes.

### T7.9 Prepare issue triage and a public roadmap
Refs: M8–M10 below; `CLAUDE.md` §2, §6; `09` §5–§6; OQ-060 resolution.
Files: `.github/ISSUE_TEMPLATE/`, `docs/TRIAGE.md`, `ROADMAP.md` (new), site links.
AC:
- Provide bug and feature templates requesting version, store/platform, minimal schema/
  tuples, expected/actual result and safe reproduction; point vulnerabilities to SECURITY.
- Document approved labels, severity/priority, maintainer ownership, duplicate handling,
  regression-fixture follow-up and how to promote a report to a spec/task. Label setup
  and any response commitments require the human action recorded in T7.17.
- Roadmap distinguishes shipped alpha/stable capabilities, M7 remaining gates and M8/M9/
  M10 candidates; clearly place T5.13 per T7.1. No promised dates or scope expansion.
  Link the known Postgres round-trip follow-up and known differences/limitations.

### T7.10 Draft launch materials (no posting)
Refs: `00` §1–§4, §7; ADR-006, ADR-011, ADR-015; T3.7;
`10` §1.6.4; `benches/RESULTS.md`.
Files: `docs/launch/0.1.0.md` (new draft and channel checklist).
AC:
- Draft release summary, long-form post and short announcement variants centred on app
  data and permissions committing/rolling back together; use the tested T7.5 tutorial.
- Include installation, docs, playground, API, migration, issues and roadmap links,
  supported scope/limitations, credits and when a separate service is preferable.
- Explain PendingRevision and caller commit ownership accurately; no unsupported async,
  WASM distribution, wire compatibility or unqualified performance/security claims.
  Any benchmark claim links measured version/hardware/context, not a blanket comparison.
- Mark destinations, timing and final text as requiring human approval; drafts stay local
  to the repository and do not contact third parties or create public announcements.

### T7.11 Rehearse release verification and close regression gaps
Refs: T3.7; T3.4–T3.6; `07` §2–§7; `09` §4–§4.1; `10` §4; `11` §6.
Files: `docs/RELEASE-0.1.0.md`, test/workflow files only for demonstrated gaps.
AC:
- Record exact-commit full local-green results, package dry run and manual build-only
  wheel run. Require all five wheel platforms on CPython 3.11/3.14, sdist rebuild away
  from checkout, SQLite-symbol/TLS checks, oldest SQLite/Python job and Postgres 15/18
  tests including every supported Python driver. No upload/tag in rehearsal.
- Run properties at ≥1,000 cases, store conformance/equivalence and Postgres revision-order
  tests (including isolation levels, constraints and retries). Run memory and Postgres
  difftests with required-service flags, fixtures and ≥500 compared generated cases each;
  review known-difference detectors and save any new failing case as a regression fixture.
- Run all three fuzz targets ≥10 minutes each with no crashes, retain useful corpus,
  and review surviving evaluator/validator mutants against `docs/MUTANTS.md`. Existing
  reports are baseline evidence, not proof that the candidate passed a new run.
- Run docs-site link/snippet checks and playground conformance, unit, size and browser
  checks from `09` §4.1. Review benchmark changes against the recorded baseline; explain
  measured regressions rather than inventing a new release threshold.
- Check all OPEN_QUESTIONS entries semantically: resolved historical `Blocking: yes`
  text is not an open blocker; unresolved gate dependencies such as OQ-044 must be closed.
  Record T3.7 coverage row by row. Fix discovered bugs in separately reviewed preceding
  work and rerun affected evidence before advancing, preserving the task order.

### T7.12 Prepare the stable release PR (no publishing)
Refs: `09` §4.1; ADR-014; T3.7; OQ-057; `CHANGELOG.md` release history.
Files: root `Cargo.toml`, root and fuzz `Cargo.lock`, `CHANGELOG.md`, both READMEs,
package metadata and release evidence index.
AC:
- Apply the existing checklist for `0.1.0` (Rust and Python), including all internal
  dependency versions and both lockfiles; only own crate versions change in lockfiles.
  Do not bump versions in T7.0 or recreate the packaging pipeline.
- Derive publishable crates and dependency order from current manifests (including
  Postgres), not OQ-035's historical pre-Postgres list. Keep testkit/difftest, Python-only
  crates and playground crates unpublished on crates.io; check package contents, docs.rs
  metadata, licences, README, `.pyi`/`py.typed` and sdist dependencies.
- Changelog covers alpha-to-stable API migrations, notably T7.2 and PendingRevision;
  change alpha-only stability language to the approved stable contract (`06` header),
  update install/status links and documentation metadata consistently.
- Existing release script extracts stable notes and classifies `0.1.0` as **not** a
  prerelease; test both stable and alpha/rc cases. Record the human publishing sequence,
  partial-failure recovery and immutable-version policy; never overwrite a release or
  roll back an existing tag to repair it. All release PR checks must pass before approval.

### T7.13 Verify the exact stable release candidate
Refs: T3.7; `09` §4.1; T7.11 verification matrix.
Files: release evidence index (candidate SHA and linked artifacts/results).
AC:
- Repeat T7.11's release matrix on the final T7.12 candidate, including build-only wheel
  workflow, dry runs (`cargo publish --workspace --dry-run --locked`), docs and playground.
  A stable package version must be tested; passing alpha builds are insufficient.
- Install built wheels and build the sdist in isolated environments outside the source
  tree; run the transactional tutorial and check version, API/stubs, imports and CLI.
  Verify packaged crates build without local path-only dependencies.
- Evidence ties every required job/artifact to the candidate SHA and records all T3.7 AC,
  T3.9 inventory, completed rename and absence of unresolved blocking questions. No
  publication side effect is needed for any acceptance criterion in this task.

### T7.14 Sign off T3.7 and the release candidate (human only)
Refs: T3.7 human gate; `09` §4.1; T7.13 evidence.
AC:
- Maintainer reviews every T3.7 row, security findings, known differences, API audit,
  documentation and migration guide, confirms no open blocking question and explicitly
  approves the exact candidate SHA, release text and supported scope.
- Merge the release PR only with current green required checks. Verify the intended main
  tree matches approved content; any substantive change returns to T7.13 and new sign-off.
  Record approval and release commit before tagging. CI green alone is not human sign-off.

### T7.15 Tag and publish 0.1.0 (human only)
Refs: `09` §4.1 steps 6–7; ADR-014, ADR-016; OQ-057.
AC:
- Human tags `v0.1.0` on the approved main release commit and publishes crates in the
  recorded dependency order, waiting for registry availability before dependent uploads.
- Human approves the protected `pypi` environment after tag wheel/sdist smoke tests;
  trusted publishing uploads those artifacts. Confirm tag workflows create a full GitHub
  Release with approved changelog notes and immutable playground archive and deploy Pages.
- Record registry versions, tag SHA and workflow/release URLs. On partial failure, stop
  launch and follow the reviewed recovery procedure; do not retag or overwrite artifacts.

### T7.16 Verify public artifacts and live documentation (read-only)
Refs: `09` §4.1 step 7; `11` §5.1; T7.4–T7.6 and T7.13.
AC:
- From fresh environments outside the repository, install `pemba==0.1.0` from PyPI and
  the Rust/CLI 0.1.0 packages from crates.io; run quick starts and transactional commit/
  rollback smoke examples. Confirm installed versions/paths and docs.rs build success.
- Check the approved docs URL, release notes and navigation, playground root and
  `/v/0.1.0/`, an existing alpha version/share link and `/next/`; version/footer, assets
  and development labels must agree. Verify registry metadata and public links resolve.
- Record results in the release evidence index (documentation PR if needed). A failure
  blocks T7.17 and goes through the approved recovery path; publication is not completion.

### T7.17 Launch and activate community processes (human only)
Refs: T7.9–T7.10 drafts; T7.14 approval; T7.16 public verification.
AC:
- Human approves final announcement text, destinations and timing, then posts the launch
  materials with verified stable links. Record public URLs; do not promise unapproved work.
- Human enables/verifies the chosen private reporting route, labels and triage ownership,
  publishes the roadmap links, and triages initial reports through the documented process.
- Release evidence records final launch approval, package/docs verification and follow-up
  issues. M7 is complete only after these actions; no M8/T5.13 work starts automatically.

---

## M8–M10 — Later milestones (outline)

Outlines only. Detailed tasks and acceptance criteria get written when each milestone
starts, against the spec as it stands then.

### M8 — Authorised queries
- Filter application queries by permission inside the database (SQL predicates or a
  join against `lookup_resources`) instead of filtering rows in application code.
- Must agree with `lookup_resources` under the reference evaluator; SQLite first.
- Pagination and ordering that stay correct at a fixed revision.

### M9 — SpiceDB import/export
- Convert schemas and relationships between pemba and SpiceDB (`zed` backup /
  `.zed` files), rejecting unsupported features with `E_UNSUPPORTED_FEATURE`.
- A migration guide in both directions; round-trip tests through the difftest harness.
- Original code only (ADR-011): formats from SpiceDB docs, not its source.

### M10 — User-driven features
- Chosen from issues and launch feedback, not planned ahead, with one exception.
- **Top post-release performance item: fewer round trips per check on deep hierarchies**
  (batched arrow reads per level, or a recursive query). On Postgres a check's cost is
  its round trips (OQ-056). With the statement cache and bulk writes, a direct check is
  5 round trips (0.76 ms p50 on a Linux CI runner where one round trip took about
  146 µs), but each level of hierarchy walked adds about 3: `folders_depth_30` is 99
  round trips and 16.2 ms, `groups_depth_10` 25 and 4.1 ms, `lookup_resources_10k` 506
  and 88 ms (`benches/RESULTS.md`). Before the cache the same checks were 8 and 196 round
  trips. Batching each level's arrow reads into one statement would make a check about
  one round trip per level; a recursive CTE could make it a constant. Either must agree
  with the reference evaluator (golden rule 1) and keep reads at one snapshot.
- Candidates carried over from the old stretch list: FastAPI helper (`06` §2.4), Node
  bindings (napi-rs) and a WASM build; anything on the
  `CLAUDE.md` §6 out-of-scope list still needs an explicit decision first.
