# Open questions

Questions where the spec is ambiguous, undecided or possibly wrong. Format:
`docs/09-repo-and-tooling.md` §6. Every entry records the conservative choice in force until
it is resolved.

## OQ-001 — Final project name
- Raised in: T0.1 (`docs/01-decisions.md` ADR-012)
- Blocking: yes, for 0.1
- Current choice: codename `warden`, kept only in crate names, the Python module name,
  `TABLE_PREFIX` and docs so the rename is a mechanical find-replace
- Resolve by: human picks a name free on crates.io, PyPI, npm and GitHub before publishing
- Resolution: resolved before 0.1 by the human: the final name is `pemba` (ADR-012
  Accepted). Every `warden` / `Warden` / `WARDEN` spelling was renamed: crates
  `warden-*` → `pemba-*`, Python package `warden` → `pemba` (native module `_pemba`,
  `WardenError` → `PembaError`), CLI binary `pemba`, environment variables `WARDEN_*` →
  `PEMBA_*`, repository `github.com/Joshua-Fields/pemba`. On-disk format change: the SQLite
  tables and indexes are now `pemba_*` (`pemba_meta`, `pemba_revisions`,
  `pemba_relationships`, `pemba_rel_*`, `pemba_schema`), so a database created under the
  codename is not recognised. Nothing was published, so there is no migration path and
  `storage_version` stays `1` (`docs/05-storage.md` §4.1).

## OQ-002 — Postgres revision ordering
- Raised in: T0.1 (`docs/01-decisions.md` ADR-005)
- Blocking: no, for 0.1
- Current choice: deferred; revisions must be strictly increasing in commit order, which is
  trivial on SQLite (single writer). Postgres store is out of scope until told otherwise
- Resolve by: design work before the Postgres store
- Resolution: (fill in)

## OQ-003 — Async store story before Postgres
- Raised in: T0.1 (`docs/01-decisions.md` ADR-003)
- Blocking: no
- Current choice: synchronous `Engine` and store traits; no async runtime in library crates.
  A future async Postgres store would sit behind a blocking adapter or use a sync driver
- Resolve by: design work before the Postgres store
- Resolution: (fill in)

## OQ-004 — `.all()` over zero related objects
- Raised in: T0.1 (`docs/01-decisions.md` ADR-013)
- Blocking: no
- Current choice: `.all()` over zero related objects evaluates to `false`; if SpiceDB differs,
  match SpiceDB and update ADR-013
- Resolve by: differential test in T3.4
- T3.4 evidence: SpiceDB 1.56.2 agrees (`false` over zero related objects;
  `crates/pemba-difftest/KNOWN_DIFFERENCES.md`, "Checked and agreeing"). Left for the
  human to mark resolved.
- Resolution: (fill in)

## OQ-005 — Self membership semantics vs SpiceDB
- Raised in: T0.1 (`docs/04-evaluation.md` §2)
- Blocking: no
- Current choice: a subject set `o#m` is always a member of `S(o, m)`
- Resolve by: differential test in T3.4
- T3.4 evidence: SpiceDB 1.56.2 agrees (`o#m` in `S(o, m)` with no tuples, also through
  a permission `is_member = member`; not across objects). Left for the human to mark
  resolved.
- Resolution: (fill in)

## OQ-006 — Does SpiceDB accept arrows over relations that allow wildcards?
- Raised in: T0.1 (`docs/03-schema-language.md` §4.2)
- Blocking: no
- Current choice: reject (`E_ARROW_OVER_WILDCARD`), conservative
- Resolve by: differential test in T3.4
- T3.4 evidence: SpiceDB 1.56.2 refuses such a schema too ("wildcard relations cannot be
  used on the left side of arrows", 60 of 60 writes). Left for the human to mark
  resolved.
- Resolution: (fill in)

## OQ-007 — Type-prefix segment rules and maximum type name length
- Raised in: T0.1 (`docs/02-data-model.md` §1.1)
- Blocking: no
- Current choice: every `/`-separated segment obeys the 3–64 character identifier rule; full
  type name at most 128 bytes
- Resolve by: differential test in T3.4
- T3.4 evidence: SpiceDB 1.56.2 accepts and refuses the same 11 probe names as pemba
  (segments of 2/3/64/65 characters, several prefixes, 128 vs 129 bytes, trailing `_`).
  Left for the human to mark resolved.
- Resolution: (fill in)

## OQ-008 — Performance targets
- Raised in: T0.1
- Blocking: no
- Current choice: no numeric targets; correctness first
- Resolve by: set after benchmarks in T3.5
- Resolution: (fill in)

## OQ-009 — Text syntax for `Revision`, `Consistency`, `WriteOp`, `RelationshipFilter`
- Raised in: T0.2 (`docs/08-milestones.md` T0.2 required `Display`/`FromStr`; `docs/02-data-model.md`
  §3 only defines syntax for objects, subjects and relationships)
- Blocking: no
- Current choice: see Resolution
- Resolve by: human review
- Resolution: resolved in T0.2 by the human. `FromStr` is kept only for `ObjectRef`,
  `SubjectRef`, `Relationship` and `Revision` (plus the name newtypes as validating
  constructors). `Revision` is decimal digits with no sign and no leading zeros, so it
  round-trips exactly (`02` §3). `Consistency`, `WriteOp` and `RelationshipFilter` keep
  `Display` for debugging only, with no stable format and no `FromStr`. T0.2 acceptance
  criteria in `08` and `02` §2–3 updated to match.

## OQ-010 — Doc comments that attach to nothing
- Raised in: T0.3 (`docs/03-schema-language.md` §2)
- Blocking: no
- Current choice: see Resolution
- Resolve by: human review
- Resolution: resolved in T0.4 by the human. A doc comment before `}`, at end of file, or
  inside a member is reported as the warning `W_DANGLING_DOC_COMMENT` (not an error, so
  schemas SpiceDB accepts still compile, ADR-004). The parser records these in
  `ast::Schema::dangling_docs`; consecutive doc comments before one node are still joined
  with `\n`. Spec `03` §2 and §5 updated.

## OQ-011 — Whitespace inside type references
- Raised in: T0.3 (`docs/03-schema-language.md` §2–3)
- Blocking: no
- Current choice: see Resolution
- Resolve by: human review
- Resolution: resolved in T0.4 by the human. Whitespace (or a comment) around `#` and `:`
  in a type reference is rejected with `E_UNEXPECTED_TOKEN`, as around `/` in a type name:
  `group # member`, `group #member`, `user : *` and `user: *` all fail with a fix-it
  (`write \`group#member\``). Reasoning: ADR-004 makes our language a strict subset of
  SpiceDB's, so every schema we accept must parse in SpiceDB. We have not confirmed that
  SpiceDB accepts the spaced forms, and the compact forms are the only ones its
  documentation uses, so accepting only those keeps us inside the subset whatever SpiceDB
  does. Loosening later is backwards compatible; tightening would not be. The `:` after a
  relation name (`relation viewer : user`) is not part of a type reference and stays free.
  Spec `03` §2 updated.

## OQ-012 — Round-trip property when the printer reorders members
- Raised in: T0.5 (`docs/03-schema-language.md` §9)
- Blocking: no
- Current choice: see Resolution. Original note: §9 asked for `compile(print(compile(s))) == compile(s)` as IR equality, but
  the printer moves relations ahead of permissions, which renumbers `RelId`s when `s`
  interleaves them, and the recompiled `source` field is the printed text, not `s`. Literal
  `==` cannot hold for such inputs. The property test checks (a) equality by name (ids
  replaced by the names they stand for) for every generated input, (b) equal `hash`,
  (c) printing is a fixed point, and (d) exact `==` of the whole `CompiledSchema` once the
  input is canonical, plus equal `types` when the input was already in canonical member
  order. Alternative not taken: make `compile` itself order members relations-first, which
  would change `RelId` assignment for every schema
- Resolve by: human review
- Resolution: resolved in T0.5 by the human. The three-part property is accepted
  (same schema by name with equal hash; printing is a fixed point so the IR round-trips
  exactly once canonical; unchanged `types` for input already in canonical member order).
  Spec `03` §9 now states it in place of literal IR equality.

## OQ-013 — `pemba schema fmt` output target, comments, exit code for I/O errors
- Raised in: T0.5 (`docs/06-api.md` §3)
- Blocking: no
- Current choice: see Resolution. Original note: `fmt` prints to stdout and never rewrites the file in place, because the
  canonical printer works from the IR and drops plain `//` and `/* */` comments; rewriting
  in place would silently delete them. `fmt --check` therefore also fails for any file with
  plain comments. Exit code 2 for "could not run" (unreadable file, bad arguments, matching
  clap's usage-error code), 1 for a rejected schema or non-canonical file. Output colours
  are deferred. A comment-preserving formatter (from the AST plus comment trivia) and an
  in-place `--write` flag are possible later additions
- Resolve by: human review
- Resolution: resolved for now in T0.5 by the human. `schema fmt` is marked experimental
  in its `--help` text and in `06` §3 ("drops non-doc comments; does not modify files").
  Exit code 2 is documented for usage and I/O errors. A comment-preserving formatter is
  task T3.8 in `08`, before the 0.1 gate.
- T3.8 done: `fmt` keeps comments (`03` §9.1, OQ-036). Still marked experimental and still
  never writes files; dropping the "experimental" label is for the human.

## OQ-014 — Store write path: typed `WritableStore`, SQL in `SqlStore`
- Raised in: T1.1 (`docs/05-storage.md` §2–3, §9)
- Blocking: no
- Current choice: as specified, `WritableStore` only had `with_write_txn` (SQL) and
  `dialect`, which the memory store cannot implement, and the traits had no way to store a
  schema version or run GC, although the conformance suite (§9) must test both through a
  generic store. `WritableStore` now has typed methods (`write`, `write_schema`, `gc`) that
  every store implements; `with_write_txn`/`dialect` moved to a sub-trait `SqlStore`, which
  the SQLite store will implement and back its `WritableStore` methods with the shared
  algorithm in `pemba-core::write`. `Engine::write_in` requires `S: SqlStore`. Schema
  validation (§5 step 1) stays in the engine, outside the store call. Spec `05` §2–3 and
  `06` §1 updated. `delete_by_filter` will be added to `WritableStore` in T2.6
- Resolve by: human review
- Resolution: (fill in)

## OQ-015 — Store result shapes the spec left open
- Raised in: T1.1 (`docs/05-storage.md` §2, §6.3, §7)
- Blocking: no
- Current choice, all pinned by the conformance suite so SQLite must match:
  (a) `read_reverse` returns *distinct* pairs; the §6.3 SQL had no `DISTINCT`, so a resource
  reached through two subject relations (`Any`) appeared twice. Now `SELECT DISTINCT`.
  (b) `read_relationships` orders by the six storage columns byte-wise (`BINARY` collation;
  wildcard id `*`, absent subject relation `''`), `after` is an exclusive keyset cursor,
  `limit = 0` returns nothing. (c) `count_by_relation` also groups by "is wildcard"; the §7
  `GROUP BY` lumped `t:*` tuples with `t` objects, so removing `t:*` from a relation's
  allowed types could not be checked for orphans. (d) every read at a revision outside
  `gc_horizon..=head` fails with `StoreError::RevisionOutOfRange` instead of answering;
  this costs the SQLite store a head/horizon check per read (or per request, if reads are
  later batched in one read transaction)
- Resolve by: human review; revisit (d) if it shows up in T3.5 benchmarks
- Resolution: resolved in T1.2 by the human. (a)–(c) stand as pinned by the conformance
  suite. (d): the engine resolves `Consistency` to one revision and validates it against
  `gc_horizon`/head once per request (authoritative reads, never a cache); the store keeps
  its per-read range check as a backstop but compares against a horizon cached in memory,
  never an extra query. Documented in `05` §2 ("Revision checks"), including the
  cross-process GC caveat T2.3 must close (run the check and the reads in one snapshot)

## OQ-016 — Order of write errors: duplicates before preconditions
- Raised in: T1.1 (`docs/02-data-model.md` §5 lists preconditions before duplicates;
  `docs/05-storage.md` §5 lists duplicates first)
- Blocking: no
- Current choice: `05` §5 wins (more specific on storage): duplicates, then preconditions,
  then `Create` conflicts. Only affects which error a batch with several problems reports.
  Tested in the conformance suite
- Resolve by: human review
- Resolution: resolved in T1.2 by the human. Duplicates are checked before preconditions.
  `02` §5 now lists the full order (batch size, schema present, validation, duplicates,
  preconditions, revision, ops/`AlreadyExists`), matching `05` §5. Tested at the engine
  level in `crates/pemba/tests/engine.rs` as well as in the conformance suite

## OQ-017 — Codes for relationship validation; validating filters
- Raised in: T1.2 (`docs/02-data-model.md` §6 named only `E_WRITE_TO_PERMISSION`;
  `docs/06-api.md` §1.1 types `InvalidRelationship.code` as `ErrorCode`)
- Blocking: no
- Current choice: (a) codes are new variants of `pemba_schema::ErrorCode`, one namespace
  with schema diagnostics as fixtures' `expectErrors` expects: undefined resource type reuses
  `E_UNKNOWN_TYPE`; undefined relation is `E_UNKNOWN_RELATION`; a permission is
  `E_WRITE_TO_PERMISSION`; any subject not in the allowed list, including an undefined
  subject type or subject relation, is `E_SUBJECT_TYPE_NOT_ALLOWED`. `Delete` ops are
  validated too. (b) `read_relationships` filters and precondition filters must name only
  defined types, relations (not permissions) and subject relations, else `InvalidRequest`:
  such a filter can never match, and a typo in `MustNotExist` would otherwise pass silently.
  (c) Batch size is checked before anything else, so an oversized batch never pays for
  validation. Preconditions are not counted against `max_write_batch`. Spec `02` §5–6 and
  `06` §1 updated
- Resolve by: human review; revisit validating `Delete` ops of tuples left behind by a schema
  change once T2.6 lands (it should then be impossible)
- Resolution: (fill in)

## OQ-018 — Which cycles count as "through negation"
- Raised in: T1.3 (`docs/04-evaluation.md` §3.6)
- Blocking: no
- Current choice (T1.3, superseded by the resolution): §3.6 as written, plus one fix. Frames
  pushed while evaluating an exclusion's `sub` side are `negated` and so is everything
  beneath; a cycle is
  `NonMonotonicCycle` if any frame from the repeated node to the top is negated **or the
  edge closing the cycle is evaluated under negation**. Without that last clause the
  self-loop `view = viewer - parent->view` with `doc:a#parent@doc:a` finds no negated
  frame (the only frame is `doc:a#view`, pushed outside the `sub` side), returns a tainted
  `false` and grants `view` — an unsound `true`. Consequence of the literal "everything
  beneath" rule: a cycle that lies entirely *inside* a `sub` side and is itself monotone
  (e.g. `view = viewer - banned`, `banned: group#member`, nested groups that contain each
  other) is also refused, although its value is well defined. Tests:
  `self_loop_through_exclusion_is_non_monotonic_cycle`,
  `any_cycle_beneath_subtracted_side_is_refused`
- Resolve by: T1.4b
- Resolution: decided by the human in T1.3 review; implemented in T1.4b. A cycle is
  `NonMonotonicCycle` only if the path from the in-progress node (including its own
  expression) to the closing edge crosses a subtracted side. Implemented with negation
  depth, not the sticky flag: each frame records how many `sub` sides it sits inside; a
  hit on an in-progress frame errors iff the current depth differs from the frame's; a
  pass-memo hit compares against the frame of the entry's region head (at its lowlink). A
  monotone cycle wholly inside a `sub` side settles by SCC iteration and is then negated.
  `04` §3.6 rewritten. `any_cycle_beneath_subtracted_side_is_refused` became
  `monotone_cycle_beneath_subtracted_side_is_negated_least_fixed_point`; new tests for a
  cycle through a nested exclusion and for both pass-memo cases; property 1 also runs
  (≥ 1 000 cases) on a generator variant with recursion beneath `sub` sides

## OQ-019 — `check` details the spec leaves open
- Raised in: T1.3 (`docs/04-evaluation.md` §1, §3; `docs/06-api.md` §1)
- Blocking: no
- Current choice:
  (a) `CheckItem` was undefined: it is `CheckRequest` without `consistency`.
  (b) Depth: the root node is evaluated at depth 1 and each nested `eval` adds one, so
  `max_depth = N` allows N nested nodes; `MaxDepthExceeded.path` lists the stack plus the
  node that would have exceeded it.
  (c) Fan-out: "tuples read by one node" sums every arrow read in a permission node's
  expression; for a relation it is the subject-set read. More than `max_check_fanout` errors.
  (d) Request validation (§1 step 1): resource type and member must exist; an object
  subject's type must exist; a subject set's type and member (relation or permission) must
  exist. Whether the subject type can reach the permission at all is not checked; such a
  check is simply `false`.
  (e) Stored subject sets whose form is not an allowed type of the relation (only possible
  after a schema change, until T2.6 refuses orphaning changes) fail the check with
  `StaleRelationship { tuple, relation }`. Originally (a5f9dc5) they were skipped, which
  fails open on the subtracted side of an exclusion: drop `group#member` from `banned` and
  every member of a banned group gains access. Changed in T1.3 review; the fan-out query now
  runs even for relations that allow no subject sets (`04` §3.3). Tests:
  `stale_subject_set_on_subtracted_side_is_an_error_not_true`,
  `stale_subject_set_on_granting_side_is_an_error_not_false`,
  `stale_subject_set_of_removed_type_is_an_error`,
  `stale_subject_set_fails_only_its_check_bulk_item`.
  Still open, same class: a stale *object* tuple is matched by the exact-hit step (grants on
  the granting side), and a stale arrow-related object whose type the tupleset no longer
  allows is treated like a type lacking the target (∅, which fails open under exclusion,
  e.g. `view = viewer - parent->banned`). Both need a decision; T2.6 removes all three cases
  (f) `check_bulk`: `Store` and mid-call `RevisionExpired` fail the call; all other errors
  are per item (documented in `06` §1)
- Resolve by: human review
- Resolution: partly resolved by the human after T1.3. The remaining stale cases of (e) (a
  stale object tuple matched by the exact-hit step; a stale arrow-related object whose type
  the tupleset no longer allows, treated as ∅) are deferred to T2.6, which makes all three
  impossible after `write_schema`; its acceptance criteria now require a test proving it.
  (a)–(d) and (f): (fill in)
  T2.6 (at the human's direction): all three are impossible **at head**: `write_schema`
  refuses any change a live tuple would fail validation under, and writes commit only at
  the schema version they were validated against (`05` §7); tests
  `schema_change_leaves_no_stale_tuple_at_head` (property) and the orphan cases in
  `crates/pemba/tests/schema_change.rs`. In history (`Exact(r)` below the current schema's
  revision) they can still be met, and each now fails closed with `StaleRelationship`: a
  stale subject set (as before), a stale object tuple on the exact-hit step, a stale
  wildcard (probed only at such revisions), a stale arrow-related object (`04` §3.3; tests
  `historical_stale_*`). (e) is resolved.

## OQ-020 — Reference evaluator's Kleene iteration is unsound with a recursive exclusion base
- Raised in: after T1.3 review (`docs/07-testing.md` §2.1), for T1.4
- Blocking: no (was blocking T1.4; resolved there)
- Problem: §2.1 recomputes every `S(o, m)` from ∅ until nothing changes. That is only sound
  for monotone schemas, and the generator restriction in §2.1 only keeps recursion out of an
  exclusion's `sub` side; a recursive *base* side is still allowed. Example:
  `permission view = (viewer - banned) + parent->view`, `folder:a#parent@folder:b`,
  `folder:b#parent@folder:a`, user `u` both `viewer` and `banned` on `a`. The correct
  (stratified) answer is `view(a) = view(b) = ∅`, and the T1.3 engine returns `false` for both.
  Simulated (scratch script, not committed), what the iteration does depends on details §2.1
  leaves open:
  - simultaneous (Jacobi) updates, `banned` direct as above: converges to ∅ (correct, by luck:
    `viewer` and `banned` fill in the same round);
  - simultaneous updates with `banned` one hop deeper (e.g. `banned: group#member`): round 1
    sees `banned = ∅`, `u` enters `view(a)`, then alternates between `view(a)` and `view(b)`
    forever (`a- -b a- -b ...`), so the iteration never terminates;
  - in-place (Gauss-Seidel) updates in source order, either variant: stops changing after
    round 1 with `u ∈ view(a)` and `u ∈ view(b)`, kept alive by the cycle. That is a wrong
    answer that still looks like a fixed point, which is worse than not terminating.
- Current choice: see Resolution. Proposed fix for T1.4: stratify by the
  `(type, member)` dependency graph (edges from computed references, arrow targets on every
  allowed type, and subject-set allowed types) and evaluate each stratum to a fixed point
  before any stratum that subtracts it. A schema where a `sub` side depends on its own
  stratum is not stratifiable. **Refusing such schemas applies only to the reference
  evaluator (`pemba-testkit::reference`) and the proptest generators**: the oracle declines
  to answer and the generators must not produce them. The schema compiler keeps accepting
  them and the engine keeps evaluating them, deciding per check from the data
  (`NonMonotonicCycle` only when a cycle is actually reached, `04` §3.6); hand-written
  `NonMonotonicCycle` tests cover that behaviour. Add the example above,
  in both the direct and one-hop-`banned` variants, as a fixed regression case, and add
  generators that put recursion on the base side of an exclusion. Note: stratifying by
  type-level graph is coarser than the engine's data-level cycle check, so some schemas the
  engine evaluates on cycle-free data may be refused by the oracle. That is acceptable for an
  oracle as long as the generators avoid those schemas. Relates to OQ-018 / T1.4b, whose
  stratified `sub`-side evaluation should reuse the same strata
- Resolve by: T1.4 (human to confirm the stratification approach)
- Resolution: resolved in T1.4 as proposed, at the human's direction. The oracle
  (`pemba-testkit::reference`) stratifies by the `(type, member)` graph above, with every
  edge reached through a `sub` side negative; a component holding a negative edge returns
  `ReferenceError::NotStratifiable` instead of an answer; components are evaluated in
  dependency order, each from ∅ to a fixed point. Monotone recursion inside a `sub` side is
  already handled (it is an earlier component), so T1.4b needs no oracle change. `07` §2.1
  rewritten. Both variants of the example are fixed tests (`oq020_banned_direct`,
  `oq020_banned_one_hop_deeper` in `crates/pemba-testkit/tests/reference.rs`), checking
  the oracle and the engine; generators put recursion on exclusion base sides

## OQ-021 — Exponential `check` cost on cyclic data (denial-of-service risk)
- Raised in: T1.4 (`docs/07-testing.md` §2.3, `docs/04-evaluation.md` §3.6)
- Blocking: yes, for the 0.1 gate (T3.7)
- Problem: §3.6 never memoises tainted results, so inside a cyclic region the evaluator
  re-walks every path: cost is exponential on dense cyclic data. Answers stay correct (every
  slow case examined agreed with the reference evaluator), but cyclic group data is
  ordinary data that users can write, so a user can make one `check` arbitrarily expensive:
  a denial-of-service risk. `max_depth` and `max_check_fanout` do not bound total work.
- Observed (property tests built with `opt-level = 3` for `pemba-core`/`pemba-testkit`):
  one 1 000-case run of `prop1_check_matches_reference_full` took 3.0 s; the next, with a
  different random seed, was still running after 573 s and was stopped (its slow case was
  never printed). Pinned case (case 96 of the deterministic runner, not shrunk): 2 types,
  30 tuples, relations allowing subject sets of the other type's permissions (dense mutual
  recursion over ids `a`..`f`). One `check` makes 32 726 store reads at depth 29 (~170 ms in
  release); the oracle takes 21 ms for the whole case. Time is dominated by a few such cases.
- Current choice: no engine change in T1.4. The case is
  `check_on_dense_cyclic_data_reads_a_bounded_number_of_tuples`
  (`crates/pemba-testkit/tests/check_cost.rs`, ≤ 1 000 store reads), `#[ignore]`d and
  failing today. Plain `cargo test` runs property 1 at 64 cases plus saved seeds; the
  1 000-case run is the manual `proptest.yml` workflow
- T1.4c phase 1: lowlink taint plus memoising cycle heads and tainted `true`s is still
  exponential (complete group graph, subject in none: only the root is a head). Design
  replaced by SCC fixed-point iteration (`04` §3.6 planned rule T1.4c). Adversarial tests
  added to `check_cost.rs`, `#[ignore]`d; the complete-graph one fails today (store reads
  for n = 5..9: 130, 652, 3 914, 27 400, 219 202)
- Resolve by: T1.4c (bound check cost on cyclic data)
- Resolution: resolved in T1.4c. SCC fixed-point iteration (`04` §3.6, current rule) with
  lowlinks on a Tarjan-style SCC stack (positions, not call-stack indices, at the human's
  direction in review) bounds a `check` to `(|S| + 1) · Σ r(x)` store reads per region;
  `EngineConfig::max_check_reads` (default 200 000, `ReadLimitExceeded`) is the fail-closed
  backstop. Every `check_cost.rs` test passes un-ignored (the pinned case now makes well
  under its 1 000-read bound); property 1 at 1 000 cases runs in ~1 s optimised

## OQ-022 — Fixture extension for expected check errors (`expectCheckErrors`)
- Raised in: T1.5 (`docs/07-testing.md` §3, §3.1 `depth_limit`, `non_monotone_cycle`)
- Blocking: no
- Problem: SpiceDB validation files can only assert `true` / `false` / caveated, but
  `depth_limit.yaml` and `non_monotone_cycle.yaml` must show a check *failing*, and
  fail-closed means that is neither `assertTrue` nor `assertFalse`.
- Current choice: a pemba-only key `expectCheckErrors`, a map from check (assertion
  syntax) to `Error::code()` (`MAX_DEPTH_EXCEEDED`, `NON_MONOTONIC_CYCLE`, ...), at top level
  and per step (`07` §3.2). A map, not a list, so one check has exactly one expected code.
  A check may not appear both here and in `assertTrue` / `assertFalse`. Checked against the
  engine only (the oracle has no limits). The engine's default `EngineConfig` is used; no
  per-fixture config key yet, because the default `max_depth` (50) already makes the 60-folder
  chain fail. Not chosen: a list of `{check, error}` objects (wordier, allows duplicates)
- Resolve by: human review
- Resolution: (fill in)

## OQ-023 — Fixture extension for write-then-assert sequences (`steps`)
- Raised in: T1.5 (`docs/07-testing.md` §3.1 `direct`: Create/Touch/Delete visibility)
- Blocking: no
- Problem: a validation file has one relationship set and one assertion block, so it
  cannot show that a delete revokes, a failed batch applies nothing, or a deleted tuple can
  be created again.
- Current choice: a pemba-only ordered `steps:` list (`07` §3.2). The top-level
  `relationships` + `assertions` are step 0, so `zed validate` checks the same initial state;
  each later step is one write batch (`ops` of `create:` / `touch:` / `delete:` tuple strings),
  an optional `expectError` code, then the step's own `assertions` and `expectCheckErrors` at
  the new head. Preconditions and consistency other than `Latest` are not expressible yet;
  add `preconditions:` / `at:` keys when a fixture needs them. The runner keeps its own model
  of live tuples (not read back from the store) to feed the oracle, so a store that mishandles
  Delete is caught by the oracle disagreeing with the engine
- Resolve by: human review
- Resolution: (fill in)

## OQ-024 — `pemba-cli` depends on the unpublished `pemba-testkit`
- Raised in: T1.5 (`docs/06-api.md` §3 `pemba test`, `docs/09-repo-and-tooling.md` §1)
- Blocking: yes, for publishing `pemba-cli` to crates.io (T3.x release)
- Problem: `07` §3 puts the runner in `pemba-testkit` (`publish = false`) and `06` §3 puts
  `pemba test` in the CLI, so the CLI now has a normal dependency on the testkit (and through
  it `proptest`, `serde`, `serde-saphyr`). `cargo publish` refuses a crate whose dependency is
  not on crates.io.
- Current choice: accept for now (nothing is published before the release milestone); `09`
  §1 notes the exception. Options at release: move `fixtures` (and `reference`, which it
  cross-checks with) into a small published crate; publish `pemba-testkit`; or put
  `pemba test` behind a CLI feature that is off for the published binary
- Also decided here, conservatively: a fixture that is not valid YAML, has an unknown key, or
  needs exactly one of `schema` / `schemaFile` and has neither is exit 1 (input rejected, as
  for `schema validate`), not exit 2; only an unreadable file is exit 2. `expectedLookups:`
  is skipped with a warning (like `validation:`) until T2.4 / T2.5, rather than refused
- Resolve by: before the first crates.io release
- Resolution: resolved in T2.1 by the human (docs only; the move itself is a T3.3 AC). Before
  T3.3 the fixture format and the engine-only fixture runner move out of `pemba-testkit`
  into a small publishable crate, which `pemba-cli` depends on for `pemba test`. The
  reference-evaluator (oracle) cross-check stays in `pemba-testkit` as an optional hook
  the runner accepts, used by internal tests only; the published CLI runs fixtures against
  the engine alone. `pemba-testkit` stays `publish = false`
- Done in T3.3: crate `pemba-fixtures` (OQ-035 (a)).

## OQ-025 — SQLite store details decided in T2.1
- Raised in: T2.1 (`docs/05-storage.md` §2, §4, §6; `docs/06-api.md` §1)
- Blocking: no
- Current choice:
  (a) `pemba_rel_fwd` is now (resource_type, resource_id, relation, subject_type,
  subject_id, subject_relation, created_rev, deleted_rev) instead of (resource_type,
  resource_id, relation, created_rev). The old index could not serve the §6.2 `ORDER BY`
  (the §2 result order), so every forward read sorted; the new one returns rows in index
  order and is covering for §6.1–6.2. Storage version 1 is unreleased, so no migration is
  needed. Spec §4 updated; a test checks the migration DDL equals the §4 DDL.
  (b) §6 queries name their index with `INDEXED BY`: unpinned, SQLite answered
  `read_reverse` with a resource type from `pemba_rel_fwd` (a range over every resource
  of the type), and app-run `ANALYZE` could move any plan.
  (c) In-memory SQLite is rejected (`InvalidRequest`) instead of supported with a
  shared-cache URI and pool size 1 (§4.2): no WAL, and shared cache swaps snapshot
  isolation for table locks. Conservative; `pemba-store-memory` covers the use.
  (d) `auto_migrate` lives on `SqliteConfig`, read by `SqliteStore::open`, because the store
  is opened (and its version checked) before any engine exists; the explicit step is
  `SqliteStore::migrate(path, &config)`. `EngineConfig::auto_migrate` and
  `Engine::migrate()` (`06` §1) currently do nothing for SQLite; proposal: remove both, or
  make `Engine::migrate` delegate through a store method, decided in T2.2/T2.3.
  (e) A database at an *older* storage version with `auto_migrate = false` is
  `NotMigrated` (same as missing tables). A `pemba_meta` table without a numeric
  `storage_version` is `StoreError::Corrupt`, never migrated over.
  (f) New `StoreError` variants: `Backend { message }` (driver failure) and `Corrupt`.
- Resolve by: human review
- Resolution: resolved in T2.2 by the human. (a), (b), (c), (e) and (f) accepted as
  implemented. (d): `EngineConfig::auto_migrate` and `Engine::migrate()` are removed;
  migration lives only on the store (`SqliteConfig::auto_migrate`, `SqliteStore::migrate`).
  `06` §1 and `05` §4.1 updated

## OQ-026 — Write-path details decided in T2.2
- Raised in: T2.2 (`docs/05-storage.md` §2, §5, §5.1, §10; `docs/06-api.md` §1)
- Blocking: no
- Current choice:
  (a) `pemba_core::write::write` runs steps 3–6 inside `SAVEPOINT pemba_write` and rolls
  back to it on error. The spec only said a failed native write rolls back; for `write_in`
  the caller owns the transaction, and without the savepoint an `AlreadyExists` on the
  third op would leave a revision row and two ops behind for a caller that commits anyway.
  (b) Same-file verification is a new `SqlStore::check_same_database(&mut dyn WriteTxn)`
  method: `Engine::write_in` is generic over the transaction type, so only the store knows
  how to ask (SQLite: `PRAGMA database_list` through the transaction, compared with the
  store's canonicalised file). Rust checks on every call; the Python per-connection cache of
  §10 stays a `pemba-py` optimisation.
  (c) New `StoreError::Conflict { message }` + `StoreError::is_retryable()` for
  `SQLITE_BUSY_SNAPSHOT` (lost lock upgrade) and `SQLITE_BUSY`/`SQLITE_LOCKED`. Verified:
  a deferred transaction that ran a `MustNotExist` precondition, then saw another connection
  commit the conflicting tuple, fails with `SQLITE_BUSY_SNAPSHOT` at the revision `INSERT`;
  no busy wait, never a successful write.
  (d) `SqliteTxn::new` takes `&rusqlite::Transaction`, not a bare connection, so a Rust
  foreign write is always inside a caller transaction (the Python autocommit guard of §10
  has no Rust counterpart to need). `pemba-store-sqlite` re-exports `rusqlite`.
  (e) SQLite placeholders are plain `?` (as `05` §2 says), never reused, so the same text
  works with Python's qmark style. `committed_at` is computed in Rust, not by SQL, to stay
  portable. GC deletes in one statement until T2.6 adds batching.
  (f) Rolled-back revisions are reused (AUTOINCREMENT's counter rolls back with the
  transaction); `06` §1 documents that such a revision is void. `pemba_revisions`'s
  "never reuse ids" comment holds only for committed revisions.
- Resolve by: human review
- Resolution: resolved in T2.3 by the human: (a)–(f) all accepted as written. `06` §1 now
  states that `StoreError::Conflict` means the caller's whole transaction is stale and must
  be rolled back and retried from the start. Consequence of (d): because
  `pemba-store-sqlite` re-exports `rusqlite` (and `SqliteTxn::new` takes its
  `Transaction`), a `rusqlite` major-version bump is a breaking change for
  `pemba-store-sqlite` and needs a major (pre-1.0: minor) version bump of that crate

## OQ-027 — Read snapshots: shape of the API (T2.3)
- Raised in: T2.3 (`docs/05-storage.md` §2 "Revision checks", `docs/04-evaluation.md` §1)
- Blocking: no
- Current choice:
  (a) New object-safe trait method `ReadStore::snapshot(&self) -> Result<Box<dyn ReadStore +
  '_>, StoreError>`, with a default returning the store itself (via a new blanket
  `impl ReadStore for &T`). The snapshot is itself a `ReadStore`, so the evaluator is
  unchanged. A public trait change: added to `05` §2.
  (b) The memory store keeps the default: its range check is authoritative (same lock as
  the rows), so a concurrent GC fails a read closed instead of hiding rows. Wrappers that
  override nothing (test stores) inherit the default too.
  (c) The SQLite snapshot holds its leased connection in a `Mutex` because `ReadStore` is
  `Sync` and `rusqlite::Connection` is not; one request uses it from one thread, so the lock
  is never contended.
  (d) The cross-process test pauses the request through a test-only wrapper store whose
  snapshot calls a hook after the first data read; there is no hook in library code.
  (e) Property 4 covers historical reads at every revision of a generated history, not GC:
  there is no `Engine::gc` yet (T2.6). GC expiry is covered by the snapshot tests and the
  conformance suite.
- Resolve by: human review
- Resolution: resolved in T2.4 by the human: (a)–(e) all accepted as written. Follow-up for
  (e): T2.6's AC now extends property 4 to GC (reads at `Exact(r)` below the horizon fail
  with `RevisionExpired`).

## OQ-028 — `lookup_resources` details the spec leaves open (T2.4)
- Raised in: T2.4 (`docs/04-evaluation.md` §5, `docs/06-api.md` §1, `docs/07-testing.md` §3)
- Blocking: no
- Current choice (conservative where it matters):
  (a) A subject-set subject `t:id#r` seeds tuples naming it exactly even if
  `subject_set_uses[(t, r)]` is empty. `check` finds such a tuple by exact `exists`
  whatever the schema allows, so gating the query on the schema could make a lookup omit a
  resource `check` allows (a stale tuple left by a schema change; T2.6 makes those
  impossible). One extra reverse read per lookup.
  (b) Reverse-read rows with a type or relation the schema no longer defines are skipped:
  no schema member reaches them. Stale subject *sets* that do resolve are kept as
  candidates; phase 2's `check` then fails the page with `StaleRelationship` as it would.
  (c) `next_cursor` is set when candidates remain after a full page, even if all of them
  will be denied, so a last page can be empty (`04` §5.2 read literally). Alternative not
  taken: evaluate ahead until one more is allowed (extra work on every page).
  (d) Each page re-runs phase 1 in full; nothing is cached between pages (a page is its own
  request and snapshot, `05` §2). Pagination therefore costs phase 1 per page.
  (e) A `check` error on any candidate fails the whole page, even one the page would not
  have returned anyway: the page cannot know which candidates it may skip.
  (f) The response carries no `EvalStats`; the `lookup_resources` tracing span has
  `nodes_visited`, `candidates`, `results`, `phase1_reads` and phase 2's `EvalStats` fields.
  (g) Fixtures: `expectedLookups.resources` at top level only (after step 0); `expect` is
  order-insensitive but may not repeat an id.
- Resolve by: human review
- Resolution: resolved in T2.5 by the human: (a)–(g) all accepted as written. `06` §1 now
  tells callers to loop until `next_cursor` is `None` and that any page, not only the last,
  may be empty. Whether to cache phase 1 candidates per (revision, query) across pages, given
  (d), is a T3.5 benchmark question.
  (d) resolved after T3.5 by the human: **no phase-1 cache in 0.1**. The T3.5 benchmarks
  (`benches/RESULTS.md`, 2026-10-19, i5-14600KF, Windows 11; p50, all 10k ids of one
  lookup) put re-running phase 1 on every page at about 3× the single-page cost, not 100×:

  | | one page | pages of 100 | ratio | extra per page |
  |---|---:|---:|---:|---:|
  | Rust, memory | 485 µs | 1,435 µs | 3.0× | ~9.6 µs |
  | Rust, SQLite | 1,200 µs | 3,440 µs | 2.9× | ~22.6 µs |
  | Python, memory | 497 µs | 1,438 µs | 2.9× | ~9.5 µs |
  | Python, SQLite | 2,383 µs | 6,025 µs | 2.5× | ~36.8 µs |

  The overhead is small and linear in the page count, and a cache would bring invalidation
  and memory questions of its own. Revisit if a workload pages through much larger result
  sets.

## OQ-029 — `lookup_subjects` details (T2.5)
- Raised in: T2.5 (`docs/04-evaluation.md` §6, `docs/06-api.md` §1, `docs/07-testing.md` §3)
- Blocking: no
- Current choice:
  (a) Cost bound: store reads count against the existing `max_check_reads` and fail with
  `ReadLimitExceeded`, rather than a new limit. One `lookup_subjects` is one evaluation by
  the same per-node, per-pass rules as one `check` (`04` §6.2), so the same bound reasoning
  and default (200 000) apply; a separate knob would add API surface with no different
  default to justify it. Cost: the error name says "check". Alternative: a
  `max_lookup_reads` with its own variant, if T3.5 shows `gather` needs a different budget.
  (b) Cycles: §3.6's SCC iteration is shared, not copied: `pemba-core::scc` is generic
  over a small lattice trait (`bool` for `check`, `SubjectSet` for `gather`). `check` was
  moved onto it unchanged (all `check` tests, `cycle_passes.rs` pass counts and property 1
  agree). The `bool` "assumed `false`" flag became "the value a hit assumed", and "a re-run
  is needed" became "some value grew strictly past what was assumed", identical for `bool`.
  (c) `gather` fan-out counts every tuple read on a relation (`check` counts subject sets
  only, because it looks objects up by `exists`): stricter, so it can fail where `check`
  would not, never the reverse.
  (d) Stale subject sets: `gather` resolves every stored set on a node it reaches and fails
  with `StaleRelationship` on any stale one; `check` can short-circuit past one after a
  `true`. Fail closed; T2.6 makes stale tuples impossible.
  (e) A stored wildcard counts only if the relation allows it (as `check`, `04` §3.3).
  (f) Short-circuits mirror `check`: an intersection stops at `Ids(∅)`, an exclusion skips
  its `sub` side when the base is `Ids(∅)`, `.all` stops at `Ids(∅)`.
  (g) The response carries no `EvalStats`; the `lookup_subjects` span has them plus
  `results` and `wildcard`. Not paginated (`04` §6 has no cursor).
  (h) Fixture `expect` format for subjects: a map `ids` / `wildcard` / `excluded` (`07` §3).
- Resolve by: human review
- Resolution: resolved in T2.6 by the human: (a)–(h) all accepted. Consequence of (a):
  `CheckReadsExceeded` / `CHECK_READS_EXCEEDED` renamed to `ReadLimitExceeded` /
  `READ_LIMIT_EXCEEDED` (nothing released yet), since `lookup_subjects` raises it too. `06`
  §1 documents (d): `lookup_subjects` may fail with `StaleRelationship` where `check`
  answers.

## OQ-030 — Schema change safety, GC order, delete-by-filter (T2.6)
- Raised in: T2.6 (`docs/05-storage.md` §5, §7, §8; `docs/03-schema-language.md` §10;
  `docs/02-data-model.md` §5; `docs/06-api.md` §1)
- Blocking: no
- Current choice:
  (a) Store traits changed (public, pre-release): `WritableStore::write` takes the schema
  version it was validated against; `write_schema` takes a `check` run on the live counts
  inside its transaction; `gc` split into `raise_gc_horizon` + `gc_batch`, with `gc` a
  provided method; new `delete_matching`, `ReadStore::schema_version`,
  `SqlDialect::row_id`, `StoreError::SchemaChanged`. The split GC is what lets a test pause
  between batches without hooks in library code.
  (b) Orphan criterion: "would fail write validation under the new schema", computed per
  `count_by_relation` group, instead of a diff of removed elements. Equivalent to the listed
  removals and complete by construction; it also catches any stale tuple left at head from
  before T2.6. Reports carry the validation `ErrorCode`.
  (c) Error code `SCHEMA_CHANGE_ORPHANS` (06's naming rule) for the variant `03` §10 calls
  `E_SCHEMA_CHANGE_ORPHANS`.
  (d) `Exact(old)` reads use the current schema (MVP, unchanged): stale tuples on surviving
  relations fail closed, but tuples on a relation the current schema removed entirely (or
  turned into a permission) are never read, so they do not count at `Exact(old)`. Evaluating
  historical reads with the historical schema is the alternative, post-MVP.
  (e) The stale-wildcard probe costs one extra point read per relation evaluation, only at
  revisions below the current schema's `created_rev`.
  (f) GC deletes until a batch removes nothing, so the last batch is always empty (one
  extra short transaction): it makes a capped or partial batch harmless and needs no
  knowledge of the batch size.
  (g) `delete_relationships` allocates a revision even when nothing matches (as `02` §5
  rule 8 for writes) and is not schema-version checked (deleting cannot create a stale
  tuple).
  (h) Writes retry up to 3 times on `SchemaChanged`; on a validation failure against the
  cached schema, the engine checks the store's version (one query) and revalidates against
  a newer schema before refusing, so a relation another process just added is accepted.
- Resolve by: human review
- Resolution: resolved in T3.1 by the human. (a), (b), (e)–(h) accepted. (c): runtime error
  codes have no `E_` prefix; the code is `SCHEMA_CHANGE_ORPHANS` and `03` §10 now says so.
  (d): accepted and documented in `04` §1 and `06` §1.1 (Rust) and §2.2 (Python):
  `Exact(r)` means "the current schema applied to the data as of `r`", not "what was
  allowed at `r`", with the removed-`banned` example; it is not an audit log.

## OQ-031 — Python binding details decided in T3.1
- Raised in: T3.1 (`docs/06-api.md` §1.1, §2.2, §2.3; ADR-014)
- Blocking: no
- Current choice:
  (a) `Error::code()` returns `TRANSACTION_CONFLICT` for `Store(StoreError::Conflict)`
  instead of `STORE_ERROR`, so "every code maps to exactly one class" holds with
  `TransactionConflictError` as its own class (a `StorageError` subclass). New
  `Error::CODES` lists every code; a core test fails to compile when a variant is added.
  (b) `Engine.migrate()` removed from §2.2, matching OQ-025 (d): with
  `auto_migrate=False` an unmigrated database fails in `Engine.sqlite` before an engine
  exists, so an instance method could never do the job. The deploy step is
  `Engine.sqlite(path)` with the default `auto_migrate=True`.
  (c) Classes not in the old §2.3: `NoSchemaError` (`NO_SCHEMA` had no class),
  `StaleRelationshipError`, `SchemaChangeOrphansError` (`.orphans`), and
  `BATCH_TOO_LARGE` under `InvalidRequestError`. `PembaError.retryable` mirrors
  `StoreError::is_retryable()`. The classes are Python (`pemba/_errors.py`), chosen by
  code in Rust, so they are ordinary subclassable exceptions.
  (d) `**consistency` spelled out as keyword-only `at_least` / `at_exactly` everywhere, so
  `mypy --strict` checks it. `Engine.sqlite` takes `str | os.PathLike[str]`.
  `delete_relationships` gains `limit`. Malformed tuple/object strings raise `ValueError`.
  Filters use `subject_relation=""` for "no subject relation".
  (e) `write(conn=...)` is accepted by the signature but raises `NotImplementedError`
  until T3.2.
  (f) `Revision` is a Python `int` subclass (PyO3 cannot subclass `int`); results build it
  on access.
- Resolve by: human review
- Resolution: (fill in)

## OQ-032 — Two SQLite copies in one process: native connections vs the app's `sqlite3`
- Raised in: T3.2 phase 0 (ADR-006, `docs/05-storage.md` §4.2 and §10, ADR-014;
  <https://www.sqlite.org/howtocorrupt.html> §2.2–2.3)
- Blocking: **yes**, for T3.2 and for 0.1
- Problem: ADR-006 says reads use the engine's own connections. In `pemba-py` those are
  `rusqlite` with `bundled` SQLite, while the app's `sqlite3` module uses CPython's own copy.
  On POSIX, SQLite works around `close()` dropping every `fcntl` lock the process holds on
  a file by keeping a process-wide list of open files. Each copy has its own list, so a
  close by one copy silently drops the other copy's locks. Foreign mode puts both copies on
  one file in one process by design.
- Test (`crates/pemba-py/probes/sqlite_copies/run_probe.py`, deterministic, no timing):
  a bundled-copy connection (built like `pemba-py`: rusqlite 0.40.2, SQLite 3.53.2) holds
  `BEGIN IMMEDIATE` plus an uncommitted insert on a WAL database. A separate process's
  `BEGIN IMMEDIATE` (busy timeout 0) fails, as it should. Then in the same process, Python
  runs `sqlite3.connect(f)`, one `SELECT` and `close()`, and the separate process tries
  again. **Result: the bug is real.** The separate process gets the write lock and commits,
  and the bundled connection commits too, believing it held the lock throughout. Python's
  last close also checkpoints and **deletes `-wal` and `-shm`** under the open writer: it
  takes the EXCLUSIVE lock, which never conflicts within one process. Afterwards
  `PRAGMA integrity_check` says `ok`, but **the other process's committed row is gone**.
  That is silent loss of a committed transaction, worse than a crash. Same result:
  - locally: WSL2 Ubuntu 26.04 with distro Python 3.14.4 (dynamic
    `/usr/lib/x86_64-linux-gnu/libsqlite3.so.0.8.6`, 3.46.1), and with uv-managed 3.11.17
    (SQLite 3.53.1 built into the interpreter, so 3.51's new close() defences on both sides
    do not help)
  - all four CI jobs (`.github/workflows/oq032-probe.yml`, PR #22). `ubuntu-latest`,
    setup-python 3.11.17 and 3.13.16: `_sqlite3` in `/opt/hostedtoolcache/...` loads the
    system `/usr/lib/x86_64-linux-gnu/libsqlite3.so.0.8.6` (3.45.1). `macos-latest` (arm64),
    python.org builds 3.11.9 (SQLite 3.45.1) and 3.13.16 (3.50.4): `_sqlite3` links only
    `libSystem`, so SQLite is compiled into it.
  - Controls: no in-process close, a close by the *same* (bundled) copy, and `connect()` +
    `close()` with no statement in between (CPython opens the WAL index lazily) all keep
    the lock. A raw `open()`/`close()` of `-shm` (no SQLite at all, §2.2) also loses it.
  - Not tested: Windows, which locks with `LockFileEx` per handle and does not have this
    mechanism. Also not tested: read snapshots losing their WAL read locks. Inferred: the
    same close drops them, so a checkpoint could overwrite pages an open `check` is still
    reading, giving wrong answers rather than errors.
- Option A, prototyped (`crates/pemba-py/probes/pystore`): a store whose every statement
  (reads, native writes, migration, GC) runs on Python `sqlite3` connections the engine
  owns, so only CPython's copy touches the file. It reuses `WriteTxn`, `SqlDialect`,
  `ReadStore`, `pemba_core::write` and the production SQL text, included verbatim from
  `pemba-store-sqlite/src/sql.rs`. **The store conformance suite passes (all 25 cases).**
  Check latency on `google_docs.yaml` (42 tuples, its 32 assertions, each timed from the
  Python call to its answer, release builds, WSL2 8 CPUs, Python 3.14.4 with SQLite
  3.46.1, 200 rounds; two runs agree within 5 %):

  | store | 1 thread p50 / p99 µs | 8 threads p50 / p99 µs | 8 threads checks/s |
  |---|---|---|---|
  | rusqlite store (today) | 36 / 120 | 327 / 1 440 | 19 500 |
  | A: GIL re-taken per statement | 100 / 300 | 40 500 / 85 000 | 190 |
  | A: GIL held for the whole check, pool 8 | 96 / 330 | 51 000 / 114 000 | 157 |
  | A: checks serialised by a Rust mutex, GIL held | 97 / 300 | 138 / 620 | 6 700 |

  Single-threaded, A costs about 2.7× (about 60 µs more per check, roughly 10 statements
  that each cross into Python). Unserialised, it collapses under threads: `sqlite3`
  releases the GIL inside every `execute`, so 8 threads convoy on it. A 0.1 ms
  `sys.setswitchinterval` changes this little (294 checks/s). Holding the GIL for a whole
  check also **deadlocks** once threads outnumber pooled connections: a thread waiting
  for the pool holds the GIL that a thread with a connection needs. Serialising requests
  (take a Rust mutex without the GIL, then hold the GIL for the whole request) gives
  6 700 checks/s with p99 below the native store's. The parallelism lost does not matter,
  because the GIL serialises the SQL anyway. Lookups were not measured.
- Option B, linking rusqlite against the SQLite `_sqlite3` uses. Assessed, not built:
  - Linux distro Python, and `actions/setup-python`'s Linux builds: `_sqlite3` loads the
    system `libsqlite3.so.0`. An extension linked dynamically to the same soname would
    share it. But a manylinux wheel may not depend on `libsqlite3.so.0` (not on the
    manylinux allow-list), and `auditwheel repair` would vendor a renamed copy, which is
    a second copy again. That leaves `dlopen`/`dlsym` of the already-loaded library at
    runtime, which needs hand-written `unsafe` FFI, and golden rule 5 forbids that.
  - python.org macOS builds: SQLite is compiled into `_sqlite3.so`. There is no library
    to link against, and its symbols are not a supported interface.
  - uv-managed (python-build-standalone): SQLite is built into the interpreter (`_sqlite3`
    is a built-in module). Same as macOS.
  - conda: `_sqlite3` loads `$CONDA_PREFIX/lib/libsqlite3.so`, a different file per
    environment. Only a conda-built package (not a PyPI wheel) could link to it.
  - Windows python.org: `DLLs/sqlite3.dll`. Linkable in principle, but Windows is not
    affected anyway.
  - Variant B′: `libsqlite3-sys`'s `loadable_extension` feature, with pemba loaded as a
    SQLite extension through the app's `conn.load_extension`. It uses the host's API
    table, so the copy is right by construction. But `enable_load_extension` is missing
    from several builds (e.g. macOS system Python), and it puts every call behind a
    function-pointer table.
  - Detecting a mismatch: equal version strings prove nothing. Two copies of 3.45.1 still
    corrupt. A real check must compare the *address* of a SQLite symbol in each copy
    (`dladdr`/`dl_iterate_phdr`, or `/proc/self/maps` on Linux), which is again `unsafe`,
    per-platform FFI. **B cannot be made reliable for PyPI wheels.**
- Rust-side variant: two rusqlite majors, each bundling SQLite, in one binary. Cargo
  already prevents this. `libsqlite3-sys` declares `links = "sqlite3"`, and Cargo refuses
  two packages with the same `links` key in one graph, so mismatched rusqlite and sqlx
  majors are a build error, not a second copy. The risk that remains is a C or `-sys`
  dependency that compiles its own `sqlite3.c` without that key (some embedded-DB and
  `cc`-built crates do), or a Rust host process that also loads a Python or other runtime
  with its own SQLite. `SqliteTxn` (`Engine::write_in`) is safe because it takes the
  caller's `rusqlite::Transaction` from the re-exported `rusqlite`, which is the same copy
  by construction. Document: "open the app's connection through
  `pemba_store_sqlite::rusqlite`".
- Current choice: resolved, see Resolution.
- Recommendation:
  1. **Option A for `pemba-py`'s SQLite store**, with requests serialised (one Rust mutex
     taken without the GIL, the GIL then held for the whole request, reader pool kept for
     snapshots). It is correct by construction on every platform, needs no `unsafe`, and
     conformance already passes. Cost: about 2.7× single-threaded latency (still about
     100 µs) and about a third of the rusqlite store's multi-thread throughput. For an
     in-process engine next to a Python web app, that is acceptable.
  2. Do not offer the rusqlite store in Python on a file any CPython `sqlite3` connection
     might open. Simplest: `Engine.sqlite()` always uses option A, and the native store is
     kept for Rust callers and the CLI. One driver per process means no warning to get
     wrong.
  3. Reject option B.
  4. Rust: add the `links` explanation and the "use the re-exported `rusqlite`" rule to
     `docs/05-storage.md` §4.2 and `docs/06-api.md` §1. Optionally add a debug-build check
     that `rusqlite::version()` equals the caller connection's `sqlite_version()`. That
     is necessary but not sufficient, so it is only a smoke alarm.
  5. Before 0.1, phase 1 must also measure lookups under option A, add the probe as a
     permanent regression test (running the real `pemba` package instead of the probe
     module, expecting `lock held`), and document it for gunicorn/threads users.
- Resolve by: human review of this entry and the draft ADR-006 amendment, then phase 1
- Resolution (human, T3.2a): **option A, with requests serialised and the GIL held per
  request; option B rejected.** ADR-006 amended accordingly. T3.2 is split into T3.2a
  (Python SQLite store, one SQLite copy) and T3.2b (foreign transaction mode).
  Implemented in T3.2a as `pemba-store-pysqlite`. `pemba-py` no longer depends on
  rusqlite, and CI fails if the built extension or the release wheel contains SQLite. The
  probe is now `crates/pemba-py/tests/test_one_sqlite.py`. It covers both directions,
  and the app-holds-the-lock test fails against the T3.1 rusqlite build. Floor: SQLite
  3.26.0 (`docs/05-storage.md` §4.3), tested on Rocky Linux 8. Details decided along the
  way are in OQ-033.

## OQ-033 — Python SQLite store details decided in T3.2a
- Raised in: T3.2a (ADR-006 as amended, OQ-032; `docs/05-storage.md` §4.3–§4.4,
  `docs/06-api.md` §2)
- Blocking: no
- Current choice:
  (a) Two new crates. `pemba-sqlite-common` is driver-free: SQL text, migration scripts
  and runner over `WriteTxn`, row decoding over `WriteTxn`, `SqliteDialect`,
  `SqliteConfig`, `MIN_SQLITE_VERSION`. `pemba-store-pysqlite` is the store over
  CPython's `sqlite3`. `pemba-store-sqlite` re-exports `SqliteConfig`/`Synchronous`, so
  its public paths are unchanged. Migration scripts moved to
  `crates/pemba-sqlite-common/migrations/` and run one statement at a time; a test pins
  the result to SQLite's own script runner. The migration runner reads `sqlite_master`,
  not its 3.33 alias `sqlite_schema`. The native store keeps its typed `rusqlite`
  decoding on the hot path; conformance and property 5 hold both stores to the same
  answers.
  (b) *Every* request on a SQLite engine is serialised (one per engine), not just
  `check`. Writes, lookups, GC and schema writes would otherwise deadlock the same way:
  a thread holding the GIL waits on the writer mutex while the mutex's holder waits for
  the GIL. The memory engine still releases the GIL.
  (c) The reader pool never blocks: with no idle connection, a request opens one, and at
  most `reader_pool` are kept. A blocking pool could deadlock a GIL holder. Under
  serialisation one request needs at most two connections (snapshot plus a store-level
  read).
  (d) `Engine.sqlite()` refuses (`StorageError`, `STORE_ERROR`, naming both versions) a
  `sqlite3` whose SQLite is older than `MIN_SQLITE_VERSION` or built without thread
  support (`sqlite3.threadsafety == 0`). Both are checked before the file is opened. New
  constant `pemba.MIN_SQLITE_VERSION` (`"3.26.0"`).
  (e) `auto_migrate=True` takes the write lock even when there is nothing to migrate,
  exactly as the native store does. App processes should open with
  `auto_migrate=False` (already the documented production advice), or `Engine.sqlite()`
  waits up to `busy_timeout_ms` behind an open app write transaction.
  (f) PyO3's deprecated `extension-module` feature is dropped. maturin sets
  `PYO3_BUILD_EXTENSION_MODULE`, and the feature would unify into
  `pemba-store-pysqlite`'s tests, which embed an interpreter and must link it. The
  workspace `test` CI job therefore installs Python 3.11 on all three OSes.
  (g) Property 5's harness moved into `pemba_testkit::store_equivalence`, so any store's
  tests can run it. `pemba-store-pysqlite` runs it (64 cases, and ≥ 1 000 with
  `--ignored`) along with the conformance suite.
  (h) A destructor that calls Python (closing connections) sets aside and restores any
  pending exception. Before this fix, a temporary `Engine` dropped while its own call was
  raising turned the exception into `SystemError`.
- Resolve by: human review
- Resolution: (fill in)

## OQ-034 — Foreign-transaction details decided in T3.2b
- Raised in: T3.2b (`docs/05-storage.md` §10, `docs/06-api.md` §2.1–§2.2)
- Blocking: no
- Current choice:
  (a) The same-file check is **not cached by `id(conn)`**, contrary to `05` §10 as first
  written. CPython reuses the `id()` of a collected object, so a new connection to
  another file could inherit a dropped connection's "verified" entry and the write would
  land in a database the engine never reads. The check (`PRAGMA database_list` on the
  app's connection plus one path canonicalisation) runs inside `Engine::write_in` on
  every write. Cheap next to the write itself, which is several statements plus the
  app's commit. If it ever matters, a cache keyed by a weak reference to the connection
  would be safe. `test_a_reused_connection_object_is_checked_every_time` pins this.
  (b) `conn` must be a `sqlite3.Connection` (a subclass is fine); anything else is a
  `TypeError`. That is stricter than "a DB-API 2.0 connection", but every other Python
  SQLite driver either brings its own SQLite (unsafe, `06` §2.5) or wraps `sqlite3`
  (pass its underlying connection, e.g. SQLAlchemy's `connection.driver_connection`).
  `conn=` on a memory engine, or `allow_autocommit=True` without `conn`, is a
  `ValueError`.
  (c) The autocommit guard is `conn.in_transaction is False`, whatever
  `isolation_level` is. A connection in `sqlite3`'s legacy mode (`isolation_level=""`
  or `"DEFERRED"`) with no open transaction would otherwise run the write in its own
  transaction (the write's `SAVEPOINT` opens one and its `RELEASE` commits it), not
  atomically with app data. With `allow_autocommit=True` that is exactly what happens.
  (d) Errors from the app's connection: unique violations and conflicts map as on the
  engine's own connections (`ALREADY_EXISTS`, `TRANSACTION_CONFLICT`, including
  `SQLITE_BUSY_SNAPSHOT` from a stale deferred snapshot); every other driver error is
  `StoreError::Foreign` (`StorageError`, `STORE_ERROR`, message "foreign connection
  error: ...").
  (e) Spec fix: the `06` §2.1 example imported `Consistency` from `pemba`, which the
  Python API never had (consistency is the `at_least` / `at_exactly` keywords, §2.2), so
  it could not run verbatim. The import was removed; the example is now in the README and
  runs in CI (`tests/test_readme_example.py`), which also checks the two copies match.
- Resolution: (a) resolved by the human on 2026-10-16 (after T3.2b): accepted. The
  same-file check is not cached by `id(conn)`, because CPython reuses the `id()` of a
  collected object; it runs on every foreign write. (b)–(e) stand as the current choice.

## OQ-035 — Release pipeline details decided in T3.3
- Raised in: T3.3 (ADR-014, `docs/09-repo-and-tooling.md` §1, §4, OQ-024)
- Blocking: no
- Current choice:
  (a) The fixture format and engine-only runner are the publishable crate
  `pemba-fixtures` (codename prefix like every other crate; renamed with the rest under
  ADR-012). The oracle hook is a trait pair, `Oracle` (`name`, `view`) and `OracleView`
  (`check`, `lookup_resources`, `lookup_subjects`); `OracleError::Declined` is a warning,
  `Failed` a failure, as `NotStratifiable` and other reference errors were before.
  `pemba_testkit::fixtures::{run, run_str}` keep their signatures and plug in
  `ReferenceOracle`, so every internal fixture test is still cross-checked.
  (b) `pemba test` therefore no longer prints the oracle's opinion: a failing assertion
  reads "engine says false", not "engine says false; reference evaluator says false". The
  one CLI test pinning that text was updated (resolution of OQ-024: "the published CLI
  runs fixtures against the engine alone").
  (c) `pemba-testkit`'s workspace entry is path-only (no `version`), so `cargo publish`
  strips it from published crates' dev-dependencies; `deny.toml` sets
  `allow-wildcard-paths = true`, which (checked) still fails a published crate that
  depends on it normally.
  (d) Publishable: `pemba-schema`, `pemba-core`, `pemba-store-memory`,
  `pemba-sqlite-common`, `pemba-store-sqlite`, `pemba`, `pemba-fixtures`,
  `pemba-cli`. Not: `pemba-testkit`, `pemba-difftest`, `pemba-store-pysqlite`,
  `pemba-py` (PyPI only). Manifests have no `repository` / `documentation` yet (cargo
  warns); left for the human with the final name.
  (e) Wheel smoke tests run on runners assumed available to a private repository:
  manylinux aarch64 under QEMU (`python:*-slim-bookworm` arm64 container) and macOS
  x86_64 as an x86_64 CPython under Rosetta 2 on the Apple-silicon runner, rather than
  native arm64 Linux / Intel macOS runners. Each runs the whole pytest suite against the
  installed wheel on CPython 3.11 (abi3 floor) and 3.14.
  (f) manylinux 2_17 (manylinux2014) for both Linux wheels: the oldest glibc maturin and
  Rust support; the Rocky Linux 8 job (glibc 2.28) covers an old real system.
- Resolve by: human review
- Resolution: (fill in)

## OQ-036 — Comment placement in the formatter (T3.8)
- Raised in: T3.8 (`docs/03-schema-language.md` §9.1, `docs/06-api.md` §3, OQ-013)
- Blocking: no
- Current choice:
  (a) Comments are anchored to items, not kept at byte positions: own-line comments lead
  the next member/definition, same-line comments trail the line's member, `{` or `}`.
  The canonical printer then reorders members and the comments go with them. A comment
  *inside* a member or definition header (e.g. `viewer + /* x */ owner`) cannot keep its
  place, because the printer rebuilds expressions from the IR, so it moves to its own
  line before that member. It is never dropped.
  (b) A dangling doc comment inside a definition is printed just before `}`, wherever it
  was. Printing it before a member would attach it as that member's doc and change the
  schema hash. One at top level can only be at the end of the file and stays there.
  (c) The formatter re-compiles its output and compares the canonical form and the
  multiset of comment texts before returning. Any difference is
  `FormatError::NotPreserved` (CLI exit 2, nothing printed): fail closed rather than
  print a schema that lost a comment or changed meaning. A property test inserts up to
  12 comments at random whitespace in 512 generated schemas per run (20 000 once,
  locally) and requires output, idempotence and every comment exactly once.
  (d) Inside bodies, comments add no blank lines (§9's "no other blank lines" holds).
  Outside definitions, a blank line before or after a comment in the source is kept as
  exactly one.
  (e) `hash` is unchanged: it still digests `to_canonical_string()`, which prints no
  plain comments, so comments never change a stored schema's version.
  (f) `schema fmt` keeps its "experimental" label and still only prints to stdout. A
  `--write` flag is not added.
- Resolve by: human review
- Resolution: (fill in)

## OQ-037 — Differential-test details decided in T3.4
- Raised in: T3.4 (`docs/07-testing.md` §4, `crates/pemba-difftest/KNOWN_DIFFERENCES.md`)
- Blocking: no
- Current choice:
  (a) pemba keeps accepting schemas where a wildcard is reached through a subject set,
  which SpiceDB refuses (KD-2). pemba's rule for them is KD-3's (a wildcard matches only
  objects). SpiceDB's refusal is itself nondeterministic: the harness writes every schema
  to 12 throwaway datastores and counts any refusal as KD-2. The minimal repro is in
  `docs/upstream/spicedb-kd2.md` for the human to file.
  (b) Generated cases are compared without cycles and with at most 16 relationships,
  because SpiceDB exhausts its depth limit on cyclic data and ran out of memory on dense
  acyclic data. Cyclic and larger data are compared only through the fixtures.
  (c) KD-3: a wildcard never matches a subject set (`04` §3.3), unchanged on the grant side.
  On the subtracted side pemba allowed what SpiceDB denies. Decided by the human on
  2026-10-16: fail closed with a new error. Done in T3.4b
  (`AMBIGUOUS_WILDCARD_EXCLUSION`, `04` §3.3).
  (f) T3.4b details: "under a subtracted side" means at least one, not an odd number. Under
  two subtracted sides the wildcard is back on a granting path, where pemba's answer is
  the stricter one, but the error is still raised: it is the simpler rule and fails closed.
  The probe runs after the exact-tuple hit (if the subject set itself is stored, the
  answer is `true` either way). It runs only for relations whose schema allows `t:*`; a
  stale `t:*` below the schema's revision is not probed for subject sets, as before.
  (d) KD-4 (depth limit near 50) was seen once and not reproduced; SpiceDB's answer there
  depends on timing. pemba's behaviour is kept and pinned by `depth_limit.yaml`.
  (e) `difftest.yml` pins `authzed/spicedb:v1.56.2`. Moving to a new SpiceDB release is a
  deliberate change that re-runs the comparison and updates `KNOWN_DIFFERENCES.md`.
- Resolve by: human review of `KNOWN_DIFFERENCES.md`
- Resolution: (fill in)

## OQ-038 — Benchmark details decided in T3.5
- Raised in: T3.5 (`docs/07-testing.md` §7)
- Blocking: no
- Current choice:
  (a) One definition of the datasets, in Rust (`benches/src/lib.rs`). The `bench-data`
  binary writes them to files that the Python and SpiceDB benches read, so the three
  cannot drift apart. Every scenario checks its answer before anything is timed.
  (b) p50/p99 per operation: Criterion only gives per-sample means, so after each
  Criterion run a separate pass times 2 000 single calls (after 200 warm-up) and
  `benches/results.py` takes percentiles of those. pytest-benchmark runs one call per round
  (`iterations=1`) and saves every duration (`--benchmark-save-data`). The SpiceDB script
  times single calls. Timer overhead (tens of ns) is included in every figure.
  (c) Added `lookup_resources_10k_pages_100`, the same lookup in pages of 100, so OQ-028
  (d) can be answered by comparing it with the one-page figure. Not answered yet: no
  benchmark has been run.
  (d) `write_batch_*` writes relationships that no earlier write made, so every write is
  an insert and the store grows during the run. The SQLite store uses `SqliteConfig`
  defaults.
  (e) SpiceDB is measured over its HTTP gateway (JSON) on one kept-alive connection,
  fully consistent by default (matching `Consistency::Latest`), on `serve-testing`'s
  in-memory datastore. The closest pemba figure is therefore the memory store. gRPC is
  not measured: it would need a gRPC client dependency for a comparison script.
  (f) `RESULTS.md` and `RESULTS.json` (summary statistics) are committed; the raw
  per-call durations stay in `target/` because they are megabytes per run. The
  generating commands are in `RESULTS.md`.
- Resolve by: human review after the first full benchmark run
- Resolution: (fill in)

## OQ-039 — Fuzzing details decided in T3.6
- Raised in: T3.6 (`docs/07-testing.md` §5)
- Blocking: no
- Current choice:
  (a) `fuzz_check_vs_reference` does not use an `arbitrary`-derived case as §5 says. It
  runs `pemba-testkit`'s construct-valid proptest generators (§2.2, the ones property 1
  uses) on a ChaCha RNG seeded from a hash of the input. Every input is therefore a valid,
  stratifiable case with property 1's restrictions, and the check is property 1's (single
  checks and `check_bulk`, with the one accepted error). The first byte picks
  `SubSides::Acyclic` or `SubSides::Recursive`.
  Cost: a hash gives libFuzzer almost no structure to steer by, so the target is in effect
  an open-ended property-1 run under AddressSanitizer, not a coverage-guided search.
  Proptest's pass-through RNG (input bytes as the random stream) was tried first and hangs.
  It gives each child RNG half of the parent's remaining bytes, so nested strategies drain
  the stream to zeros, and rand's range sampling then rejects every draw forever. Padding
  the input does not help, because each split halves what is left. A structure-aware
  version needs an `arbitrary`-derived case model resolved like `generate::resolve`.
  That is a second generator to keep in step with the first: deferred, human to decide.
  (b) `fuzz/` is its own Cargo workspace (it needs nightly; the main workspace builds on
  the pinned stable toolchain), with its own `Cargo.lock`. Build with
  `cargo +nightly fuzz build`.
  (c) Seed corpora come from `fuzz/seed_corpus.py`: the schemas and relationships in
  `testdata/`, tuple edge cases, and 32 fixed pseudo-random inputs for the third target.
  The inputs libFuzzer adds during a run are committed afterwards, as §5 asks.
  (d) Done on 2026-10-19 (pre-release sweep): each target ran 600 s with no crash, timeout or
  slow unit (`fuzz_schema_parse` 3.37 M executions, `fuzz_tuple_parse` 35.3 M,
  `fuzz_check_vs_reference` 53.9 k), and the inputs libFuzzer added are committed. The
  generator hang described in (a) did not reappear in those 53.9 k runs under a 60 s
  per-input limit. `cargo-mutants` on `eval.rs` and `compile.rs`: 202 mutants, 0 surviving,
  recorded in `docs/MUTANTS.md`. Still not written: `fuzz.yml` (09 §1) and a weekly
  `cargo-mutants` job; both run from `scripts/overnight.ps1` for now.
- Resolve by: human review after the first 10-minute runs
- Resolution: (fill in)

## OQ-040 — Rust and Python write benchmarks measure different table sizes
- Raised in: T3.5 benchmarks, pre-release sweep (2026-10-19)
- Blocking: no
- Current choice: `benches/RESULTS.md` says the `write_batch_*` figures are not comparable
  across languages. Criterion runs `write_batch_*` thousands of times (3,360 iterations for
  memory on the benchmark machine), each writing a fresh batch into the same database, and
  the Rust latency pass then runs on that grown database; pytest-benchmark writes 30
  batches. Same SQL and pragmas, different table sizes: SQLite `write_batch_1000` p50 was
  51.7 ms in Rust and 7.0 ms in Python, while `write_batch_100` was close (2.1 ms and 1.8 ms).
- Resolve by: make both measure the same table size (a fixed number of batches per sample,
  or a fresh database per sample), then compare
- Resolution: (fill in)

## OQ-041 — `Span::new` accepts an inverted span
- Raised in: pre-release sweep (2026-10-19), cargo-mutants on `compile.rs`
- Blocking: no
- Current choice: `pemba_schema::Span::new(start, end)` (public, `const fn`) builds any
  span, including `start > end`. A mutant of `ident_error` produced `Span::new(57, 56)` and
  nothing objected; the diagnostic renderer drew it as a one-column caret, so only a test
  asserting the span itself (`ident_format_highlights_exactly_the_offending_character`)
  caught it.
- Resolve by: after 0.1.0-alpha.1, add `debug_assert!(start <= end)` to `Span::new` (allowed
  in a `const fn`), so an inverted span fails in tests and debug builds without changing
  release behaviour; run the schema test suite and the fuzz targets with it
- Resolution: (fill in)
