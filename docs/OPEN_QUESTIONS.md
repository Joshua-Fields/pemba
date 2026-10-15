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
- Current choice: a doc comment before `}` or end of file, or inside a member (e.g. within an
  expression), is discarded like a plain comment; consecutive doc comments before one node are
  joined with `\n`. Losing documentation is not a correctness risk, and rejecting would refuse
  schemas SpiceDB accepts (ADR-004)
- Resolve by: human review; revisit when the pretty printer (T0.5) needs to round-trip comments
- Resolution: (fill in)

## OQ-011 — Whitespace inside type references
- Raised in: T0.3 (`docs/03-schema-language.md` §2–3)
- Blocking: no
- Current choice: whitespace is rejected around `/` in a type name (`docs / document` →
  `E_UNEXPECTED_TOKEN`), conservative. Whitespace around `#` and `:` in allowed types
  (`group # member`, `user : *`) is accepted, as the grammar treats them as separate tokens
- Resolve by: differential test in T3.4 (does SpiceDB accept each form?)
- Resolution: (fill in)
