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
- Identifier rules (`02` §1.1) are implemented once, in `warden-schema`, and reused by
  `warden-core`.
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

### T0.5 Pretty printer and `warden schema validate|fmt`
Refs: `03` §9, `06` §3.
AC:
- Canonical printing; round-trip property test.
- `CompiledSchema::hash` (sha-256 of the normalised source, `03` §6), deferred from T0.4
  because it depends on the normaliser.
- `warden-cli` binary with `schema validate` and `schema fmt [--check]`; correct exit codes.

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
- Reference evaluator independent of `warden-core::eval`.
- Generators per §2.2; properties 1 and 6 from §2.3 passing at ≥ 1 000 cases.

### T1.4c Bound check cost on cyclic data
Refs: `04` §3.6, `06` §1, OQ-021. After T1.4, before T1.4b.
AC:
- Taint is lowlink-style: each result carries the lowest stack index of an in-progress node
  it assumed `false`.
- A node that finishes as its own cycle head (lowlink not below its own stack index) is
  memoised.
- Tainted `true` results are memoised (`04` §3.6 allows this with property tests).
- New per-request cap on total store reads, `EngineConfig::max_check_reads`; exceeding it
  returns a new error (fail closed), documented in `06`.
- The OQ-021 test (`check_on_dense_cyclic_data_reads_a_bounded_number_of_tuples`) passes
  with its `#[ignore]` removed.
- Property 1 at ≥ 1 000 cases passes in under 60 s optimised, on several seeds.

### T1.4b Stratified negation for cycles
Refs: `04` §3.6 (planned rule), OQ-018. After T1.4c (needs lowlink-style taint).
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
- Runner + `warden test` CLI command.
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
- Write logic lives in `warden-core::write`, generic over `WriteTxn` + `SqlDialect`.
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
- `E_SCHEMA_CHANGE_ORPHANS` with per-relation counts; permission removal always allowed.
- A test proves stale relationships (OQ-019 (e) and the two remaining cases) cannot arise
  after `write_schema`.
- Multi-process schema reload test (two engines on one SQLite file).
- `delete_relationships` with limit; `gc` in batches; `RevisionExpired` after GC.

---

## M3 — Python, verification, release

### T3.1 `warden-py` bindings
Refs: `06` §2.
AC:
- All signatures in `06` §2.2 with `.pyi` stubs; `mypy --strict` passes on a usage sample.
- GIL released for engine calls; exception hierarchy mapped by `Error::code()`.
- pytest suite per `07` §6 (except foreign txn) passes.

### T3.2 Foreign transaction mode (headline feature)
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

### T3.4 Differential testing against SpiceDB
Refs: `07` §4.
AC:
- `warden-difftest` harness; all fixtures + ≥ 500 generated cases agree with SpiceDB, or each
  divergence is listed in `KNOWN_DIFFERENCES.md` with a reason and an ADR update if semantics
  changed (e.g. `.all` over zero objects, self membership, arrow over wildcard).
- CI job (manual trigger + weekly) running it in Docker.

### T3.5 Benchmarks
Refs: `07` §7.
AC:
- Criterion + pytest-benchmark scenarios implemented; results table generated by script into
  `benches/RESULTS.md` with hardware/version metadata.
- SpiceDB comparison scripts runnable end-to-end.

### T3.6 Fuzzing and hardening
Refs: `07` §5.
AC:
- Three fuzz targets build and run ≥ 10 minutes each with no crashes; corpus committed.
- `cargo-mutants` on `eval` and schema validator: surviving mutants reviewed and either killed
  by new tests or justified in `docs/MUTANTS.md`.

### T3.8 Comment-preserving formatter (print from the AST with comment trivia)
Refs: `03` §9, `06` §3, OQ-013. Lands before the T3.7 gate.
AC:
- Plain comments (`//` and `/* */`) survive `warden schema fmt`.
- `warden schema fmt --check` passes on canonical files containing comments.

### T3.7 Documentation and 0.1 gate
AC (release gate — human signs off):
- README: pitch, install, quick start (Python + Rust), the transactional-write example,
  schema guide (supported subset + differences from SpiceDB), consistency guide, limits,
  "when to use SpiceDB instead", credits.
- `docs.rs`-ready rustdoc on all public items; Python docstrings on all public methods.
- All CI jobs green, differential tests green, no open `OPEN_QUESTIONS.md` items marked
  `blocking`.
- Final name chosen (ADR-012) and rename completed.

### Stretch (only after the gate, in this order)
1. `check_with_trace` (`04` §3.8) + `warden check --explain`.
2. FastAPI helper (`06` §2.4).
3. Batched arrow reads (`04` §4).
4. Postgres store (requires resolving ADR-003/ADR-005 open questions).
5. Node bindings (napi-rs), WASM build.
