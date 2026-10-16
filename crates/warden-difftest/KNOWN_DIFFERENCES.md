# Known differences between warden and SpiceDB

The differential test (`docs/07-testing.md` §4, T3.4) compares warden with SpiceDB on every
fixture and on at least 500 generated cases. A mismatch fails the run unless one of the
entries below explains it. Each entry has a detector in `src/known.rs`, and every detector
needs the exact shape of the disagreement plus a property of the case, so it cannot hide
an unrelated bug.

Compared against SpiceDB **1.56.2** (`spicedb serve-testing`, HTTP gateway), 2026-10-16:

| Run | Cases | Compared | Questions | Explained mismatches | Unexplained |
|---|---|---|---|---|---|
| Fixtures | 18 | 18 | 23 577 | KD-1: 85; KD-3: 3 | 0 |
| Generated | 665 | 500 | 499 517 | KD-2: 165 cases not compared; KD-3: 1 656 | 0 |

Fixtures that expect schema or write errors (`schema_errors/`) are not run here. warden
and SpiceDB word their errors differently, so two errors count as agreement.

## Caps on generated cases

SpiceDB cannot run every case the property-test generator produces, so generated cases
are restricted before they are compared:

- **No cycles in the data.** Each tuple that would close a cycle between objects is
  dropped (`without_cycles`). On cyclic data SpiceDB dispatches until its depth limit and
  explores paths exponentially (KD-1). Cyclic data is still compared through the fixtures
  (`nested_groups_cycle.yaml`, `non_monotone_cycle.yaml`).
- **At most 16 relationships per case** (`MAX_GENERATED_TUPLES`), after cycle removal.
  SpiceDB does not memoise across branches of one check. On the generator's densest
  acyclic data (re-converging subject sets under intersections), one bulk check used up
  2.5 GB and `serve-testing` was OOM-killed on a 4 GB machine. Larger data is compared
  through the fixtures.
- **One test at a time.** `serve-testing` keeps every datastore in memory; two runs at
  once more than doubled its peak memory.

warden's own property tests (`07` §2) have none of these caps.

## KD-1: cyclic data

- **Shape:** warden answers; SpiceDB fails with `max depth exceeded`; the data has a cycle
  between objects.
- **Why:** warden evaluates cycles to a fixed point (ADR-009, `04` §4), so a group that
  contains itself is a finite question. SpiceDB keeps dispatching around the cycle until
  it reaches its depth limit and reports an error.
- **Minimal repro:** `group:a#member@group:a#member`, then check
  `group:a#member@user:x`: warden `false`; SpiceDB `max depth exceeded`.
- **Seen in:** `nested_groups_cycle.yaml`, `non_monotone_cycle.yaml` (85 questions).
- **Decision:** keep warden's behaviour. It is the documented fixed-point semantics, and
  an error from SpiceDB is not an answer warden could match.

## KD-2: SpiceDB refuses wildcards reached through a subject set or an arrow, nondeterministically

- **Shape:** SpiceDB's schema write fails with "wildcard relations cannot be transitively
  included" or "wildcard relations cannot be used on the left side of arrows"; warden
  accepts the schema. The case is counted, not compared.
- **Why:** SpiceDB forbids a relation that allows `t:*` from being reached through a
  subject set (`relation viewer: group#member` where `member: user:*`) and, through the
  left side of an arrow, from a relation that allows a wildcard. warden refuses the arrow
  case too (`E_ARROW_OVER_WILDCARD`, `03` §4.2, OQ-006), but accepts a wildcard reached
  through a subject set. There a wildcard matches only concrete objects of its type
  (`04` §3.3, KD-3).
- **Nondeterminism:** for some schemas SpiceDB's check passes on some writes and fails on
  others. The minimal case (`docs/upstream/spicedb-kd2.md`) was accepted 28 times in 200
  writes on `serve-testing` and 10 in 100 on `serve`. The harness therefore writes each
  schema to 12 throwaway datastores first (`SCHEMA_PROBES`), and a refusal on any of them
  counts as KD-2. Such a schema slips through all 13 writes about once in 150 000 cases.
  It then shows up as an unexplained mismatch, never as a silent pass.
- **Decision:** keep accepting. Each relationship SpiceDB would refuse here is one warden
  evaluates under KD-3's rule. The nondeterminism is SpiceDB's bug, written up for the
  human to file upstream in `docs/upstream/spicedb-kd2.md`.

## KD-3: a wildcard and a subject-set subject

- **Shape:** a `check` whose subject is a subject set `t:x#r`; the case stores at least
  one `t:*` tuple; the two engines give opposite booleans; asked again with the case's
  `t:*` tuples removed, the two engines agree.
- **Why:** in warden a stored `t:*` matches every **object** of type `t` and never a
  subject set (`04` §3.3). SpiceDB matches a subject set `t:x#r` against `t:*` as well.
  This happens in both directions:
  - **Grant side:** warden `false`, SpiceDB `true`.
    ```
    definition user {}
    definition grp { relation member: user }
    definition doc { relation viewer: grp:* | grp#member }

    doc:d#viewer@grp:*
    check doc:d#viewer@grp:g#member   -> warden false, SpiceDB true
    ```
  - **Subtracted side:** warden `true`, SpiceDB `false`. The wildcard on the subtracted
    side of an exclusion removes nothing for warden, so warden allows what SpiceDB
    denies.
    ```
    definition user {}
    definition grp { relation member: user }
    definition doc {
        relation viewer: grp#member
        relation banned: grp:*
        permission view = viewer - banned
    }

    doc:d#viewer@grp:g#member
    doc:d#banned@grp:*
    check doc:d#view@grp:g#member     -> warden true, SpiceDB false
    ```
  Both repros are the fixture `testdata/fixtures/wildcard_subject_set.yaml`.
- **Seen in:** 1 656 generated questions and the 3 in `wildcard_subject_set.yaml`, every
  one recognised by the re-ask.
- **Decision:** the grant side stays as it is: a wildcard never matches a subject set.
  The subtracted side is the fail-open direction. Next task, T3.4b: replace that answer
  with the error `AMBIGUOUS_WILDCARD_EXCLUSION` (OQ-037 (c)).

## KD-4: the depth limit, near 50

- **Shape:** one engine answers and the other fails with a depth error; the data is
  acyclic and has a chain of at least 20 objects.
- **Why:** both engines stop at depth 50, but they count it and order their work
  differently. warden evaluates left to right and stops at the first operand that grants
  (`04` §3.4), counting depth as in `04` §3.7 and OQ-019 (b). SpiceDB dispatches branches
  concurrently and can fail on a branch past its limit before another branch grants.
- **Seen in:** `depth_limit.yaml` in the overnight run of 2026-10-16:
  `check folder:f59#view@user:near`, warden `true` (granted at depth 3), SpiceDB
  `max depth exceeded`. Not seen in the later run that day, so on SpiceDB's side it
  depends on timing.
- **Decision:** keep warden's behaviour. It is deterministic and pinned by
  `depth_limit.yaml`.

## Checked and agreeing (no entry needed)

These open questions were to be settled by this comparison (`docs/OPEN_QUESTIONS.md`).
SpiceDB 1.56.2 agrees with warden on each:

- **OQ-004** `.all()` over zero related objects: `false` in both
  (`doc:none#approved@user:anne` with no `approver_group`).
- **OQ-005** self membership: `group:empty#member@group:empty#member` is `true` with no
  tuples, `group:x#member@group:y#member` is `false`, and
  `group:empty#is_member@group:empty#member` is `true` (`is_member = member`).
- **OQ-006** arrows over a relation that allows a wildcard: SpiceDB refuses the schema
  (60 of 60 writes), as warden does (`E_ARROW_OVER_WILDCARD`).
- **OQ-007** type names: the same 11 names accepted or refused by both. These covered
  segments of 2, 3, 64 and 65 characters, several prefixes, total lengths of 128 and
  129 bytes, and a segment ending in `_`.
