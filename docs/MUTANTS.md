# Mutation testing

`cargo-mutants` on the evaluator and the schema validator (`docs/08-milestones.md` T3.6):
every surviving mutant is either killed by a test or justified here. Run by
`scripts/overnight.ps1` (steps `mutants pemba-core eval` and `mutants pemba-schema
validator`), each mutant against the whole workspace's tests.

## Current result

Run on 2026-10-19 at `d6f9fd3`, cargo-mutants 27.1.0, `--jobs 2 --minimum-test-timeout 120`.

| File | Mutants | Caught | Missed | Timeout | Unviable |
|---|---:|---:|---:|---:|---:|
| `crates/pemba-core/src/eval.rs` | 105 | 78 | 0 | 0 | 27 |
| `crates/pemba-schema/src/compile.rs` | 97 | 72 | 0 | 0 | 25 |

No surviving mutants, so nothing is justified below. Unviable mutants do not compile (for
example a `Default::default()` replacement for a type with no `Default`); they need no
review.

## Survivors killed

The first run (default timeouts) reported 0 missed and 18 timeouts. Re-run with room to
finish, 12 of the 18 were caught by a failing test and 6 passed every test. None hung. The
6 are killed by:

| Mutant | Gap | Killed by |
|---|---|---|
| `eval.rs:98:26` `+=` → `*=` in `EvalStats::absorb` | `check_bulk`'s span total of `tuples_read` was never asserted | `check_bulk_emits_span_with_stats_fields` (`crates/pemba/tests/check_tracing.rs`) |
| `eval.rs:143:9` `Memo::get_negated` → `None` | no test reused a value settled under a subtracted side (answers unchanged, only re-evaluation) | `stats_count_memo_hits_under_a_subtracted_side` (`crates/pemba/tests/check.rs`) |
| `eval.rs:633:47` `+` → `*` in `arrow_all` | every depth-limit test chained `->`, never `.all()` | `depth_limit_counts_all_arrow_steps` (`crates/pemba/tests/check.rs`) |
| `compile.rs:291:62` `+` → `-` in `ident_error` | snapshots draw a one-column caret for an empty or inverted span | `ident_format_highlights_exactly_the_offending_character` (`crates/pemba-schema/tests/compile_errors.rs`) |
| `compile.rs:291:62` `+` → `*` in `ident_error` | as above | as above |
| `compile.rs:699:40` `&&` → `\|\|` in `check_direct_cycles` | the two-cycle test used disjoint cycles | `a_cycle_reachable_from_another_is_reported_too` (`crates/pemba-schema/tests/compile_errors.rs`) |

## Why the timeout floor

cargo-mutants sets its test timeout from an uncontended baseline (3 s, giving 20 s). A
caught mutant stops at its first failing test binary; a surviving one runs them all while
the other job competes for the CPU, and took 52-75 s. Under 20 s every survivor was
reported as a timeout, so all 6 were hidden. `--minimum-test-timeout 120` lets them finish
and show as missed. A mutant that really hangs still times out (exit 3) and must be reviewed
here.
