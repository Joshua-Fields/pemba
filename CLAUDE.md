# CLAUDE.md — Agent entry point

You are building **pemba** (see `docs/01-decisions.md` ADR-012), an
**embedded, Zanzibar-style authorisation engine written in Rust** with Python bindings.
It runs inside the application process and stores relationships in a database the
application already has (SQLite first, Postgres later).

This file is the entry point. Read it fully at the start of every session. Then read only
the spec files the current task needs (see "Which docs to read").

---

## 1. What we are building, in five sentences

1. Applications define a **schema** (object types, relations, permissions) in a small DSL
   that is a strict subset of SpiceDB's schema language.
2. Applications write **relationship tuples** such as `document:readme#viewer@user:anne`.
3. Applications ask **questions**: `check` (can Anne view the readme?),
   `lookup_resources` (which documents can Anne view?), `lookup_subjects`
   (which users can view the readme?).
4. Answers are computed by walking the compiled schema over stored tuples, at a specific
   **revision**, so results are consistent and read-after-write safe.
5. Because the engine is embedded, relationship writes can **join the application's own
   database transaction**, which eliminates the dual-write problem that service-based
   engines have. This is the project's headline feature.

---

## 2. Golden rules (non-negotiable)

1. **Correctness over speed.** Authorisation bugs are security bugs. Never ship an
   optimisation without a test proving it agrees with the reference evaluator
   (`docs/07-testing.md` §2).
2. **Do not copy or translate SpiceDB source code.** SpiceDB (Apache-2.0) is a design
   reference only. Read its docs and behaviour; write original Rust. If you believe a
   piece genuinely must be adapted from SpiceDB code, stop and ask the human first —
   attribution rules apply (`docs/01-decisions.md` ADR-011).
3. **The spec is the source of truth.** If the spec is ambiguous or wrong, do not guess
   silently: write the question into `docs/OPEN_QUESTIONS.md` (create it if missing),
   pick the most conservative option, and say so in your summary.
4. **Fail closed.** Any error during evaluation (store error, depth exceeded, unknown
   relation at runtime) returns an `Err`, never `Ok(false)` and never `Ok(true)`.
   Callers decide what an error means; the engine never silently denies or allows.
5. **No `unsafe`** (`#![forbid(unsafe_code)]` in every crate except `pemba-py`, where
   PyO3 macros need it — and even there, write none by hand).
6. **No `unwrap()` / `expect()` / `panic!` in library code.** Tests and benches may use
   them. Use typed errors (`thiserror`).
7. **Every public item has a doc comment.** `#![warn(missing_docs)]` on library crates.
8. **Small steps.** One task from `docs/08-milestones.md` at a time. Each task ends with
   green CI locally (see §5) and a short summary of what changed.
9. **Do not add dependencies casually.** Each new crate dependency needs a one-line
   justification in the PR/summary. Prefer the approved list in `docs/09-repo-and-tooling.md`.
   Check the current latest version on crates.io rather than pinning from memory.
10. **Do not change public APIs or on-disk formats** defined in the spec without updating
    the spec in the same change and flagging it in your summary.

---

## 3. Which docs to read

| Task area | Read |
|---|---|
| Anything, first time | `docs/00-overview.md`, `docs/01-decisions.md` |
| Core types, tuple strings, revisions | `docs/02-data-model.md` |
| Lexer, parser, schema validation, IR | `docs/03-schema-language.md` |
| `check`, `lookup_resources`, `lookup_subjects` | `docs/04-evaluation.md` (+ `02`) |
| Store traits, SQLite, transactions, GC | `docs/05-storage.md` (+ `02`) |
| Rust `Engine` API, Python bindings, errors | `docs/06-api.md` |
| Tests, fixtures, proptest, differential, benches | `docs/07-testing.md` |
| What to build next, acceptance criteria | `docs/08-milestones.md` |
| Repo layout, CI, lints, dependencies | `docs/09-repo-and-tooling.md` |
| Postgres store design (proposal) | `docs/10-postgres.md` |
| Glossary of terms | `docs/00-overview.md` §6 |

When two docs disagree, the more specific one wins
(`04` beats `00` on evaluation, `05` beats `02` on storage) and you log the conflict in
`docs/OPEN_QUESTIONS.md`.

---

## 4. Workflow for every task

1. Find the task in `docs/08-milestones.md`. Re-read its acceptance criteria.
2. Read the referenced spec sections.
3. Write tests first where the spec gives exact behaviour (parser cases, check scenarios,
   SQL semantics). Fixture scenarios live in `testdata/fixtures/` (`docs/07-testing.md` §3).
4. Implement.
5. Run the full local check (§5). Fix everything.
6. Summarise: what changed, which acceptance criteria are met, anything deferred,
   any new entries in `docs/OPEN_QUESTIONS.md`.

---

## 5. Local "green" definition

All of these must pass before a task is considered done:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --all-features -- --ignored   # slow/property tests, when touched
cargo doc --workspace --no-deps                       # no broken intra-doc links
# fuzz/ is its own workspace, so `--all` / `--workspace` above do not reach it:
(cd fuzz && cargo fmt -- --check)
(cd fuzz && cargo check --all-targets)
# Python (in a virtualenv with maturin, pytest and mypy installed):
cd crates/pemba-py && maturin develop && pytest -q && mypy --strict tests
```

While iterating, run only what you touched: `cargo test -q -p <crate>` and
`cargo clippy -p <crate> --all-targets`, plus the tests of crates that depend on it
when the change is to a public API (pemba-core → pemba, pemba-testkit, pemba-py).
Run the full check above once, before opening or updating a PR, not after every edit.

---

## 6. Things that are explicitly out of scope until told otherwise

Caveats / CEL, expiring relationships, `use typechecking`, `use self` / the `self` keyword,
Watch / changefeeds, gRPC/HTTP server mode, Node bindings, WASM, Postgres store,
cluster/distributed dispatch, SpiceDB wire-API compatibility.
The parser must **recognise** the SpiceDB syntax for caveats, `with expiration`, `use ...`
and `self`, and reject it with `E_UNSUPPORTED_FEATURE` rather than a generic parse error
(`docs/03-schema-language.md` §7).

---

## 7. Style

- Rust edition 2021 (or 2024 if the toolchain in `rust-toolchain.toml` supports it; do not
  switch editions mid-project without asking).
- Errors: one `Error` enum per crate, `#[non_exhaustive]`, converted at crate boundaries.
- Naming: `ObjectRef`, `SubjectRef`, `Relationship`, `Revision`, `Consistency` exactly as in
  `docs/02-data-model.md`. Do not invent synonyms.
- Logging/tracing: `tracing` crate, spans on `check`, `lookup_*`, `write`. No `println!`.
- Comments explain *why*, not *what*.


---

## 8. Context hygiene

Context is finite; tool output is the main cost.

- Run builds, tests and benches quietly with output to a file, then read only the tail or
  grep for failures: `cargo test -q --workspace 2>&1 > target/test.log; tail -40 target/test.log`.
  Same for clippy, pytest (`-q`) and cargo bench. Never print full logs into the conversation.
- Read spec files by section: grep for the heading, then read that range. Don't read
  OPEN_QUESTIONS.md or 05-storage.md whole.
- Delegate self-contained heavy work to a subagent and keep only its summary: benchmark
  A/B runs, difftest investigation, codebase-wide searches, external research.
- Don't poll CI in a loop. Push, open the PR, report the PR link, and stop unless told
  to wait. If you must wait, use one blocking `gh run watch --exit-status` and read only
  the final status.
- Summaries: lead with outcome and anything needing a human decision; at most ~25 lines.