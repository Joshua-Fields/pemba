# 07 — Testing and benchmarking

Authorisation correctness is the product. The test strategy is layered so that every
optimised code path is checked against something simpler.

| Layer | Tool | Where |
|---|---|---|
| Unit tests | `cargo test` | next to code |
| Snapshot tests (diagnostics, IR, pretty-print) | `insta` | `pemba-schema` |
| Fixture scenarios (YAML) | `pemba-fixtures` runner + `pemba-testkit` oracle | `testdata/fixtures/` |
| Reference-evaluator property tests | `proptest` | `pemba-testkit`, `pemba-core` |
| Store conformance | generic suite | `pemba-testkit::store_conformance` (memory, SQLite, Python SQLite) |
| Differential vs SpiceDB | Docker + HTTP | `pemba-difftest` (`#[ignore]`, CI job) |
| Fuzzing | `cargo-fuzz` | `fuzz/` |
| Mutation testing | `cargo-mutants` (eval + schema validator) | CI weekly |
| Python tests | `pytest` | `crates/pemba-py/tests/` |
| Benchmarks | `criterion`, `pytest-benchmark` | `benches/` |

Coverage target (`cargo llvm-cov`): ≥ 90% lines for `pemba-schema` and `pemba-core::eval`.

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

`pemba-testkit::reference` — **deliberately naive, written for obviousness, not speed.**
It must not share evaluation code with `pemba-core::eval` (it may share the schema IR types
and parsers).

### 2.1 Algorithm (stratified fixed points, OQ-020)

1. Universe `U[t]` for each type `t` = every object id of type `t` appearing anywhere in the
   tuples **plus one fresh id** `__fresh__` (a valid object id; it stands for every
   never-seen object, to test wildcard semantics). Tuples must not use `__fresh__`, and
   queries must stay inside the universe (two unseen ids are not interchangeable).
2. **Dependency graph** over members `(type, member)`. Edges: a permission to every member
   it computes; an arrow `t->p` to the tupleset `(T, t)` and to `(X, p)` for every type `X`
   allowed on `t` that has `p`; a relation allowing the subject set `x#y` to `(x, y)`. Every
   edge reached through the `sub` side of an exclusion, at any depth (including the base of
   an exclusion nested inside a `sub`), is **negative**.
3. **Strata** = strongly connected components. If a component contains a negative edge, the
   schema is not stratifiable and the oracle returns `ReferenceError::NotStratifiable`
   ("unsupported"), never an answer. The engine still accepts such schemas and decides per
   check from the data (`docs/04` §3.6); hand-written tests cover it.
4. Evaluate components in dependency order. For each, every `S(o, m)` (all `o` in `U[t]`,
   all `(t, m)` in the component) starts at ∅ and is recomputed from the semantics table in
   `docs/04` §2 (wildcards expand to `U[t]`, objects only; `.all` over zero objects is ∅;
   self membership `o#m ∈ S(o, m)`), reading earlier components as fixed, until nothing
   changes. Inside a component every edge is positive, so the sets only grow: this is the
   least fixed point. Monotone recursion inside a `sub` side is just an earlier component.
5. `check(o, m, s) = s ∈ S(o, m)`.

Plain Kleene iteration over all members at once (the previous algorithm) is wrong once a
recursive member sits on the base side of an exclusion: depending on update order it stops
at a non-least fixed point or never stops (OQ-020).

### 2.2 Generators (`proptest` strategies)

- Schemas: 1–4 types, 1–4 relations and 0–3 permissions per type; allowed types drawn from
  object / subject-set / wildcard; expressions of depth ≤ 3 over union, intersection,
  exclusion, arrows (`Any`, `All`); every generated schema must pass validation (filter or
  construct-valid) and be stratifiable (§2.1). Recursion is allowed on the base side of an
  exclusion (OQ-020). Two variants for the `sub` side: the default keeps recursion off it
  (nothing a `sub` side reaches lies on a cycle of the dependency graph); the
  `SubSides::Recursive` variant (T1.4b) allows monotone recursion beneath it, dropping only
  a `sub` side that reaches back to its own permission. Property 1 runs on both.
  Implemented in `pemba-testkit::generate`, construct-valid.
- Data: 0–40 tuples valid for the schema, ids from a small alphabet (`a`..`f`) to force
  sharing and cycles.
- Queries: random `(object, member, subject)` from the universe, including fresh ids and
  subject sets.

### 2.3 Properties (each 1 000+ cases, `PROPTEST_CASES` env to raise locally)

Property 1 runs 64 cases (plus saved regression seeds) in plain `cargo test`; its 1 000+
case variant is `#[ignore]`d and runs, with the other full-count properties, in ci.yml's
`prop1` job on every push to main and every pull request (Linux only, about a minute
since T1.4c bounded check cost, OQ-021). Runs at a raised `PROPTEST_CASES` (default
10 000) are weekly and manual only (`proptest.yml`).

1. `engine.check == reference.check` for memory store and SQLite store.
2. `engine.lookup_resources` (all pages concatenated) == `{ o : reference.check(o, m, s) }`.
3. `engine.lookup_subjects` agrees with reference for every subject in `U[t]` (`docs/04` §6.3).
   Runs on both stores over both generator variants, plus a fixed schema with wildcards
   on both sides of exclusions and intersections under recursion (random data): the
   generated schemas put a wildcard beneath an exclusion in only ~2% of cases, which would
   leave the cofinite cells of `04` §6.1 barely exercised end to end.
4. After random interleaved writes/deletes and GCs (T2.6), `check` at `Exact(r)` equals the
   reference run on the tuple set as of revision `r` for every `r` at or above the GC
   horizon, and fails with `RevisionExpired` below it (MVCC correctness), on both stores.
   Alongside (T2.6): after any `write_schema` the engine accepts, every live tuple fits the
   new schema and `check` at head agrees with the oracle; a refused change reports exactly
   the live tuples it would orphan (`schema_change_leaves_no_stale_tuple_at_head`).
5. Memory store and SQLite store return identical results for identical op sequences.
6. Schema pretty-print round-trip: `compile(print(ir)) == ir`.
7. `check_with_trace` (T4.1, `04` §3.8): on both stores and both generator variants,
   `check_with_trace(..).0.allowed == check(..) == reference.check`, the same error where
   `check` fails, equal stats (bar `elapsed`), the root's result equal to `allowed`, and
   every trace invariant of `04` §3.8 (`pemba_testkit::trace::violations`). A third engine
   with `max_trace_nodes = 6` must give the same answers with truncated traces.
   It also runs on `generate::cyclic_case`, built so cyclic regions re-run their head
   (intersections, arrows and exclusion bases on data cycles; the general generators
   almost never produce one). Its 1 000+ case run prints how many cases re-ran a head
   (17–20% in local runs, up to 6 passes) and fails below 10%.
   `generated_traces_exercise_every_feature` keeps the property from passing vacuously;
   hand-built cases and insta snapshots of rendered traces are in `tests/trace.rs`.

**The one accepted error (T3.4b).** Properties 1, 2, 4 and the schema-change property
treat every engine error as a failure, except `AMBIGUOUS_WILDCARD_EXCLUSION` where the
oracle shows the case applies (`Reference::ambiguous_wildcard_exclusion`). For
`check(o, m, s)` the condition `A(o, m, s)` is: `s` is a subject set `x#r` with `x` of type
`t`, and the evaluation graph has a path from node `o#m` to a relation node `o'#r'` holding
a stored `o'#r'@t:*` that crosses the subtracted side of at least one exclusion. The graph
has an edge from a permission node to every member its expression names on the same object,
and through an arrow to the target member of every object stored on the tupleset. A
relation node has an edge to `x#y` for every stored subject set `x#y`. Edges are followed
whatever the engine would short-circuit, so `A` over-approximates what it reaches:
- error and `A` false → failure;
- error and `A` true → accepted;
- an answer → must equal `reference.check`, whatever `A` is.
For `lookup_resources(T, m, s)` the error is accepted iff `A(o, m, s)` holds for some `o`
of type `T` in the universe (a page fails as a whole, `04` §5.2); otherwise the ids must
match. `generated_ambiguity_is_exercised` keeps this from passing vacuously. On fixed
samples, at least 10 generated checks must fail with the error and at least 50 must answer
where `A` holds; removing the error fails it, as does `wildcard_subject_set.yaml`.

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

# pemba extensions (ignored by zed):
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
  comment lines and blank lines allowed. They are written as one batch of `Touch` ops.
- SpiceDB's `validation:` (expected relations) and `assertCaveated:` keys are **recognised and
  skipped with a warning** in the MVP.
- `expectedLookups.subjects` (T2.5), also after step 0: `query` is
  `type:id#permission@subject_type`; `expect` is a map `{ ids: [...] }` or
  `{ wildcard: true, excluded: [...] }` (keys default to empty / `false`; ids in any order,
  each once; `ids` with `wildcard`, or `excluded` without it, is a failure). The runner
  answers with the engine and with the oracle, whose answer is a wildcard exactly when it
  allows its fresh id (standing in for every object never written), excluding the denied
  ids of its universe.
- `expectedLookups.resources` (T2.4) is checked after step 0 (top level only; steps have no
  lookups yet). `query` is `type#permission@subject`; `expect` lists ids in any order, each
  once. The runner answers each with the engine, concatenating every page at the first
  page's revision, once at the default page size and once at page size 1, and with the
  oracle as `{ o ∈ U[type] : check(o, permission, subject) }` (without its fresh stand-in
  id), and names each that disagrees with the file.
  Any other unknown key is a failure, so a misspelt `assertTure` cannot silently test nothing.
- `expectErrors` lists the codes (`E_...`) the schema compile, or the `relationships` write,
  must fail with, compared as a multiset. A write failure that is not a relationship
  validation error is named by `Error::code()` (e.g. `DUPLICATE_IN_BATCH`). A fixture with
  `expectErrors` has no assertions or steps.
- Runner: the publishable `pemba-fixtures` crate (format + engine-only runner, which the
  CLI `pemba test` uses) and `pemba-testkit::fixtures::run(path, store_kind)`, the same
  runner with the reference evaluator plugged in as its `Oracle` (OQ-024, T3.3). pemba's
  own tests use the testkit form.
  Each fixture runs against **both** memory and SQLite stores (memory only until the SQLite
  store lands in M2; adding a store kind is one `StoreKind` variant and one match arm).
- **Every `assertTrue` / `assertFalse` is cross-checked** (testkit form; `pemba test`
  checks the engine alone): the runner answers it with the
  engine and with the reference evaluator (§2.1) over its own model of the live tuples, and
  reports a failure naming each of the two that disagrees with the file. Objects the
  assertions name are added to the oracle's universe. If the oracle declines the schema
  (`NotStratifiable`), the runner warns and checks the engine only.
- CRLF line endings are accepted: the runner turns `\r\n` into `\n` in the fixture and in a
  `schemaFile` before parsing, so schema, relationships and assertions read the same.
- A fixture that is not valid YAML, or not a valid fixture, is a failure (exit 1); only an
  unreadable file is an I/O error (exit 2).
- YAML crate: `serde-saphyr` (`docs/09-repo-and-tooling.md` §2).

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
| `schema_errors/*.yaml` | one file per schema error code (except `E_EMPTY_RELATION_TYPES`, which the parser makes unreachable from source text: `relation r:` is `E_UNEXPECTED_TOKEN`) |

### 3.2 pemba extensions (ignored by `zed`)

Both extensions are pemba-only keys; `zed validate` does not know them, so a file using
them is a pemba fixture first. What `zed` would check is exactly the top-level
`relationships` and `assertions` ("step 0").

**`expectCheckErrors`** (OQ-022): a map from a check, in assertion syntax, to the code
`Error::code()` that check must fail with. Allowed at top level (checked after step 0) and in
each step. Not cross-checked against the oracle, which has no evaluation limits.

```yaml
expectCheckErrors:
  document:deep#view@user:root: MAX_DEPTH_EXCEEDED
  doc:a#view@user:u: NON_MONOTONIC_CYCLE
```

A check named here must not also appear in `assertTrue` / `assertFalse`: an expected answer
and an expected error are different claims.

**`steps`** (OQ-023): an ordered list run after step 0. Each step is one write batch
(`ops`, applied in order at one revision), then its own `assertions` and
`expectCheckErrors`, evaluated at the new head. `expectError` names the code the batch
must fail with (`Error::code()`, or the `E_...` code for an invalid relationship); a batch
that fails applies nothing, which the step's assertions can then show.

```yaml
steps:
  - ops:
      - create: document:spec#viewer@user:erin
      - create: document:readme#viewer@user:anne   # already live
    expectError: ALREADY_EXISTS
    assertions:
      assertFalse:
        - document:spec#view@user:erin             # nothing from the batch applied
  - ops:
      - delete: document:readme#viewer@user:anne
    assertions:
      assertFalse:
        - document:readme#view@user:anne
```

---

## 4. Differential testing against SpiceDB

Crate `pemba-difftest` (dev-only, never published). Tests are `#[ignore]` and require
`PEMBA_SPICEDB_URL` (the HTTP gateway) + `PEMBA_SPICEDB_KEY` (a token prefix). Without
them the tests print that they did not run and pass, unless `PEMBA_DIFFTEST_REQUIRED` is
set (CI sets it). CI: `difftest.yml`, weekly and on manual dispatch (`09` §4).

Run SpiceDB (pinned in `difftest.yml`; 1.56.2 when T3.4 landed):
```bash
docker run --rm -p 8443:8443 authzed/spicedb:v1.56.2 serve-testing --http-enabled
PEMBA_SPICEDB_URL=http://127.0.0.1:8443 PEMBA_SPICEDB_KEY=difftest \
    cargo test -p pemba-difftest -- --ignored
```
`serve-testing` gives an isolated, in-memory datastore per bearer token; the harness uses
a fresh token per case (and per throwaway probe), so cases never see each other's data.

Procedure per case (every fixture without `expectErrors`, plus generated cases):
1. `POST /v1/schema/write` with the same schema text, first to 12 throwaway datastores
   (KD-2: SpiceDB's refusal of some wildcard schemas is nondeterministic), then to the
   case's own. A refusal that KD-2 explains counts the case as not compared.
2. `POST /v1/relationships/write` with the same tuples (`OPERATION_TOUCH`).
3. Questions: `check` of every member of every object in the case (plus a never-written
   `fresh` object of every type) for every object subject and every subject set in the
   tuples, via `POST /v1/permissions/checkbulk` with `fullyConsistent`; up to 12
   `lookup_resources` (object subjects) and 12 `lookup_subjects`, compared as sets.
4. Compare with pemba (memory store). Two errors agree (the wording differs). A mismatch
   that a `KNOWN_DIFFERENCES.md` detector explains is counted; any other fails the run and
   prints the case as fixture YAML ready to paste into `testdata/fixtures/regressions/`.

Generated cases come from the property-test generator (`generate::case_with`, both
`SubSides` variants, deterministic seed) with two caps, because SpiceDB cannot run every
generated case:
- **No cycles in the data**: each tuple that would close a cycle between objects is dropped
  (`without_cycles`). On cyclic data SpiceDB dispatches until its depth limit (KD-1).
- **At most 16 relationships** per case (after cycle removal). SpiceDB does not memoise
  across branches of one check; on the densest acyclic data one bulk check used up 2.5 GB
  and SpiceDB ran out of memory.
Cases are generated until at least 500 were compared (`PEMBA_DIFFTEST_CASES` raises it).
Cyclic and larger data are compared through the fixtures. The property tests (§2) have
neither cap.

Known/expected divergences are listed in `crates/pemba-difftest/KNOWN_DIFFERENCES.md`, each
with a reason, a minimal repro and a detector in `src/known.rs` that needs the exact shape
of the disagreement; the harness explains only those.

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
- Wheel smoke test in CI on each platform: `pip install <wheel> && python -c "import pemba"`.

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
