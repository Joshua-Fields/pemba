# 07 — Testing and benchmarking

Authorisation correctness is the product. The test strategy is layered so that every
optimised code path is checked against something simpler.

| Layer | Tool | Where |
|---|---|---|
| Unit tests | `cargo test` | next to code |
| Snapshot tests (diagnostics, IR, pretty-print) | `insta` | `warden-schema` |
| Fixture scenarios (YAML) | `warden-testkit` runner | `testdata/fixtures/` |
| Reference-evaluator property tests | `proptest` | `warden-testkit`, `warden-core` |
| Store conformance | generic suite | `warden-testkit::store_conformance` |
| Differential vs SpiceDB | Docker + HTTP | `warden-difftest` (`#[ignore]`, CI job) |
| Fuzzing | `cargo-fuzz` | `fuzz/` |
| Mutation testing | `cargo-mutants` (eval + schema validator) | CI weekly |
| Python tests | `pytest` | `crates/warden-py/tests/` |
| Benchmarks | `criterion`, `pytest-benchmark` | `benches/` |

Coverage target (`cargo llvm-cov`): ≥ 90% lines for `warden-schema` and `warden-core::eval`.

---

## 1. Unit test expectations per module

- **Tuple parser**: every row of `docs/02` §3 table; each invalid-identifier example in
  `docs/02` §1; round-trip property `parse(display(x)) == x`.
- **Lexer/parser**: every grammar production; every error code in `docs/03` §5 has at least one
  failing example with a snapshot of the rendered diagnostic; every `E_UNSUPPORTED_FEATURE`
  case in `docs/03` §7.
- **Mixing rule**: `a + b & c` rejected; both parenthesisations accepted and produce
  different IR.
- **SubjectSet algebra**: all 12 cells of `docs/04` §6.1 plus a property test against a
  model over a finite universe `{0..8}` with explicit complement.

---

## 2. Reference evaluator (the oracle)

`warden-testkit::reference` — **deliberately naive, written for obviousness, not speed.**
It must not share evaluation code with `warden-core::eval` (it may share the schema IR types
and parsers).

### 2.1 Algorithm (Kleene iteration)

1. Universe `U[t]` for each type `t` = every object id of type `t` appearing anywhere in the
   tuples **plus one fresh id** `__fresh__` (to test wildcard semantics for never-seen subjects).
2. For every object `o` and member `m`, maintain `S(o, m)` as an explicit finite set of
   subjects (object subjects as `(type,id)`; subject sets as `(type,id,rel)`), initially ∅,
   plus self membership (`docs/04` §2).
3. Repeat: recompute every `S(o, m)` from the semantics table in `docs/04` §2 using the
   current sets (wildcards expand to `U[t]`), until nothing changes.
4. `check(o, m, s) = s ∈ S(o, m)`.

Iteration converges for monotone schemas. The property-test **generators must not put
recursive members under the `sub` side of an exclusion** (exclusion `sub` = a non-recursive
relation with only plain object subjects). Non-monotone cycles are covered by hand-written
tests expecting `NonMonotonicCycle`.

### 2.2 Generators (`proptest` strategies)

- Schemas: 1–4 types, 1–4 relations and 0–3 permissions per type; allowed types drawn from
  object / subject-set / wildcard; expressions of depth ≤ 3 over union, intersection,
  exclusion (restricted as above), arrows (`Any`, `All`); every generated schema must pass
  validation (filter or construct-valid).
- Data: 0–40 tuples valid for the schema, ids from a small alphabet (`a`..`f`) to force
  sharing and cycles.
- Queries: random `(object, member, subject)` from the universe, including fresh ids and
  subject sets.

### 2.3 Properties (each 1 000+ cases in CI, `PROPTEST_CASES` env to raise locally)

1. `engine.check == reference.check` for memory store and SQLite store.
2. `engine.lookup_resources` (all pages concatenated) == `{ o : reference.check(o, m, s) }`.
3. `engine.lookup_subjects` agrees with reference for every subject in `U[t]` (`docs/04` §6.3).
4. After random interleaved writes/deletes, `check` at `Exact(r)` equals the reference run on
   the tuple set as of revision `r` (MVCC correctness).
5. Memory store and SQLite store return identical results for identical op sequences.
6. Schema pretty-print round-trip: `compile(print(ir)) == ir`.

Failing cases are shrunk by proptest; persist regressions in `proptest-regressions/`
(committed).

---

## 3. Fixture files

Mirror SpiceDB validation-file keys so files are familiar and reusable with `zed validate`
(for the subset we support).

```yaml
# testdata/fixtures/docs_basic.yaml
schema: |-
  definition user {}
  definition document {
      relation viewer: user | user:*
      relation owner: user
      permission view = viewer + owner
  }
# or: schemaFile: "./docs_basic.zed"

relationships: |-
  document:readme#viewer@user:anne
  document:readme#owner@user:carol
  document:public#viewer@user:*

assertions:
  assertTrue:
    - document:readme#view@user:anne
    - document:readme#view@user:carol
    - document:public#view@user:anyone
  assertFalse:
    - document:readme#view@user:bob

# warden extensions (ignored by zed):
expectedLookups:
  resources:
    - query: "document#view@user:anne"
      expect: ["public", "readme"]
  subjects:
    - query: "document:public#view@user"
      expect: { wildcard: true, excluded: [] }
expectErrors:                       # optional: schema or write must fail with these codes
  - E_AMBIGUOUS_OPERATORS
```

- `relationships` is a single multi-line string (as in SpiceDB), one tuple per line; `//`
  comment lines and blank lines allowed.
- SpiceDB's `validation:` (expected relations) and `assertCaveated:` keys are **recognised and
  skipped with a warning** in the MVP.
- Runner: `warden-testkit::fixtures::run(path, store_kind)` and the CLI `warden test`.
  Each fixture runs against **both** memory and SQLite stores.
- Use a maintained YAML crate (check status before choosing; `serde_yaml` is archived).

### 3.1 Required fixture corpus (M1–M2 deliverable)

| File | Covers |
|---|---|
| `direct.yaml` | direct relations, Create/Touch/Delete visibility |
| `wildcard.yaml` | `user:*`, fresh subjects, wildcard not matching subject sets |
| `subject_sets.yaml` | `group#member`, nested groups 3 deep |
| `nested_groups_cycle.yaml` | groups containing each other; check terminates correctly |
| `arrows_folders.yaml` | `parent->view` recursion, depth 10 |
| `arrows_ignore_subject_relation.yaml` | arrow over `group#member` subject walks `group` |
| `intersection.yaml` | `member & viewer` |
| `exclusion_banned.yaml` | `(viewer + edit) - banned` |
| `all_arrow.yaml` | `.all()` with 0, 1, many related objects |
| `any_alias.yaml` | `.any()` identical to `->` |
| `self_membership.yaml` | `group:eng#member` checked against `group:eng#member` |
| `google_docs.yaml` | realistic org/folder/doc/group model, 30+ tuples |
| `github_like.yaml` | org → team → repo with admin/maintain/write/read |
| `multi_tenant.yaml` | tenant isolation: no cross-tenant leakage |
| `prefixed_types.yaml` | `docs/document`, `iam/user` |
| `depth_limit.yaml` | chain of 60 folders → `MaxDepthExceeded` |
| `non_monotone_cycle.yaml` | cycle through exclusion → `NonMonotonicCycle` |
| `schema_errors/*.yaml` | one file per schema error code |

---

## 4. Differential testing against SpiceDB

Crate `warden-difftest` (dev-only, never published). Tests are `#[ignore]` and require
`WARDEN_SPICEDB_URL` + `WARDEN_SPICEDB_KEY`.

Run SpiceDB (from its README):
```bash
docker run --rm -p 50051:50051 -p 8443:8443 authzed/spicedb serve \
  --http-enabled true --grpc-preshared-key "testkey"
```
(Default datastore is in-memory; confirm with `spicedb serve --help`. Alternatively
`spicedb serve-testing` gives an isolated empty datastore per preshared key — useful for
parallel tests, check whether it exposes HTTP.)

Procedure per case (fixtures + proptest-generated cases):
1. `POST /v1/schema/write` with the same schema text.
2. `POST /v1/relationships/write` with the same tuples (`OPERATION_TOUCH`).
3. For each query, `POST /v1/permissions/check` with full consistency (verify the exact
   consistency field name in the HTTP API reference: https://authzed.com/docs/spicedb/api/http-api).
4. Compare with warden. Any mismatch fails with a minimal reproduction printed as a fixture
   YAML ready to paste into `testdata/fixtures/regressions/`.
5. Lookups: compare as sets (SpiceDB streams lookup results; check the HTTP endpoint shape).

Use a fresh preshared key (or wipe) per case for isolation. Use a blocking HTTP client
(`ureq`) to avoid pulling tokio into the workspace.

Known/expected divergences must be listed explicitly in `warden-difftest/KNOWN_DIFFERENCES.md`
with a reason; the harness skips only listed cases.

---

## 5. Fuzzing (`fuzz/`, nightly toolchain)

- `fuzz_schema_parse`: arbitrary bytes → `compile` must never panic.
- `fuzz_tuple_parse`: arbitrary bytes → `Relationship::from_str` must never panic; on success
  round-trips.
- `fuzz_check_vs_reference`: `arbitrary`-derived small schema + data + query → engine vs
  reference (same restrictions as §2.2).
Run each ≥ 10 minutes before a release; corpus committed under `fuzz/corpus/`.

---

## 6. Python tests (`pytest`)

- API surface: every method in `docs/06` §2.2, every exception class mapping.
- Foreign transaction mode: commit makes tuples visible; rollback discards them; revision
  returned before commit is not readable (`ConsistencyError`); mismatched DB file →
  `StorageError`; autocommit guard.
- Threads: 8 threads doing checks while 1 thread writes; no errors, results monotone in
  revision.
- Wheel smoke test in CI on each platform: `pip install <wheel> && python -c "import warden"`.

---

## 7. Benchmarks

`criterion` benches in `benches/` (Rust) and `pytest-benchmark` (Python), memory and SQLite
stores, warm caches, reported as p50/p99 per operation:

| Scenario | Shape |
|---|---|
| `direct_hit` | user directly on document |
| `groups_depth_{1,5,10}` | nested group chain |
| `folders_depth_{1,10,30}` | `parent->view` chain |
| `wide_groups_10k` | user in 10 000 groups, document shared to one |
| `exclusion` | `(viewer + edit) - banned` |
| `lookup_resources_10k` | 10 000 documents, user can see 5% |
| `write_batch_{1,100,1000}` | write throughput |

SpiceDB comparison (`benches/spicedb/`, scripts only): same datasets loaded into a local
SpiceDB, measured over localhost gRPC/HTTP from the same machine. Publish the hardware,
versions, commands and raw results with the numbers. **Never publish a number without its
script.** No performance targets are fixed until the first run (open question).
