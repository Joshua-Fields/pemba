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
- Resolution: (fill in)

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
- Resolution: (fill in)

## OQ-005 — Self membership semantics vs SpiceDB
- Raised in: T0.1 (`docs/04-evaluation.md` §2)
- Blocking: no
- Current choice: a subject set `o#m` is always a member of `S(o, m)`
- Resolve by: differential test in T3.4
- Resolution: (fill in)

## OQ-006 — Does SpiceDB accept arrows over relations that allow wildcards?
- Raised in: T0.1 (`docs/03-schema-language.md` §4.2)
- Blocking: no
- Current choice: reject (`E_ARROW_OVER_WILDCARD`), conservative
- Resolve by: differential test in T3.4
- Resolution: (fill in)

## OQ-007 — Type-prefix segment rules and maximum type name length
- Raised in: T0.1 (`docs/02-data-model.md` §1.1)
- Blocking: no
- Current choice: every `/`-separated segment obeys the 3–64 character identifier rule; full
  type name at most 128 bytes
- Resolve by: differential test in T3.4
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

## OQ-013 — `warden schema fmt` output target, comments, exit code for I/O errors
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

## OQ-014 — Store write path: typed `WritableStore`, SQL in `SqlStore`
- Raised in: T1.1 (`docs/05-storage.md` §2–3, §9)
- Blocking: no
- Current choice: as specified, `WritableStore` only had `with_write_txn` (SQL) and
  `dialect`, which the memory store cannot implement, and the traits had no way to store a
  schema version or run GC, although the conformance suite (§9) must test both through a
  generic store. `WritableStore` now has typed methods (`write`, `write_schema`, `gc`) that
  every store implements; `with_write_txn`/`dialect` moved to a sub-trait `SqlStore`, which
  the SQLite store will implement and back its `WritableStore` methods with the shared
  algorithm in `warden-core::write`. `Engine::write_in` requires `S: SqlStore`. Schema
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
  level in `crates/warden/tests/engine.rs` as well as in the conformance suite

## OQ-017 — Codes for relationship validation; validating filters
- Raised in: T1.2 (`docs/02-data-model.md` §6 named only `E_WRITE_TO_PERMISSION`;
  `docs/06-api.md` §1.1 types `InvalidRelationship.code` as `ErrorCode`)
- Blocking: no
- Current choice: (a) codes are new variants of `warden_schema::ErrorCode`, one namespace
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
  evaluator (`warden-testkit::reference`) and the proptest generators**: the oracle declines
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
  (`warden-testkit::reference`) stratifies by the `(type, member)` graph above, with every
  edge reached through a `sub` side negative; a component holding a negative edge returns
  `ReferenceError::NotStratifiable` instead of an answer; components are evaluated in
  dependency order, each from ∅ to a fixed point. Monotone recursion inside a `sub` side is
  already handled (it is an earlier component), so T1.4b needs no oracle change. `07` §2.1
  rewritten. Both variants of the example are fixed tests (`oq020_banned_direct`,
  `oq020_banned_one_hop_deeper` in `crates/warden-testkit/tests/reference.rs`), checking
  the oracle and the engine; generators put recursion on exclusion base sides

## OQ-021 — Exponential `check` cost on cyclic data (denial-of-service risk)
- Raised in: T1.4 (`docs/07-testing.md` §2.3, `docs/04-evaluation.md` §3.6)
- Blocking: yes, for the 0.1 gate (T3.7)
- Problem: §3.6 never memoises tainted results, so inside a cyclic region the evaluator
  re-walks every path: cost is exponential on dense cyclic data. Answers stay correct (every
  slow case examined agreed with the reference evaluator), but cyclic group data is
  ordinary data that users can write, so a user can make one `check` arbitrarily expensive:
  a denial-of-service risk. `max_depth` and `max_check_fanout` do not bound total work.
- Observed (property tests built with `opt-level = 3` for `warden-core`/`warden-testkit`):
  one 1 000-case run of `prop1_check_matches_reference_full` took 3.0 s; the next, with a
  different random seed, was still running after 573 s and was stopped (its slow case was
  never printed). Pinned case (case 96 of the deterministic runner, not shrunk): 2 types,
  30 tuples, relations allowing subject sets of the other type's permissions (dense mutual
  recursion over ids `a`..`f`). One `check` makes 32 726 store reads at depth 29 (~170 ms in
  release); the oracle takes 21 ms for the whole case. Time is dominated by a few such cases.
- Current choice: no engine change in T1.4. The case is
  `check_on_dense_cyclic_data_reads_a_bounded_number_of_tuples`
  (`crates/warden-testkit/tests/check_cost.rs`, ≤ 1 000 store reads), `#[ignore]`d and
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
  `EngineConfig::max_check_reads` (default 200 000, `CheckReadsExceeded`) is the fail-closed
  backstop. Every `check_cost.rs` test passes un-ignored (the pinned case now makes well
  under its 1 000-read bound); property 1 at 1 000 cases runs in ~1 s optimised
