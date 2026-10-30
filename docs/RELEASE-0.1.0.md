# 0.1.0 release evidence

This is the evidence index required by M7. No stable release is approved or published.
T7.2–T7.12 implementation/rehearsal/preparation evidence is recorded here. T7.13's
exact-candidate verification record is linked below; T7.14–T7.17 remain human/public
verification gates. T7.11 is merged; the stable release PR remains unmerged.

## Preceding gates

| Requirement | Source | Completed task / evidence | Revision / result |
|---|---|---|---|
| Ordered M7 plan | T7.0 | [PR #80](https://github.com/Joshua-Fields/pemba/pull/80), merged | main `fee1cac`; merged 2026-10-29 |
| Maintainer scope and publication decisions | T7.1; OQ-044, OQ-058–OQ-060 | [PR #81](https://github.com/Joshua-Fields/pemba/pull/81), merged | main `8852eba`; merged 2026-10-29 |
| Previous implementation PR merged | M7 ordering | [PR #78](https://github.com/Joshua-Fields/pemba/pull/78), alpha.4 release preparation | merged; earlier release baseline only, not new verification evidence |

T7.1 approves sync Postgres scope, only Rust `check_str`, the extensible-type audit,
VitePress routes/content policy, and best-effort owner-led security/triage. Private
vulnerability reporting activation/verification remains human work at its later gates.
PR #79 was open during initial T7.2 preparation. It subsequently merged as main
`9e2d894`; that main revision is incorporated into the T7.2 branch to resolve the
Unreleased changelog conflict, retaining both tasks' entries. Reassess deployment
integration against this merged state in T7.6/T7.11.

## T7.2 / T3.9 public API audit

Source: T3.9 all AC; T7.2 all AC; `06` §1–§2; `09` §3; OQ-044.
Implementation: this T7.2 PR on `t7.2-public-api-audit`. Production/API source commit:
[`ff21c0bda7b795d80ffaf755c8e753c05ed39505`](https://github.com/Joshua-Fields/pemba/commit/ff21c0bda7b795d80ffaf755c8e753c05ed39505).
Verification-only corrections (Python test annotation and fuzz formatting):
[`4dc21b721b9437077c6bc93bb738c3e9c0efb2e2`](https://github.com/Joshua-Fields/pemba/commit/4dc21b721b9437077c6bc93bb738c3e9c0efb2e2). [PUBLIC_API_AUDIT.md](PUBLIC_API_AUDIT.md) inventories
98 public structs/enums in all nine publishable crates, including facade/Postgres coverage.
Requests remain constructible downstream; custom stores retain producer constructors.
`check_str` and `Error::Parse` implement the approved helper; Python preserves ValueError.
Migration is in `06` and CHANGELOG. No storage format or package version changes.
OQ-061 records the conservative parse-code mapping clarification for review.

### Acceptance coverage

| Requirement | Evidence | Result |
|---|---|---|
| Every publishable public struct/enum has an explicit extension decision and reason | PUBLIC_API_AUDIT.md: 98 individual declarations; all 9 publishable manifests; macro-generated names, facade and separate Postgres coverage | Satisfied |
| Extensible inputs remain usable outside their defining crate | Constructors/defaults plus public field assignment; migrated facade, CLI, bindings, stores, fixtures, testkit, benches, fuzz and playground consumers | Satisfied; downstream integration tests and workspace checks pass |
| Changed construction/helper behavior has focused downstream coverage | `cargo test -p pemba --test public_api`; `cargo test -p pemba-core -p pemba --doc`; CheckRequest compile-fail doctest | Passed; allow/deny, revision selection, parse component/byte position and propagated errors |
| Spec, migration and binding error mapping agree | `06` §1–§2, CHANGELOG, OQ-044/OQ-061; Python ERROR_CODES mapping tests | Satisfied; Python signatures/stubs unchanged; malformed arguments remain ValueError |
| T3.9 all acceptance criteria | Inventory, constructors, spec/migration entries and implemented OQ-044 helper above | **T3.9 satisfied by this T7.2 implementation**; PR review/merge is still required |

### Local verification, 2026-10-29

Host: Windows x86_64; Rust 1.99.0; Python 3.12.10; Node 24.20.0;
wasm-bindgen 0.2.129. The full required local command suite ran once on `ff21c0b`.
The only subsequent non-documentation changes are the test annotation and fuzz rustfmt
corrections in `4dc21b7`; the affected checks were rerun, rather than repeating the full
suite. The production API/evaluator/store sources are identical in these commits.
All rows below are local command results, not claims about remote CI.

| Command / environment | Exact source | Result |
|---|---|---|
| `cargo fmt --all -- --check` | ff21c0b | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | ff21c0b | Passed |
| `cargo test --workspace --all-features` | ff21c0b | Passed: 685 tests/doctests; 18 slow tests intentionally deferred to next row |
| `cargo test --workspace --all-features -- --ignored` | ff21c0b | Passed: all 18 slow tests, including 1,000-case properties, store equivalence, Postgres GC measurement/revision-order and SpiceDB fixtures plus at least 500 generated comparisons |
| `cargo doc --workspace --no-deps` | ff21c0b | Passed, no warnings |
| `(cd fuzz && cargo fmt -- --check)` | 4dc21b7 | Passed after correcting formatting; initial ff21c0b formatting check failed |
| `(cd fuzz && cargo check --all-targets)` | ff21c0b | Passed; later fuzz change is formatting only |
| `(cd crates/pemba-py && maturin develop && pytest -q && mypy --strict tests)` in fresh tool-only venv | ff21c0b, with affected typing check on 4dc21b7 | Build passed; pytest 161 passed, 1 module skipped (Postgres drivers absent); strict mypy passed for 13 files after test annotation correction and using the crate working directory/config |
| `pytest -q tests/test_errors.py` in the same venv after annotation correction | 4dc21b7 | Passed: 29 tests |
| Package LICENSE/NOTICE byte comparison for every shipping package, plus root/Python README equality | 4dc21b7 | All copies match |

The isolated venv contains only maturin, pytest, mypy and their transitive dependencies.
The initial mypy invocation from the repository root did not load the Python crate's
existing Django override and failed; it also exposed the new test's too-narrow dictionary
annotation. Neither failure is called a pass. The correct crate-directory invocation
passes without installing optional driver packages in this venv. The skipped Postgres
module is covered separately below; its skip is not counted as a service-suite pass.

The Rust runs set `PEMBA_PG_URL` to the local PostgreSQL 18 service and
`PEMBA_PG_REQUIRED=1`. The slow run also set `PEMBA_SPICEDB_URL`,
`PEMBA_SPICEDB_KEY=difftest`, `PEMBA_DIFFTEST_REQUIRED=1`, and
`PEMBA_DIFFTEST_CASES=500` against local `authzed/spicedb:v1.56.2` in `serve-testing` mode.
Missing services therefore fail these runs. The generated comparison used the default
memory backend; Postgres conformance/equivalence and revision-order are separate checks,
not a claim of the later T7.11 Postgres differential/release matrix.

### WASM and playground

These runs exercise the same production sources as `ff21c0b`; the later Python annotation
and fuzz whitespace do not affect this dependency graph. The generated page footer was
built at `ff21c0b`. No deployment or publication was performed.

| Command | Source | Result |
|---|---|---|
| `cargo clippy --locked --target wasm32-unknown-unknown -p pemba-core -p pemba-schema -p pemba-store-memory -p pemba-fixtures -p pemba-playground -- -D warnings` | ff21c0b | Passed |
| `cargo clippy --locked --target wasm32-unknown-unknown -p pemba-fixtures --test wasm_smoke -- -D warnings` | ff21c0b | Passed |
| `cargo test --locked --target wasm32-unknown-unknown -p pemba-fixtures --test wasm_smoke` with wasm-bindgen-test-runner | ff21c0b | Passed: 4 tests under Node |
| `playground/conformance/run.sh` | ff21c0b | Passed: 1,000 generated cases agree with reference (12,499 checks, 3 expected check errors, 15,678 lookups); 1,037 fixtures total; native/WASM transcripts identical across 45,323 lines |
| `npm test` in playground/web | ff21c0b | Passed: 165 tests in 9 files |
| `npm audit --omit=dev` | ff21c0b | Passed: zero vulnerabilities |
| `npm run wasm`, `npm run build`, `npm run size` | ff21c0b | Passed; 578.2 KB total gzip, below 700 KB budget |
| `npm run e2e` | ff21c0b | Passed: 72 Chromium tests |

### Additional service checks

Local Docker services: PostgreSQL 15.19 and 18.6; PostgreSQL 18 TLS service with the
image's untrusted certificate. Rust commands used `PEMBA_PG_REQUIRED=1`; the Python runs
used `PEMBA_PG_URL` and `PEMBA_PG_TLS_URL` with all four drivers installed in a **separate**
venv. Python driver versions match ci.yml: psycopg 3.3.6, psycopg2-binary 2.9.13,
SQLAlchemy 2.1.6, Django 6.1.2 and greenlet 3.5.6.

| Command | Source | Result |
|---|---|---|
| `cargo test -p pemba-store-postgres --all-features` on PG15 | Production ff21c0b (unchanged in 4dc21b7) | Passed, including normal-role/conformance/equivalence/foreign-transaction paths; ignored measurement/order are not counted here |
| `cargo test -p pemba-store-postgres --all-features --test revision_order -- --ignored --nocapture` on PG15 | Same production source | Passed: commit-order oracle; PG18 order and GC measurement passed in full slow run |
| `cargo test -p pemba-store-postgres --no-default-features` on PG18 | Same production source | Passed: plaintext build; ignored suites are not counted here |
| `maturin develop`, then `pytest -q -rs tests/test_postgres.py tests/test_errors.py` against PG18 and PG15 | 4dc21b7 | Passed: 62 tests on each server, zero skips; all supported drivers, commit/rollback and untrusted-TLS-certificate rejection |
| `mypy --strict tests` from crates/pemba-py in driver venv | 4dc21b7 | Passed: 13 files; initial root-directory invocation failed to load existing Django config and is not counted as a pass |
| `python ci/check_tls.py` and `python ci/check_no_sqlite.py` on built extension | 4dc21b7 | Passed: rustls present, no OpenSSL, no bundled SQLite markers |

Local logs remain in ignored `target/t7.2-*`; generated build output and credentials are
not committed. Temporary service containers are cleaned up after these checks.

### CI and remaining approval boundaries

The T7.2 PR must run existing [CI](https://github.com/Joshua-Fields/pemba/actions/workflows/ci.yml)
and [Playground](https://github.com/Joshua-Fields/pemba/actions/workflows/playground.yml)
workflows. At preparation time these PR runs are **pending/not yet observed**: no remote
job is reported as passed. Linux/macOS, the oldest supported Python/SQLite job, packaging
dry runs and platform-only checks remain subject to those workflow results. No manual
wheel/release rehearsal is claimed; that belongs to the later ordered release tasks.

Review OQ-061's conservative `PARSE` → built-in `ValueError` mapping with this PR.
Human review/merge is required before T7.3; nothing here approves a release candidate,
activates security settings, merges a PR, changes a tag, publishes a package or deploys.
At T7.2 preparation, T7.3 and all later tasks had not started; see the T7.3 evidence
below for subsequent work.

### PR #82 merge-conflict refresh

Merged main `9e2d894` (PR #79) into the T7.2 branch. The only conflict was the
Unreleased section of CHANGELOG; both the T7.2 Breaking/Added entries and the merged
playground Fixed entry are retained. No Rust, Python, manifest, lockfile, fuzz, WASM or
conformance source differs from verified `4dc21b7`; earlier source evidence is retained.
The imported deployment changes are from main, and no deployment is executed here.
On the resolved tree, `npm test` passed 169 tests in 10 files, `npm run build` passed,
workspace and separate fuzz rustfmt checks passed, and `git diff --cached --check`
passed with no unmerged index entries. Full functional suites were not repeated for
this changelog resolution; remote CI must validate the refreshed PR tree.

## T7.3 public API documentation audit

Sources: T3.7 rustdoc/docstrings AC; T7.3 all AC; `06` §1–§3; `09` §3–§4;
[T7.2 public inventory](PUBLIC_API_AUDIT.md). Prerequisites: T7.0 and T7.1 merged
as recorded above; [T7.2 PR #82](https://github.com/Joshua-Fields/pemba/pull/82)
merged as `5e7e0d871e7dab546609e7dd43ebaed309d8306b`. Its
[CI](https://github.com/Joshua-Fields/pemba/actions/runs/40857197571),
[Playground](https://github.com/Joshua-Fields/pemba/actions/runs/40857197403) and
[differential workflow](https://github.com/Joshua-Fields/pemba/actions/runs/40857197559)
completed successfully. The user explicitly authorised continuing the audit while the
last Postgres differential job was running; it subsequently passed. Skipped deployment
jobs are not execution evidence. The recorded T7.1 decisions remain unchanged.

Implementation source: `ac2a2ec0049697fd2d95f67ddfa7f7b4afc665ee`; documentation lint,
integer-conversion wording and regression-test corrections:
`ce9389d656d15f19a2437d2e6ea8746775714013`. All runtime Rust/Python implementations,
APIs, versions, storage formats and dependencies remain unchanged. Subsequent changes
are specification clarification and this evidence index only.

### Audited coverage

This is a contract audit, in addition to warning checks. The T7.2 inventory's 98
structs/enums and facade/re-export map remain the type baseline; traits, functions,
constants, fields, constructors and examples were also reviewed in their modules.

| Surface | Coverage and result |
|---|---|
| `pemba` facade | Re-exports, quick start and explicit `memory`/`sqlite` features. Fixed three links that broke without optional stores; gated the memory doctest. The external quick start uses only facade imports and `model` types. |
| `pemba-core` engine/config/requests/results | Builder validation, schema, writes, foreign transactions, resolution, all reads, traces and GC. Added missing wildcard-exclusion error, empty-page termination, stricter stale-subject lookup behavior, engine-connection read isolation and SQLite/Postgres pending distinctions. Limits and fail-closed behavior retained. |
| `pemba-core` model/store/write modules | Name/tuple/revision parsing, consistency, filters, pending revisions, store traits, SQL dialect/transaction contracts, shared write functions/constants. Clarified `Exact`: current schema over historical data, not an audit log. Existing detailed trait/error/transaction docs retained. |
| `pemba-schema` | Compilation/formatting, diagnostics, identifiers, AST/IR, spans and indexes; existing item docs and examples retained. |
| `pemba-store-memory` | Construction, revision visibility, read/write trait implementation; existing docs retained. |
| `pemba-sqlite-common`, `pemba-store-sqlite` | Config, SQL/dialect/constants, migrations/reads, store open/read-only/migrate, `SqliteTxn` and upstream re-export; existing contracts retained. |
| `pemba-store-postgres` | Config, open/migrate, TLS/pool, `PgTxn`, dialect and upstream re-export. Corrected retry documentation to distinguish unique violations from backend errors. |
| `pemba-fixtures`, `pemba-cli` | Fixture loading/parsing/runners/oracle/report contracts; CLI commands, errors and exit semantics checked against `06` §3. No new public API or CLI behavior. |
| Python `Engine` (all 18 methods) | Constructors; schema write/read/validation; write/delete; check/detailed/trace/bulk; resources/iterator/subjects; relationship reads; head/GC. Built-extension help now covers shared consistency/errors/default limits, all supported transaction drivers, caller ownership, pending resolution/rollback, retries and pagination errors. Corrected bulk whole-call vs per-item errors. |
| Python argument/result/exception classes | All exports in `pemba.__all__`; 82 locally declared public methods/properties have runtime docs. Reviewed frozen relationship/precondition/results/trace/diagnostic fields, exception code/retryability, `Revision` and `PendingRevision`. `.revision()` now explains SQLite precommit knowledge versus committed visibility and Postgres resolution errors. |
| Python stubs/signatures | Native signatures inspected; all Engine overload keyword sets checked against the built extension. Restored the existing `allow_autocommit` keyword in the `conn=None` overload, removed the now-unnecessary test typing ignore, and aligned affected method descriptions. Negative/unrepresentable unsigned arguments raise built-in `OverflowError` before engine validation; runtime, stubs and `06` now agree. |
| Alpha.4 migration | Searched docs, examples, READMEs and Python/Rust sources for pending integer/index conversions. Runnable examples already use `.revision()` or `at_least=pending`; actual `Revision` integer formatting and intentional rejection tests remain valid. Updated ADR-015's stale transition wording to alpha.3-only, removed in alpha.4; retained historical OQ-045 decision text. No new product decision or open question. |
| docs.rs feature policy | Existing facade metadata `all-features = true` intentionally documents both stores. Other publishable crates use default features (including Postgres `tls`); no metadata change needed. Strict all-feature workspace documentation plus feature-disabled facade documentation build successfully. |

### Verification, 2026-10-29

Windows x86_64, pinned Rust 1.99.0, CPython 3.12.10, Node 24.20.0,
wasm-bindgen 0.2.129. The full §5 command sequence ran once, starting at `ac2a2ec`.
The initial formatting check found two CRLF files and Clippy found missing documentation
backticks. Both were corrected; final fmt and full workspace Clippy passed. Runtime
code did not change during verification. Final doc comments and the added limit-help
regression were rechecked at `ce9389d` using the affected checks below; the complete
Rust runtime suite was not repeated for doc-comment formatting. Initial failures are
not counted as passes.

| Command / evidence | Source and result |
|---|---|
| `cargo fmt --all -- --check` | Final tree `ce9389d`: passed after newline correction |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | `ce9389d`: passed after documentation lint corrections |
| `cargo test --workspace --all-features` | Runtime source `ac2a2ec` (unchanged through `ce9389d`): 685 tests/doctests passed; 18 ignored tests **not run**, not counted as passes. No slow/property implementation or tests touched, so §5's conditional ignored-suite rerun is not applicable. PG18 service present with `PEMBA_PG_REQUIRED=1`. |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features` | Final documentation `ce9389d`: passed |
| `cargo test --workspace --all-features --doc` | Final documentation `ce9389d`: all workspace doctests passed |
| Strict `cargo doc -p pemba --no-default-features --no-deps`; `cargo test -p pemba --no-default-features --doc` | `ce9389d`: passed; initial no-feature doc build failed on three links before the fix |
| External quick start | `ce9389d`: extracted actual facade crate-doc example into `target/t7.3-quickstart`, with its own `[workspace]` and `pemba` path dependency using `default-features=false, features=["memory"]`. `cargo run -q --manifest-path target/t7.3-quickstart/Cargo.toml` passed using published-facing imports; no claim of a registry install. |
| Separate fuzz workspace fmt / `cargo check --all-targets` | Passed; fuzz source unchanged |
| Fresh tool-only venv: `maturin develop`, `pytest -q -rs`, `mypy --strict tests` from `crates/pemba-py` | Built extension passed; final pytest: 165 passed, one Postgres module skipped because drivers are absent; strict mypy: 14 files passed. Skip is covered separately below and is not called a pass. Venv contains only maturin/pytest/mypy and their dependencies. |
| Built-extension documentation tests | `ce9389d`: 4 passed; inspect runtime exports/82 members, `pydoc` help, stub keyword parity and unsigned-limit exceptions. Initial tests exposed missing help/keyword coverage; added limit caveat test failed before documentation correction. Additional docs + foreign-transaction focused run: 20 passed. Help comes from native `_pemba.pyd`, not stubs alone. |
| wasm32 Clippy / smoke | `ce9389d`: main five playground-compatible crates and `wasm_smoke` Clippy passed with `-D warnings`; all 4 Node WASM smoke tests passed |
| `playground/conformance/run.sh` | `ce9389d`: 1,000 generated cases agree with reference (12,499 checks, 3 expected check errors, 15,678 lookups); 1,037 fixtures; native/WASM transcripts identical across 45,323 lines |
| Playground `npm test`; `npm audit --omit=dev` | Runtime/site sources unchanged: 169 tests across 10 files passed; zero shipping vulnerabilities |
| Playground WASM build, `npm run build`, `npm run size`, `npm run e2e` | `ce9389d`: passed; 578.2 KB gzip < 700 KB; 72 Chromium tests passed |
| Separate driver venv: `maturin develop`; `pytest -q -rs tests/test_postgres.py tests/test_errors.py tests/test_documentation.py` | `ce9389d`, PostgreSQL 18.6 plus untrusted TLS server, `PEMBA_PG_REQUIRED=1`: 66 passed, zero skips, including psycopg 3/psycopg2/SQLAlchemy/Django, transaction commit/rollback and TLS rejection |
| Driver-venv `mypy --strict tests`; `python ci/check_tls.py`; `python ci/check_no_sqlite.py` | `ce9389d`: passed (14 files); built extension has rustls/no OpenSSL and no bundled SQLite |

The clean venv's first mypy attempt found the documentation test using an untyped pydoc
helper and exposed an obsolete ignore after correcting the stub overload. Both test-only
issues were corrected before the full check. A scratch quick-start extraction initially
left blank Rust doc-comment markers in the generated file; correcting the extraction
made the actual example compile/run. No package or runtime change was needed.

Logs, native help output and the generated downstream crate are ignored in `target/t7.3-*`;
no credentials or build outputs are committed. The T7.3 PR's CI/platform/service and
Playground workflows are pending/not yet observed at preparation time. Local Windows
results do not claim those remote jobs, the oldest Python/SQLite combination or the
later release/wheel rehearsal have passed. No manual wheel run is required by unchanged
packaging in this task. No release, deployment, tag or security activation was performed.
Human review/merge is still required; T7.4 has not started.

## T7.4 installation, quick starts and user guides

Sources: T7.4 all AC; T3.7 README AC; ADR-006, ADR-014–ADR-015;
`05` §4.3–§4.4, §10; `06` §2.1, §2.5;
`10` §1.6.4, §1.6.7–§1.6.8, §2–§3.
Implementation source: `8331df4373f9bda88a8d81e19aaea926f097ba57`
on `t7.4-install-and-user-guides`; the following evidence commit changes only this index.
Runtime/API sources remain main `17bfd6756e3c3b5dcf61bddc6347b6b5bd7820d9`.
No new dependency, API, storage format, version, tag, upload or deployment.

### Preceding gates

T7.0–T7.2 merged as recorded above. [T7.3 PR #83](https://github.com/Joshua-Fields/pemba/pull/83)
merged as main `17bfd6756e3c3b5dcf61bddc6347b6b5bd7820d9`.
Its [CI](https://github.com/Joshua-Fields/pemba/actions/runs/40860098802) and
[Playground](https://github.com/Joshua-Fields/pemba/actions/runs/40860098586) checks passed.
Its [differential workflow](https://github.com/Joshua-Fields/pemba/actions/runs/40860098600)
was running at the initial prerequisite check. The agent stopped; the maintainer
explicitly authorised continuing T7.4 while it ran. It subsequently completed successfully
before this PR was prepared. Skipped release/deployment jobs are not publication evidence.
T7.1 decisions remain in force; later human activation/publication/launch gates remain.

### Acceptance coverage

| Requirement | Implementation / verification |
|---|---|
| Concise README and deeper guides | README links five focused guides: installation/quick starts, schema, consistency, transactions/Postgres and troubleshooting. Existing examples retained; root/Python README bytes match. |
| Requirements and installation | CPython >=3.11, interpreter SQLite >=3.26.0 with threads, pinned Rust 1.99.0, Postgres >=15/UTF8, five wheel platforms, manylinux2014/glibc floor, compiler/headers/maturin prerequisites, CLI installation and feature selection. No support floor changed. |
| Published alpha versus checkout | Python alpha.4 installs and runs the existing README. Rust README uses post-alpha.4 constructors/check_str: corrected its misleading registry-dependency claim and supplied a checkout manifest with explicit memory feature. Alpha install commands remain for T7.12; no stable-package claim. |
| Clean quick starts | Exact README Python block executed outside the repository with a clean PyPI-alpha venv, then separately in another empty directory with a fresh source venv. Exact Rust README block compiled/ran from a new external Cargo project with only facade imports and a memory-only path dependency. CLI installed into an isolated external root; installed help passed. |
| Transactions | Caller commit/rollback, engine-connection reads, no uncommitted-read promise, same database/one SQLite copy, migrations and per-store PendingRevision behavior documented. SQLite rollback numbers are void/reusable; Postgres pending/rolled-back errors covered. |
| Schema, consistency and bounds | Supported DSL/differences, denial versus errors, limits/trace truncation, pagination, GC horizon, current schema on historical data, sync/async limits and when to use SpiceDB covered. |
| Postgres operations | Privileges/deploy role, TLS validation/platform roots, PgBouncer prepared statements, 2PC/early constraints and whole-transaction SERIALIZABLE/REPEATABLE READ retries. T5.13 remains deferred. |
| Snippet checks | Two new executable Python guide blocks have new pytest coverage; existing verbatim README/spec test retained. Rust guide reuses README code. No T7.5 app-row tutorial or T7.6 site implemented. |
| Specification ambiguity | OQ-062 records `pemba migrate --repair` absent from the shipped CLI. Guide conservatively advises no schema renaming and invents no repair procedure. Maintainer disposition remains open, non-blocking for the documented no-rename path. |

### Verification, 2026-10-29

Windows x86_64; Rust 1.99.0; CPython 3.12.10; Node 24.20.0; wasm-bindgen 0.2.129.
Checks ran on the working tree committed as `8331df4`. Subsequent guide edits only
added the manylinux tag/OQ link and qualified savepoint recovery; no snippet, test or
runtime changed. Links/anchors, mirror bytes and focused tests were rechecked before commit.

| Command / evidence | Result |
|---|---|
| New snippet tests before guides existed | 2 expected missing-file failures; after implementation, new blocks plus README/spec tests pass (4 focused tests). |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test --workspace --all-features` | Initial run failed: local PG endpoint refused connections (28 PG unit failures). Started dedicated disposable PG18/TLS containers; reran with `PEMBA_PG_REQUIRED=1`: **685 tests/doctests passed**, 18 ignored tests unrun. No slow/property implementation touched; conditional ignored-suite rerun not applicable. |
| Strict all-feature workspace docs and doctests | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`; `cargo test --workspace --all-features --doc` passed. Strict facade no-default-feature docs/doctests also passed. |
| Separate fuzz workspace fmt / check | `cargo fmt --manifest-path fuzz/Cargo.toml -- --check`; `cargo check --manifest-path fuzz/Cargo.toml --all-targets` passed. No fuzz target executed. |
| Fresh tool-only venv: maturin develop, pytest, strict mypy | Built source extension; 167 passed, one PG module skipped for absent psycopg (not a pass). Strict mypy: 15 files passed. Only maturin/pytest/mypy plus dependencies and pemba installed. |
| Separate driver venv: rebuilt extension; Postgres/error/documentation pytest files | PG18.6 plus untrusted TLS server, `PEMBA_PG_REQUIRED=1`: 66 passed, zero skips; psycopg 3/psycopg2/SQLAlchemy/Django, transactions/async bridge and TLS rejection. Covers the absent driver suite separately. |
| Driver strict mypy; TLS and SQLite probes | 15 typing files passed; `ci/check_tls.py` and `ci/check_no_sqlite.py` passed: rustls/no OpenSSL, no bundled SQLite. |
| Clean external registry install | New external venv: `python -m pip install --pre pemba` installed `pemba-0.1.0a4`; verbatim Python README passed. Windows alpha wheel only, not stable/all-platform verification. |
| Clean external source quick starts | Source-venv Python README passed in its own empty directory. External Rust project with `default-features=false, features=["memory"]`: `cargo +1.99.0 run` passed. Source dependency, not registry Rust installation. |
| Source CLI installation | `cargo +1.99.0 install --path crates/pemba-cli --locked --root <new external directory>` and installed `pemba --help` passed; existing installations untouched. |
| wasm32 Clippy / smoke | Five playground-compatible crates and wasm_smoke Clippy passed with `-D warnings`; 4 Node WASM tests passed. |
| Native/WASM conformance | `playground/conformance/run.sh`: 1,000 generated cases agree with reference (12,499 checks, 3 expected check errors, 15,678 lookups), 1,037 fixtures; identical transcripts across 45,323 lines. |
| Playground WASM/build/unit/size/browser/audit | Passed: 169 unit tests/10 files, 72 Chromium tests, 578.2 KB gzip <700 KB; zero shipping vulnerabilities (`npm audit --omit=dev`). |
| Local Markdown and copy checks | All local guide targets/anchors resolve, README bytes match, `git diff --check` passed. |

Initial extraction needed explicit UTF-8 on Windows. One source Python attempt reused
the alpha run's directory and correctly hit the duplicate app-row insert; rerunning in
its own empty directory passed. Neither setup failure is counted as a successful run.
Review found overly broad savepoint-recovery wording; the guide now directs rollback
on failed authorisation writes and handles commit errors. No runtime fix was needed;
fresh review found no further actionable correctness/scope/AC gaps.

Logs/scripts remain ignored under `target/t7.4-*`; quick starts/CLI live outside the
repository. No build output or credentials committed. T7.4 PR remote CI/platform/service
and playground jobs are **not yet observed** at preparation. Skips/unavailable jobs are
not passes. Oldest Python/SQLite, PG15, all-platform wheels, package dry runs and long
property/differential/fuzz checks remain their existing CI/release gates; this is not
T7.11/T7.13 rehearsal evidence. Human review/merge is required before T7.5. No next task,
merge, tag, publication or deployment is authorised by this evidence.

## T7.5 transactional-write tutorial and regression examples

Sources: T7.5 all AC; ADR-006, ADR-008, ADR-015; `05` §10;
`06` §1, §2.1–§2.3; `07` §3, §6; `10` §1.6.4, §2.3, §4 item 1.
Implementation source: [`257875f1c8b3b448e104a5ce4d0ab404f989628a`](https://github.com/Joshua-Fields/pemba/commit/257875f1c8b3b448e104a5ce4d0ab404f989628a)
on `t7.5-transactional-write-tutorial`. This subsequent evidence-only change does not
alter any source exercised by the checks. No runtime/API, dependency, storage format,
version, tag, publication or deployment changes.

### Preceding gates

Main was checked out and pulled to `5fa7024328b1e92208eba7a50c751f5a3e706aff`.
[T7.4 PR #84](https://github.com/Joshua-Fields/pemba/pull/84) is merged at that commit
(GitHub metadata and first-parent history verified). T7.0 plan PR #80, T7.1 decisions
PR #81, T7.2 audit PR #82 and T7.3 docs PR #83 are merged in the same main history;
acceptance/verification evidence is recorded above. Required T7.1 scope/helper/docs/
security decisions remain in force. Private reporting activation and final release/
launch approvals remain at their later human gates, not prerequisites to this tutorial.

The maintainer explicitly authorised continuing T7.5 even while other Actions checks
were still running. Prior T7.4 remote runs were not reclassified as passed here; its
merge and recorded local AC evidence were verified. OQ-062 remains the existing
non-blocking no-schema-rename restriction. No new specification ambiguity or product
decision was required, and no OPEN_QUESTIONS entry was added.

### Acceptance coverage

| Requirement | Implementation / evidence |
|---|---|
| Runnable app-row + permission examples, both languages/stores | `examples/transactional_sqlite.rs`, `transactional_postgres.rs`, `transactional_sqlite.py`, `transactional_postgres.py`. The Rust examples are registered on their existing store crates, using only existing dependencies; Python Postgres uses the already supported psycopg 3 driver. |
| Successful commit and forced rollback | All four insert `committed` plus its owner tuple, then insert `rolled_back` plus its tuple and deliberately cause an app duplicate-primary-key failure. Explicit Rust/SQLite rollback or the psycopg transaction context removes both app and permission data; surviving app row and allow/deny outcomes are asserted. |
| Pending, resolution, consistency and error distinction | SQLite asserts resolved number plus precommit `RevisionNotYetAvailable`; Postgres asserts unresolved/`RevisionPending`. Both assert Latest cannot see the uncommitted grant, resolve after commit, allow Anne with AtLeast and deny Bob. Postgres asserts `WriteRolledBack`; SQLite discards the void/reusable token. Errors propagate distinctly from denial. |
| Tests execute the real examples on temporary databases | Existing Rust SQLite `write_in` and PG `foreign` suites import the same Rust source as the CLI targets; Python `test_examples.py` / `test_postgres.py` execute the same scripts via runpy. Existing TempDir/TestSchema/schema fixtures provide temporary files/schemas. CLI entry points were also run directly. |
| Required service and driver | Existing CI PG15/18 jobs run `foreign` and `test_postgres.py` with `PEMBA_PG_REQUIRED=1`. Required runs now fail on missing psycopg instead of silently skipping the module. Local PG18 driver suite has zero skips; new tutorial paths also passed on PG15. |
| Reuse guard/rollback/savepoint coverage | Existing Rust write_in/foreign and Python foreign/postgres suites retain mismatched database, savepoint, rollback, autocommit and supported-driver guards. Only tutorial execution paths and required-driver enforcement were added. |
| Focused guide and related examples | [Tutorial](guides/transactional-tutorial.md), linked from the existing transactions guide, documents commands, local disposable database setup, same-SQLite-copy rule, caller ownership, store-specific pending/rollback and error semantics. Links existing agent script/fixture, Google-Docs-style sharing and GitHub-like fixtures; explains memory/WASM playground cannot demonstrate real database transactions. |

### Verification, 2026-10-29

Windows x86_64, pinned Rust 1.99.0, CPython 3.12.10, Node 24.20.0,
wasm-bindgen 0.2.129; all implementation checks below use `257875f`.
The full §5 command sequence ran once. Subsequent checks cover separate graphs/services
or correct only environment isolation; the full suite was not repeated after each edit.

| Command / evidence | Result |
|---|---|
| Test-first example regressions | SQLite Rust compilation and SQLite/PG Python execution first failed for the absent example files. Implemented scripts then passed. No runtime library behavior was changed. |
| Focused Rust tests / Clippy | `cargo test -p pemba-store-sqlite --test write_in -p pemba-store-postgres --test foreign`: 20 passed; `cargo clippy -p pemba-store-sqlite -p pemba-store-postgres --all-targets --all-features -- -D warnings` passed. |
| Focused Python examples/foreign/PG | Initially 52 passed, one TLS test skipped because TLS URL was unset; that skip is not a pass. TLS was configured for the separate service run below. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test --workspace --all-features` | 687 tests/doctests passed; PG18 present, `PEMBA_PG_REQUIRED=1`. 18 ignored tests unrun, not counted as passes. No slow/property tests or implementation touched; §5 conditional ignored-suite rerun is not applicable. |
| Strict all-feature workspace docs and doctests | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features` and `cargo test --workspace --all-features --doc` passed. |
| Separate fuzz workspace | `cargo fmt --manifest-path fuzz/Cargo.toml -- --check`; `cargo check --manifest-path fuzz/Cargo.toml --all-targets` passed. No fuzz target run. |
| Fresh tool-only Python venv | Only maturin 1.15.0, pytest 9.1.1, mypy 2.4.0 and dependencies, then source pemba via `maturin develop`. Build passed. Initial pytest inherited required-PG variables and failed collection on missing psycopg, proving the new required-driver guard; not a pass. Removed service variables for this isolated environment and reran pytest: **168 passed, one absent-service PG module skipped**, not counted as a PG pass. `mypy --strict tests` from Python crate passed for 15 files. |
| Separate required PG18 driver suite | Rebuilt extension in separate all-driver venv; `PEMBA_PG_URL`, `PEMBA_PG_TLS_URL`, `PEMBA_PG_REQUIRED=1`. `pytest -q -rs tests/test_postgres.py tests/test_errors.py tests/test_documentation.py tests/test_examples.py`: **70 passed, zero skips**, including psycopg 3, psycopg2, SQLAlchemy, Django, tutorial and untrusted TLS rejection. Driver versions retained from the existing CI-matched venv (psycopg 3.3.6, psycopg2 2.9.13, SQLAlchemy 2.1.6, Django 6.1.2, greenlet 3.5.6). |
| Driver typing and probes | `mypy --strict tests` passed (15 files); `ci/check_tls.py` and `ci/check_no_sqlite.py` passed (rustls, no OpenSSL, no bundled SQLite). |
| PG15 tutorial compatibility | Required-service `cargo test -p pemba-store-postgres --all-features --test foreign`: 10 passed; Python `test_postgres.py -k tutorial`: one passed, 33 intentionally deselected, zero skips. This is focused coverage, not the later complete PG15 release matrix. |
| Direct example entry points | Both documented Cargo example commands and both Python scripts passed with separate fresh SQLite files / disposable PG18 schemas. No credentials embedded in source. |
| wasm32 Clippy and smoke | Main five playground-compatible crates plus wasm_smoke Clippy passed with `-D warnings`; all 4 Node WASM smoke tests passed. |
| Native/WASM conformance | `playground/conformance/run.sh`: 1,000 generated cases agree with reference (12,499 checks, 3 expected check errors, 15,678 lookups); 1,037 fixtures; identical native/WASM transcripts across 45,323 lines. |
| Playground builds/tests/size/audit | WASM build, web build, 169 unit tests/10 files, 72 Chromium tests all passed; gzip 578.2 KB <700 KB; `npm audit --omit=dev`: zero shipping vulnerabilities. |
| Package boundary spot check | `cargo package -p pemba-store-sqlite --no-verify --allow-dirty` and corresponding Postgres command succeeded. Cargo warns that root external example sources are omitted; normalized package manifests omit their targets. Tutorials intentionally describe the current checkout. This is not package build verification or publishing, and does not replace later package dry runs. |
| Links/copies/review | Local guide file links and anchors resolve; existing README mirror test remains green; `git diff --check` passed. Fresh read-only whole-branch reviewer found no actionable correctness/scope/AC issue. |

Logs and harnesses remain ignored in `target/t7.5-*`; temporary database containers
are removed after verification. No generated output or credentials committed. New PR
CI/platform/service/playground runs are **pending/not yet observed** at preparation;
no skipped or unobserved remote job is counted as passed. Oldest Python/SQLite,
all-platform wheels, package-build dry runs and long release/property/differential/fuzz
matrix remain their existing later gates. Human review/merge is required before T7.6;
this task does not approve a release candidate or authorise later tasks/actions.

## T7.6 documentation site

Sources: T7.6 all AC; OQ-059; ADR-016; `11` §5–§5.1; `09` §4.
Implementation: `bc814d1`, link-pinning correction
[`2b1a34d5f2cd38842010e43144f6ea5b7295292e`](https://github.com/Joshua-Fields/pemba/commit/2b1a34d5f2cd38842010e43144f6ea5b7295292e),
on `t7.6-documentation-site`. Subsequent evidence/whitespace-only edits do not change
exercised behavior. Rust/Python/WASM runtime sources are byte-identical to main
`07ffd74fdb3e8bf55ffaaca42a6763ede78e94c1`, against which the single full local run was
started; frontend/docs checks exercise the T7.6 implementation and correction.

### Preceding gates

Fresh checkout: `git checkout main` then `git pull`, main `07ffd74`.
GitHub metadata confirms T7.0–T7.5 PRs #80–#85 merged, including previous implementation
[PR #85](https://github.com/Joshua-Fields/pemba/pull/85). Required T7.1 decisions in
OQ-044/OQ-058–OQ-060 remain recorded; later activation/publication/launch gates remain
human actions. Pages fix [PR #79](https://github.com/Joshua-Fields/pemba/pull/79) is
merged as `9e2d894`; this implementation extends that existing deploy/live verifier.
At inspection, #85's [CI](https://github.com/Joshua-Fields/pemba/actions/runs/40897391692)
and [Playground](https://github.com/Joshua-Fields/pemba/actions/runs/40897391634) runs
passed; its wheel run was queued, not counted as a pass or full release rehearsal.

### Acceptance coverage

| Requirement | Implementation / evidence |
|---|---|
| Approved generator and source reuse | Locked VitePress in `docs/site/`; generated views reuse README quick starts and all six T7.4–T7.5 guides. Navigation includes installation, quick starts, schema, transactions/tutorial, consistency, limits/troubleshooting and API links. Later contributor/security/roadmap links are not added prematurely. |
| Version/content policy | Stable docs use selected stable tag source; development uses main at `/docs/next/`. Every guide shows a visible release/development banner, including mobile. Other repository links pin to the selected commit; absolute README main links are normalized too. OQ-063 records conservative no-stable/legacy placeholders for maintainer review. |
| Strict accessible build | Build warnings and broken local page/anchor/asset links fail. Responsive default navigation and visible labels verified at 1280px and 390px, with keyboard skip link and tutorial navigation; no browser errors/404s or horizontal overflow. |
| Single Pages assembly | Existing main deploy gains docs after playground assembly. Stable builds first, development second; existing root, `/next/`, immutable `/v/` assets and live byte verification remain in the same artifact/deployer. Tags continue to dispatch main; no competing deploy. |
| Release/deployment regressions | CI-invoked Node suite builds actual docs into a synthetic complete site. A temporary repository with old stable, newer stable and newer prerelease tags proves an older-tag dispatch selects newest stable source, keeps root and archived engine bytes unchanged and includes main docs. Wrong channel/version, missing pages/assets/anchors are rejected; existing release classifier is reused. Existing playground share/version tests remain green. |
| Dependencies | `docs/site/README.md` justifies VitePress 1.6.4, Vue 3.5.44 for the visible theme banner and a scoped Vite 6.4.5 security override. Initial default Vite 5/esbuild audit found advisories; override removes them, final full docs audit reports zero. Playground lock/dependencies unchanged. |

### Verification, 2026-10-29

Windows x86_64; pinned Rust 1.99.0; CPython 3.12.10; Node 24.20.0;
wasm-bindgen 0.2.129. Logs/harnesses remain ignored under `target/t7.6-*`.

| Check | Result |
|---|---|
| Full §5 once: fmt, workspace all-target/all-feature Clippy, workspace all-feature tests | Passed; 687 Rust tests/doctests with required PG18.6 service. 18 ignored tests unrun, not passes; no slow/property implementation or tests changed. |
| Strict all-feature rustdoc; separate fuzz fmt and all-target check | Passed. Fuzz targets were not executed; long release fuzzing remains T7.11/T7.13. |
| Fresh tool-only venv: maturin, pytest, strict mypy | Build and mypy passed (15 files); pytest 166 passed, three skipped: PG-driver module absent, two CLI tests could not locate the shared Cargo-target binary. Configured documented `PEMBA_CLI`, then focused CLI rerun passed 2/2. Skips are not passes. |
| Separate required PG18.6/all-driver/TLS environment | 70 Python tests passed, zero skips; strict mypy and TLS/no-bundled-SQLite probes passed. Disposable services removed afterward. No new PG behavior requires repeating the later all-platform release matrix. |
| wasm32 Clippy and Node smoke | Passed; all four WASM smoke tests passed. |
| Native/WASM conformance | 1,000 generated reference-checked cases; 1,037 fixtures; identical transcripts over 45,323 lines (12,499 checks, three expected check errors, 15,678 lookups). |
| Docs tests/build/browser | Six Node regressions passed. Strict development build and no-stable build passed with no warnings; desktop/mobile Chromium keyboard/label/asset checks passed. |
| Playground | Initial fresh-checkout unit run failed because generated WASM glue was absent; built the required WASM prerequisite, then all 170 unit tests passed. Production/typecheck build, 72 Chromium tests, 578.2 KB gzip against 700 KB budget passed. Shipping npm audit: zero vulnerabilities. |
| Docs dependency audit | Initial default-tooling audit: two moderate and one high dependency findings; corrected scoped override, final `npm audit --prefix docs/site`: zero vulnerabilities. No known finding claimed passed without correction. |
| Workflow and diff | actionlint 1.7.12 passed (`-shellcheck=` because shellcheck is unavailable; shellcheck is not claimed). Final branch whitespace check passed. |
| Independent review | One P2: stable README links could escape to main. Added regression, observed failure, fixed absolute-repository link mapping; all six docs regressions and 170 playground unit tests then passed, and docs build/browser checks passed. No remaining P1/P2 finding reported. |

The new PR's remote CI/platform/Pages jobs are pending/not observed at preparation;
no skipped, queued or unobserved job is counted as passed. No deployment was executed,
and live public docs verification remains T7.16. All-platform wheels, oldest supported
platforms and long release/property/differential/fuzz verification remain their later
gates. Maintainer review of OQ-063 and this PR's merge are required before T7.7.
No APIs, storage formats, releases, tags or deployed artifacts were changed.


## T7.7 developer onboarding

Sources: T7.7 all AC; `CLAUDE.md` §§2–6; `07` §§1–6; `09` §§1–5.
Implementation/source commit: [`6937663f27556d1cd188297e99590ca6c3e3595f`](https://github.com/Joshua-Fields/pemba/commit/6937663f27556d1cd188297e99590ca6c3e3595f)
on `t7.7-developer-onboarding`. Subsequent evidence-only edits do not change the
exercised guide, site, workflows or runtime sources.

### Preceding gates

Fresh clone into this task's workspace, then `git checkout main` and `git pull`:
main `2d861878f8edbe3cbf1b37ba563cda08b3839cf0`, clean and up to date.
First-parent history confirms T7.0–T7.6 PRs #80–#86 merged; GitHub metadata confirms
previous implementation [PR #86](https://github.com/Joshua-Fields/pemba/pull/86)
merged 2026-10-29 at that main commit. Earlier AC evidence is indexed above and the
required T7.1 maintainer decisions remain recorded in PR #81/OQ-044/OQ-058–OQ-060.
OQ-063's conservative no-stable/legacy presentation was presented for maintainer
review in the now-merged #86; its non-blocking interpretation is retained. OQ-062
remains non-blocking for the documented no-rename path. No new product decision or
specification ambiguity arose. The maintainer explicitly authorised T7.7 to continue
while other Actions checks run; prior pending checks are not reclassified as passes.
Later security activation, release approval/publication and launch remain human gates.

### Acceptance coverage

| Requirement | Implementation / evidence |
|---|---|
| Clean setup and prerequisite pins | New root `CONTRIBUTING.md` reuses `rust-toolchain.toml`, Node 24 and the exact CI Python tool/driver pins; describes native compilers, Python/SQLite floors, Bash/PowerShell setup and a fresh tool-only venv. No new dependencies or alternate pin files. |
| Layout and regression workflow | Guide maps crates, fixtures, oracle, examples, docs, playground and separate fuzz workspace; demonstrates one fixture, independent oracle-backed corpus, saved regressions, focused tests and relevant dependents. |
| Pre-PR versus long release work | Full §5 commands, separate fuzz checks, strict docs, applicable WASM/playground, Postgres/TLS/driver flags, SpiceDB required-service flag, honest skip reporting; long properties/differential/fuzz/wheel/platform verification stays at its release gates. |
| Contribution rules and boundaries | One task/branch/PR, conventional commits, spec/API/format changes, human ADR decisions, dependency justification/licences, fail closed, original code/attribution and no automatic release actions. |
| Contributor site navigation | Generated from the selected revision's root guide, with local guide links and commit-pinned repository links; omit page/navigation when an older selected source lacks it. Existing workflow path filters include `CONTRIBUTING.md` for rebuilds. No competing deployer. |
| Clean onboarding exercised | New clone, fresh local Cargo output (no shared target), fresh tool-only venv and locked npm installs; compiled CLI, imported extension, one fixture, focused Rust/Python tests and strict docs builds executed below. |

### Verification, 2026-10-29

Windows x86_64; Rust 1.99.0; CPython 3.12.10; Node 24.20.0; wasm-bindgen 0.2.129.
Disposable Postgres 18.6 containers use loopback ports 56432/56433, with separate
untrusted-certificate TLS service and required-service flags. Commands/logs/harness
remain ignored under `target/t7.7-*`; venvs and generated artifacts are not committed.

| Command / check | Actual result |
|---|---|
| `cargo build --locked -p pemba-cli`; `maturin develop --manifest-path crates/pemba-py/Cargo.toml`; `python -c "import pemba; print(pemba.__file__); print(pemba.Engine.memory())"` | Passed from fresh build output and isolated venv; import points to this checkout's installed extension/package. |
| `cargo run --locked -q -p pemba-cli -- test testdata/fixtures/direct.yaml` | Passed: one memory fixture, 26 checks. |
| `cargo test -q -p pemba-testkit --test fixtures`; `cargo test -q -p pemba-schema`; focused schema Clippy; `python -m pytest -q tests/test_readme_example.py` | Passed: 18 fixture tests, 158 schema tests/doctests, lint and two Python README tests. |
| Full §5 once: fmt, workspace all-target/all-feature Clippy, workspace all-feature tests | Passed: 687 Rust tests/doctests, zero failures, with required PG18 service. 18 ignored tests unrun, not passes; no slow/property implementation or tests changed. |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`; separate fuzz fmt/all-target check | Passed. Actual nightly fuzz runs remain later release checks. |
| Tool-only fresh venv: maturin, `python -m pytest -q -rs`, `python -m mypy --strict tests` | Passed build, 168 tests and typing (15 files); one skipped Postgres module because this venv deliberately has no drivers/service URL. That skip is not a Postgres pass; separate service results below. |
| wasm32 Clippy (engine/playground and smoke target), Node smoke test | Passed; four WASM tests, zero failures. |
| `npm test --prefix docs/site`; strict development and no-stable builds; `node docs/site/scripts/browser.mjs` | Passed: seven regressions, both builds with no warnings; wide/narrow keyboard navigation reaches Contributing with correct heading/development label and no missing assets/browser errors or horizontal overflow. Screenshots also inspected. |
| New contributor regression | Observed expected failure before implementation, then pass: old sources omit contributor content, new source uses its guide and selected revision's links. Existing older-tag assembly/release tests retained. |
| CI package copies and workflow syntax | All 20 LICENSE/NOTICE copies and README mirror byte-identical; actionlint passed with unavailable shellcheck disabled. Shellcheck is not claimed. |
| Fresh independent review | No actionable findings; reviewed guide commands, service environment, source-version navigation, workflow filters and T7.7 AC. |
| `bash playground/conformance/run.sh` | Passed: 1,000 generated reference-checked cases, 1,037 fixtures; native and WASM byte-identical across 45,323 transcript lines (12,499 checks, three expected check errors, 15,678 lookups). |
| Playground WASM generation, unit tests, production/typecheck build, size and Chromium e2e | Passed: 170 unit tests, 72 browser tests, 578.2 KB total gzip against 700 KB budget. |
| `npm audit --omit=dev --prefix playground/web`; `npm audit --prefix docs/site` | Passed: zero vulnerabilities in shipping playground dependencies and full docs-tooling dependencies. |
| Separate CI-pinned all-driver venv, required PG18/TLS: maturin, `python -m pytest -q -rs tests/test_postgres.py tests/test_errors.py`, strict mypy, TLS/no-SQLite probes | Passed: 63 Python tests, zero skips; all four driver families installed. Build, typing and both binary probes passed. |
| `cargo test -p pemba-store-postgres --no-default-features` with required service | Passed, including plaintext-only configuration. Full-feature store conformance/equivalence/foreign-write tests also passed in the full workspace run above. |

Only evidence documentation changed after the verified source commit. Final whitespace,
local contributor link targets, package-file copies and scope checks passed. An ad-hoc
local link-check script initially used Windows' default text encoding; rerunning it
with explicit UTF-8 passed (no product/source change). Disposable services were stopped
after verification. No unavailable, skipped or unobserved check is counted as passed.
The new PR's remote checks are not observed at preparation; docs-only CI skips are not
release evidence. PG15, oldest-platform checks, the all-platform wheel/package matrix,
long properties/differential/fuzz/mutation runs and live deployment verification remain
T7.11/T7.13/T7.16 gates, not claims made by this onboarding task.

No API, storage format, package version, release, tag or deployed artifact changed.
Human review/merge is required before T7.8. No release approval or next-task action is
performed by this PR.

## T7.8 security reporting and release safeguards

Sources: T7.8 all AC; CLAUDE §2; ADR-008/ADR-011; `07` §5; `09` §4;
`10` §3.3; approved OQ-060. Verification results and limitations are recorded below.

### Preceding gates and implementation

Fresh clone, `git checkout main` and `git pull` yielded clean main
`ed63ed3dcfcf2ad43023029c82b4d5bfd3fd9733`. T7.0–T7.7 PRs #80–#87 are merged in
first-parent history. GitHub metadata confirms previous implementation PR #87 merged
2026-10-29 at that SHA. Earlier AC evidence and T7.1 decisions are indexed above.
The maintainer authorizes proceeding while other Actions runs finish; no pending or
skipped prior check is relabelled a pass. Later human release/launch gates remain.

`SECURITY.md` implements OQ-060: private GitHub reporting, owner handling, latest stable
0.1.x security/correctness maintenance on a best-effort basis, case-by-case older
versions, no SLA/guaranteed patch/LTS, and no public exploit details or secrets.
The existing docs builder includes security content/navigation only when present in
the selected source revision. SECURITY changes now trigger the single existing docs/
playground workflow. No runtime/API, dependency, package version or deployment change.

### Read-only safeguards review, 2026-10-29

| Area / evidence | Finding and disposition |
|---|---|
| Workflow permissions and events: all `.github/workflows/*.yml`; GitHub Actions permissions APIs | All workflows declare `contents: read` by default. Live repository default is read; Actions cannot approve PRs. No `pull_request_target` or `workflow_run` execution of untrusted PR code. Fork PRs use ordinary `pull_request`; no repository/environment secrets referenced. |
| Elevated release/Pages jobs: `playground.yml` | `contents: write` only for tag release asset; `actions: write` only for tag deploy dispatch; Pages/id-token writes only in main push/manual deploy. PRs do not enter these jobs. Tag asset immutability and single Pages deployer retained. No workflow event/permission changes needed. |
| PyPI trusted publishing: `wheels.yml` | Publishing needs build/sdist/all platform smoke/Postgres smoke success, tag push, `pypi` environment and job-only `id-token: write`. Manual and PR runs build/test only. No API token in workflow. Downloaded artifacts come from the same run; five wheels/one sdist required. Human must verify PyPI's registered publisher binding to owner/repo, `wheels.yml`, `pypi`; workflow text or old uploads do not prove current account configuration. |
| Live required reviewer: `gh api repos/Joshua-Fields/pemba/environments/pypi` | Required reviewer is `Joshua-Fields`. `prevent_self_review=false`, `can_admins_bypass=true`, no deployment branch restriction. These are recorded residual controls, not independent two-person approval or a tag-only environment guarantee. Existing job condition enforces tag pushes; retain explicit human approval, no settings modified. |
| Secret handling: workflow review; repository and `pypi` secrets APIs | Both secret counts are zero (values never requested). `GH_TOKEN` uses ephemeral `github.token` in the three trusted release/deploy jobs only. No shell tracing or secret echo. Database trust auth/test-only passwords are disposable CI fixtures, not production credentials. Review excludes personal/org secret stores and developer credential files. |
| Public crate ownership: crates.io `/api/v1/crates/<name>/owners` for all nine publishable crates | Sole returned owner is `Joshua-Fields` for pemba, pemba-cli, pemba-core, pemba-fixtures, pemba-schema, pemba-sqlite-common, pemba-store-memory, pemba-store-postgres and pemba-store-sqlite. Actual account access, 2FA and token scopes still need human verification; no publishing probe performed. |
| Repository access / protection APIs | Sole returned collaborator is owner/admin `Joshua-Fields`; no rulesets and main protection API reports 404 (branch not protected). Actions allowed-actions is `all`, SHA pinning not required; actions use version tags. Record as hardening opportunities for human review, not proof of enforced merge review, immutable action pins or protected release tags. No repository policy altered or new approval policy invented. |
| GitHub security APIs | Private vulnerability reporting returns `enabled: true`; this is observed configuration, not a human end-to-end submission/notification test. Dependabot security updates and secret scanning/push protection are disabled; weekly npm Dependabot configuration and cargo-deny/npm audits remain existing checks. Human may review additional settings. |
| Package ownership/access unavailable from this session | Authenticated PyPI owner/maintainer list, trusted publisher registration, registry 2FA/recovery and scoped publishing access are not observable through available connected tools/CLI. Human must verify these before T7.14/T7.15; never infer them from package metadata or previous successful publishing. |
| Attribution and licence copies | ADR-011 retained: Apache-2.0, original implementation, no SpiceDB copying. CI's byte-identical LICENSE/NOTICE copies across ten package directories plus README mirror will be checked below; no attribution/package changes made. |

### Human follow-up

Repository owner must confirm the private report form/notification path works without
submitting a fabricated vulnerability; verify authenticated PyPI owners and trusted
publisher configuration, registry access/2FA/recovery and appropriate token scopes.
Record that confirmation before release sign-off/publication, then reconfirm reporting
and triage activation at T7.17. Enabled settings alone do not supply human approval.
Existing required `pypi` reviewer is observed; do not bypass it. Main/tag protection,
SHA-pinned actions and additional scanning are recorded hardening choices for the owner,
not silently activated or treated as newly approved policy.

### Reviewed security regression coverage

Reviewed existing assertions instead of duplicating tests: `crates/pemba/tests/check.rs`
checks store failures, union/intersection/exclusion error propagation, stale schema
subjects, depth, fanout and read budgets as typed errors; `lookup.rs` fails the whole
page on a candidate-check error and `lookup_subjects.rs` enforces depth/fanout.
`pemba-testkit` trace/cycle/cost tests and fixture/oracle comparisons cover bounded
cyclic evaluation without changing fail-closed semantics. These ran in the workspace
suite below. No evaluator optimization or regression fix was introduced.

Postgres `tests/migrate.rs::a_hostile_search_path_cannot_redirect_the_store` checks
qualified tables, spoofed functions and trigger `search_path=pg_catalog, pg_temp`;
`bad_configuration_is_an_invalid_request` checks invalid configuration and password
redaction. `tests/foreign.rs::an_unprivileged_role_can_write_in` creates a NOSUPERUSER
NOINHERIT app role with limited schema/table/sequence privileges and commits a foreign
write; migrations use the separate privileged test connection. All passed against
required PG18. This verifies the tested normal-role path, not every possible production
grant configuration. No schema rename/repair capability is added (OQ-062 remains).

Reviewed the SQLite-symbol and rustls/no-OpenSSL byte probes in `crates/pemba-py/ci/`
and their installed-extension/wheel CI call sites. The Python TLS test requires an
untrusted TLS server, asserts a certificate handshake error, then proves the same
server is reachable without TLS. It ran with both service URLs and all drivers below;
no TLS skip is counted as a pass. Wheel/platform execution remains the release matrix.
The three fuzz targets match `07` §5 and existing manual fuzz CI; only build checks ran
here. Fresh >=10-minute runs per target remain T7.11/T7.13.

### Verification, 2026-10-29

Source commit [`3c94b6901565bb412b7930b2f09dd4dff7ec2172`](https://github.com/Joshua-Fields/pemba/commit/3c94b6901565bb412b7930b2f09dd4dff7ec2172).
Subsequent edits change release evidence only. Windows x86_64, Rust 1.99.0, CPython
3.12.10, Node 24.20.0, wasm-bindgen 0.2.129, cargo-deny 0.20.2. Disposable PostgreSQL
18.6 and untrusted-certificate TLS services bind loopback 57432/57433; required-service
flags are set for Rust and the separate driver venv. Logs/harness under ignored
`target/t7.8-*`; fresh venvs and generated assets are not committed.

| Command / check | Actual result |
|---|---|
| Full §5 once: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features` | Passed: 687 Rust tests/doctests, zero failures, required PG18 including privileged/normal-role and hostile-search-path tests. 18 ignored tests unrun; slow/property tests and implementation were not changed. |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`; separate fuzz `cargo fmt -- --check` and `cargo check --all-targets` | Passed strict docs and fuzz workspace build/format checks; fuzz targets not executed. |
| Fresh tool-only venv, CI-pinned tools: `maturin develop`; `python -m pytest -q -rs`; `python -m mypy --strict tests` | Passed build, 168 tests and typing (15 files). One skipped Postgres module with no drivers/service URL in this venv; not a service pass. |
| `cargo deny check` | Passed: advisories, bans, licences and sources all OK. Seven duplicate-version warnings (base64, const-oid, fallible-iterator, getrandom, syn, wasi, windows-sys), allowed by the existing warn policy; reviewed transitive/dev/platform overlap, no dependency or policy changes. |
| `npm audit --omit=dev --prefix playground/web`; `npm audit --prefix docs/site` | Both passed: zero vulnerabilities; shipping playground graph and full documentation-tooling graph respectively. |
| `npm test --prefix docs/site`; strict development and no-stable builds; `node docs/site/scripts/browser.mjs` | Eight regressions passed, including expected red-before-green security generation. Both builds and local link/anchor/asset checks passed without warnings. Wide/narrow keyboard navigation reaches Security with the correct heading, channel label and private-reporting URL; no browser/asset errors or horizontal overflow. Screenshots inspected. |
| wasm32 Clippy for engine/playground and smoke target; wasm32 `wasm_smoke` under Node | Passed; four smoke tests, zero failures. |
| `bash playground/conformance/run.sh` | Passed: 1,000 generated reference-checked cases, 1,037 fixtures; 45,323 native/WASM transcript lines byte-identical (12,499 checks, three expected check errors, 15,678 lookups). |
| Playground WASM generation; `npm test`; production/typecheck build; size; Chromium e2e | Passed: 170 unit tests, 72 browser tests; 578.2 KB total gzip within 700 KB budget. |
| Fresh separate CI-pinned all-driver venv with required PG18/TLS: maturin, `python -m pytest -q -rs tests/test_postgres.py tests/test_errors.py`, strict mypy, `ci/check_tls.py`, `ci/check_no_sqlite.py` | Passed: 63 Python tests, zero skips, all four driver families installed; build/typing and both binary probes passed on the built development extension. No all-platform wheel claim. |
| `cargo test -p pemba-store-postgres --no-default-features` with required PG18 | Passed plaintext-only configuration, including conformance, equivalence, foreign writes and migrations. Full-feature paths ran in the workspace run above. |
| CI package-file copy check; actionlint (`-shellcheck=`); final whitespace/scope check | All 20 LICENSE/NOTICE copies and README mirror byte-identical; actionlint passed with unavailable shellcheck disabled, not claimed. Whitespace/scope checks passed. |
| Independent review of implementation and acceptance coverage | No actionable findings; reviewed approved policy, selected-source navigation, older sources, workflow filters and qualified safeguard findings. Verification results were completed afterwards. |

The first container readiness probe ran before startup and reported unavailable;
a later explicit readiness probe and `SHOW ssl=on` succeeded before test execution.
No application fix or service skip was hidden. No confirmed release-blocking security
finding or genuine specification ambiguity arose; no OPEN_QUESTIONS entry was added.
Unobservable account controls remain explicit human verification gates above.

Remote checks for this PR are unobserved at preparation; docs-only skips are not release
evidence. PG15, oldest supported Python/SQLite, all-platform wheel/sdist/package dry
runs, long properties/differential/fuzz/mutation and public deployment verification
remain T7.11/T7.13/T7.16 gates. Nothing here supplies release sign-off or launch approval.
No settings, APIs, on-disk formats, versions, releases, tags or deployed artifacts were
changed. Human review/merge of this PR is required; T7.9 is not started.

## T7.9 issue triage and public roadmap

Sources: T7.9 all AC; M8–M10 outline; CLAUDE §§2,6; `09` §§5–6; approved OQ-060.

### Preceding gates and acceptance coverage

Fresh clone, `git checkout main` and `git pull` produced clean main
`b7263629f0aecd87287e0b2baa4415cb7d3cfd30`. First-parent history confirms T7.0–T7.8
PRs #80–#88 merged; GitHub metadata confirms previous implementation
[PR #88](https://github.com/Joshua-Fields/pemba/pull/88) merged 2026-10-29 at that SHA.
Earlier acceptance/verification evidence is indexed above. Required T7.1 human
decisions remain in PR #81/OQ-044/OQ-058–OQ-060; merged T7.6 includes OQ-063's
conservative empty/legacy-docs presentation. OQ-062 remains non-blocking for the
documented no-rename path. No new specification ambiguity or product decision arose.
The maintainer explicitly authorizes continuing while other Actions checks run;
pending/skipped prior runs are not reclassified as passes.

| Acceptance criterion | Evidence |
|---|---|
| Safe bug and feature intake | `.github/ISSUE_TEMPLATE/bug.yml` and `feature.yml` require version, store/platform, minimal synthetic schema/tuples (or explanation of non-applicability), expected/actual behavior and safe reproduction. Both direct suspected vulnerabilities to SECURITY/private reporting; config disables blank issues and exposes the private contact. No auto-labels or assignments. |
| Labels, severity/priority and ownership | `docs/TRIAGE.md` documents existing labels, impact-based severity and textual priority, owner Joshua-Fields, best-effort support without SLA, duplicate handling, regression fixtures, specification/task promotion and closure with release-version accuracy. Read-only `gh label list --repo Joshua-Fields/pemba --limit 100 --json name,description` confirmed all 12 documented labels exist. No labels/settings changed. |
| Roadmap scope and status | `ROADMAP.md` distinguishes shipped alpha.1–alpha.4 from unapproved/unpublished stable 0.1.0, remaining M7 gates, M8/M9/M10 candidates and T5.13 after stable per T7.1. Links the known Postgres round-trip follow-up/OQ-056/benchmark context, known differences, limitations and no-rename restriction. No dates, new supported scope or performance promises. |
| Site integration | Selected-source roadmap/triage pages and sidebar links appear only when that source contains them. Internal links route locally; repository material stays pinned to the selected commit. Existing single Pages workflow includes both source paths. No deployment mechanism or existing playground routes changed. |
| Human boundaries | Label setup, triage/roadmap activation and any response commitments remain human work at T7.17. Reporting usability and authenticated registry access/publisher confirmation remain the T7.8 release safeguards before T7.14/T7.15. No merge, tag, publication, announcement or deployment performed. |

### Verification, 2026-10-29

Source commit [`93a8c1ddb698198aff7d6ece73ba109f391ae341`](https://github.com/Joshua-Fields/pemba/commit/93a8c1ddb698198aff7d6ece73ba109f391ae341).
Subsequent edits change this evidence index only. Windows x86_64, Rust 1.99.0,
CPython 3.12.10, Node 24.20.0, wasm-bindgen 0.2.129. Disposable PostgreSQL 18.6
services bind loopback 57532/57533 (second server has an untrusted TLS certificate);
required-service flags are set for Rust and the separate all-driver venv.
Fresh tool-only venv uses CI-pinned maturin/pytest/mypy; a separate disposable
PyYAML environment validates forms, never contaminating the Python test venv.
Logs/harness under ignored `target/t7.9-*`; no generated output or credentials committed.

| Command / check | Actual result |
|---|---|
| Focused selected-source regression, then `node --test docs/site/scripts/*.test.mjs` | New regression observed failing before implementation, then passed; all nine docs tests pass, including older sources, current stable/development content, no-stable metadata, pinned/local links and older-tag assembly preserving playground bytes. Initial full docs attempt preceded `npm ci` and failed for missing VitePress; passed after locked dependencies installed. |
| Strict `npm run build --prefix docs/site`; `node docs/site/scripts/build.mjs --channel stable --empty true --out target/t7.9-docs-stable` | Both passed with warnings and broken local links treated as failures. Development source metadata names the source commit; no-stable placeholder retains the approved policy. |
| Headless Chromium docs verification | Roadmap/triage pages passed at desktop 1280×900 and mobile 390×844: HTTP 200, content/development label, sidebar navigation, keyboard focus and no page errors. Initial preview omitted DOCS_BASE and returned 404 for the project path; corrected preview base passed without source changes. |
| Disposable PyYAML validation and Markdown target/heading-anchor check | All three issue YAML files parse; five required input groups/private route/no auto-label or assignment validated for both forms, blank-issue/private contact configuration checked, workflow source filters occur for PR and push, and new documents' relative links/anchors resolve. |
| Full §5 once: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features` | Passed: 687 Rust tests/doctests, zero failures, required PG18 including normal-role and hostile-search-path paths. 18 ignored tests unrun; slow/property tests and runtime implementation were not changed. |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`; separate fuzz `cargo fmt -- --check` and `cargo check --all-targets` | Passed strict docs and separate workspace format/build. Fuzz targets not executed; long fuzz/property/differential runs remain the release rehearsal/candidate gates. |
| Fresh tool-only venv: `maturin develop`; `python -m pytest -q -rs`; `python -m mypy --strict tests` | Passed build, 168 tests and strict typing of 15 files. One skipped Postgres module without service URL/drivers in this venv; not counted as a service pass. |
| wasm32 lint and smoke: portable-crate Clippy, fixture smoke-target Clippy and four smoke tests | Passed both Clippy commands with `-D warnings` and all four Node-run wasm32 smoke tests. |
| `bash playground/conformance/run.sh` | Passed after the linker correction below: 1,000 generated cases agree with the reference evaluator, 1,037 fixtures total, 45,323 identical native/WASM transcript lines. Native DLL builds print a `linker stdout: Creating library` warning; this is not a test failure or a claim of a warning-free release build. |
| Playground WASM generation; `npm test`, `npm run build`, `npm run size`, `npm run e2e` in `playground/web` | Passed after linker correction: 170 unit tests, TypeScript/production build, 578.2 KB gzip within 700 KB budget, 72 Chromium browser tests. |
| Separate all-driver PG18/TLS venv: `maturin develop`; `pytest -q -rs tests/test_postgres.py tests/test_errors.py`; `mypy --strict tests` | Passed build, 63 tests, zero skips, strict typing of 15 files. Uses CI-pinned psycopg3, psycopg2, SQLAlchemy, Django and greenlet, required PG URL and untrusted-certificate TLS URL. |
| `cargo test -p pemba-store-postgres --no-default-features`; `ci/check_tls.py`; `ci/check_no_sqlite.py` | Passed required-service plaintext Rust configuration and installed-extension rustls/no-OpenSSL and no-SQLite probes. |
| `npm audit --omit=dev --prefix playground/web`; `npm audit --prefix docs/site` | Both passed: zero vulnerabilities. No dependencies or lockfiles changed. |
| Attribution/mirrors; workflow syntax; independent review | All 20 LICENSE/NOTICE copies and Python README mirror match. actionlint passed with unavailable shellcheck disabled (no shellcheck pass claimed). Fresh independent implementation/AC review found no actionable issues; reviewer also ran seven content tests successfully. |

The first Git Bash conformance/WASM build selected Git's `/usr/bin/link.exe` instead
of MSVC and failed. Missing WASM then caused unit/typecheck/build, size and browser
checks to fail downstream. Explicitly setting
`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER` to the installed MSVC x64 linker fixed
the test environment; only those affected checks were rerun and all passed.
The complete §5 check was run once, not repeated after evidence-only edits.

Remote PR CI/platform/service/Pages checks are pending/not observed at preparation;
none is claimed passed here. All-platform wheels, oldest supported runtimes, PG15,
long properties/differential/fuzz runs and live/public artifact verification remain
T7.11/T7.13/T7.16 gates. This documentation task changes no runtime API, on-disk
format, dependency, package version, release, tag or deployed artifact.

Human review/merge of this PR is required. Label setup, reporting/triage activation
and new response commitments remain T7.17 human actions. No new OPEN_QUESTIONS entry
was needed. T7.10 is not started; no release or launch approval is supplied here.

An ad-hoc final log-summary reader initially failed on Windows console encoding while
printing cargo-deny's Unicode diagnostic tree. Explicit UTF-8 output rerun succeeded
and verified every recorded check's zero exit code; no test or source change was needed.

## T7.10 launch drafts

Sources: T7.10 all AC; `00` §§1–4,7; ADR-006/ADR-011/ADR-015; T3.7;
`10` §1.6.4; measured context/caveats in `benches/RESULTS.md`.

### Preceding gates and acceptance coverage

Updated clean main after verifying [PR #89](https://github.com/Joshua-Fields/pemba/pull/89)
merged at `5bec1eb`. All applicable checks on its exact head `7419748` succeeded in
[CI](https://github.com/Joshua-Fields/pemba/actions/runs/40951006855) and
[Playground](https://github.com/Joshua-Fields/pemba/actions/runs/40951007041), including
PG15/18, platform Python/Rust, oldest SQLite/Python, property, package dry run,
wasm32, conformance and documentation/site. PR-only release/deploy jobs were skipped;
no publication or deployment result is inferred. T7.0–T7.9 and required T7.1 decisions
remain recorded in the preceding evidence. The maintainer now explicitly authorizes
waiting for checks and merging implementation PRs before continuing. Release sign-off,
release-PR merge, tagging/publishing and launch approvals remain their human gates.

`docs/launch/0.1.0.md` contains a proposed release summary, long-form post, two short
variants and channel checklist centered on the tested T7.5 app-row/permission commit
and forced-rollback examples. It accurately describes caller ownership, SQLite versus
Postgres pending/rollback behavior, post-commit resolution/AtLeast, engine-connection
reads and error versus denial. Links cover installation, docs, tutorial, playground,
Rust/Python API contracts, migrations, issues, roadmap, limitations and original-code
credits. It states when a separate service is preferable and preserves sync-only,
memory-playground, no general WASM/Node/wire-API and deferred T5.13 boundaries.

All materials are explicitly drafts, with stable URLs/install commands marked as
proposed post-publication targets requiring T7.16 validation. No numerical performance
or comparative security claims are proposed; any later benchmark claim must carry
measured version/commit, hardware/backend/context and existing caveats. Venue/account,
final text and timing remain unchosen/unapproved, with six unchecked human gates.
No third party was contacted or public announcement/external draft created. No new
specification ambiguity or OPEN_QUESTIONS entry was needed.

### Verification

Source commit `edcaf32ddcb449611b496b40e8dfc13101839cbc`; subsequent edits change this
index only. Windows x86_64, Rust 1.99.0, CPython 3.12.10, Node 24.20.0,
wasm-bindgen 0.2.129. Disposable required PG18.6/plaintext and untrusted-TLS services
bind loopback 57632/57633. Tool-only Python venv remains isolated from the separate
CI-pinned all-driver venv. Explicit MSVC linker selection avoids Git Bash's GNU link
collision. Logs/harness are ignored `target/t7.10-*`; no credentials/build output committed.

| Command / check | Actual result |
|---|---|
| Focused draft audit and relative link/anchor validation | Passed: required sections, transaction/scope/draft boundaries, installation/docs/playground/API/migration/issues/roadmap/security/credits link families and six unapproved human checklist items. Proposed stable external links are not claimed live before publication. |
| `node --test docs/site/scripts/*.test.mjs`; strict `npm run build --prefix docs/site` | Nine tests and warnings/broken-link-failing development build passed. Launch drafts remain repository-only and are not added to public docs navigation. |
| Independent review | No actionable findings on AC coverage, policy/transaction accuracy, draft status, credits and approval boundaries. |
| Full CLAUDE §5 once: fmt, all-target/all-feature Clippy with `-D warnings`, workspace tests | Passed: 687 Rust tests/doctests, zero failures, required PG18 service exercised (no 'not run' lines). 18 ignored tests unrun; no slow/property implementation was touched. |
| Strict all-feature rustdoc; separate fuzz workspace fmt and all-target check | Passed; fuzz targets were built, not executed. Fresh long runs remain T7.11/T7.13. |
| Tool-only venv: maturin develop, pytest, strict mypy | Passed build, 168 tests and typing of 15 files. One skipped Postgres module without driver/service in this venv, not a service pass. |
| wasm32 portable-crate/smoke-target Clippy with `-D warnings`; four Node smoke tests | Passed both lints and all four tests. |
| Native/WASM conformance | Passed 1,000 generated reference-evaluator cases, 1,037 fixtures and 45,323 identical transcript lines. Native DLL creation prints the existing informational linker-stdout warning; no warning-free release-build claim. |
| Playground WASM generation, unit, typecheck/build, gzip size and browser tests | Passed: 170 unit tests, production/typecheck build, 578.2 KB/700 KB budget and 72 Chromium browser tests. |
| Separate all-driver required PG18/untrusted-TLS venv and Rust plaintext configuration | Passed maturin build, 63 Python tests with zero skips, strict mypy, `cargo test -p pemba-store-postgres --no-default-features` and installed-extension rustls/no-OpenSSL/no-SQLite probes. |
| Shipping playground and full docs npm audits; package copies | Zero vulnerabilities in each audit; all 20 LICENSE/NOTICE copies and Python README mirror match. |

All recorded checks above exited zero on the source commit. No runtime/API, on-disk
format, dependency, version, release, tag, deployment or repository setting changed.
Remote T7.10 PR checks are unobserved at evidence preparation; the authorized wait
and implementation merge occur only after inspecting their actual results. Skipped
release/deploy jobs or code-filtered suites are not execution evidence. Platform wheels,
PG15/oldest-runtime, properties/differential/fuzz and live publication remain later
release gates. Approval of the final launch copy, channels and timing remains T7.17.

## T7.11 release rehearsal

Source: `73b8b8fa3657f68f15aaf48155cfeff734ca96d7` (merged T7.10).
Refs: T7.11; T3.7; T3.4–T3.6; `07` §§2–7; `09` §§4–4.1; `10` §4; `11` §6.
This is an alpha-version rehearsal, not stable candidate approval. Built alpha.4-version
artifacts contain current source and are **unpublished rehearsal builds**, not the
registry's already-published alpha.4 bytes. They must never be uploaded over that
version. Stable-version verification is T7.13.

### Ordered prerequisites and verification scope

T7.0–T7.10 implementation/decision PRs are merged. [PR #90](https://github.com/Joshua-Fields/pemba/pull/90)
merged at the source SHA after its [CI](https://github.com/Joshua-Fields/pemba/actions/runs/40975057401)
completed: six success checks including GitGuardian, nine code-filtered suites skipped,
none failed/pending. Local full checks are recorded above; skipped remote suites were
not executions. The maintainer authorized waiting and merging implementation PRs,
then continuing. This grants no release sign-off, tag, publication, announcement,
account approval or release-PR merge before its T7.14 human gate.

Manual workflows used `t7.11-release-rehearsal` at the exact source SHA, with build-only,
non-main conditions preventing publication/deployment. Audit, test, artifact-install
and fuzz workspaces did not modify source. Subsequent commits document evidence.

### Platform and package matrix

| Requirement | Exact-source evidence and actual result |
|---|---|
| Full main CI | [40975224698](https://github.com/Joshua-Fields/pemba/actions/runs/40975224698): 19 successful jobs, zero skipped/failed jobs. All three native OSes, Python, wasm32, package dry run/copies, rustdoc, cargo-deny, 1,000-case properties and required PG15/18. Platform suites without services are not PG evidence; separate required-service jobs supply it. |
| Manual build-only wheels | [40975448442](https://github.com/Joshua-Fields/pemba/actions/runs/40975448442): 18 successful jobs, publish correctly skipped. Five builds: Linux x86_64/aarch64, macOS arm64/x86_64, Windows x86_64. Ten installed-wheel smoke jobs: each platform on CPython 3.11/3.14; architecture/import-path assertions and SQLite/rustls/no-OpenSSL probes passed. No tag/upload/publish. |
| Wheel service paths | Same run: PG15/18 all-driver/TLS smoke each passed 63 tests, zero skips. Each plain platform smoke passed 166 tests with three known skips: two CLI tests without the separate CLI binary and one PG module. Those skips are not CLI/PG evidence; native CLI tests and required backend jobs passed separately. |
| sdist outside checkout | Same run: `/tmp/sdist-venv` rebuilt/installed the sdist and imported its site-packages extension outside checkout. Five wheels/one sdist downloaded; `.pyi`, `py.typed`, LICENSE/NOTICE and manifest/lock members checked; SHA256s below. |
| Oldest supported combination | Main CI Rocky Linux 8 CPython 3.11/SQLite 3.26.0 passed build/import/no-SQLite probe and 166 tests; three CLI/PG skips as above, not those suites' evidence. No support floor changed. |
| PG15/18 and revision order | Main CI covered privileged/normal-role foreign transactions, hostile search path, gaps, constraints, migrations, GC and supported Python drivers/TLS. All production concurrency variants had zero correctness violations and zero exhausted retries; negative allocate-at-write controls produced violations as required. Forced inversion, commit-cost and immediate-constraint variants passed. |

PG18 REPEATABLE READ writer wait p99 334.58 ms and PG15 SERIALIZABLE p99 117.44 ms
were reported above 100 ms, within the **existing** 500 ms shared-runner ceiling
(`10` §4). These are report-only under that policy, not strict-100-ms passes or a new
waiver. The largest reported production visibility-lag p99 was 14.930284 ms; this
does not bound every individual observation. Immediate-constraint long-transaction
waits near one second are expected and separately unbounded. Correctness stayed strict.
CI also reported runner-migration, Node-action deprecation and capacity notices;
successful checks do not imply those notices were absent.

### Full local and installed-artifact checks

Windows x86_64, Rust 1.99.0, CPython 3.12.10, Node 24.20.0, wasm-bindgen 0.2.129.
Required local PG18.6 and untrusted-TLS services: loopback 57732/57733. Tool-only and
all-driver venvs separate; MSVC linker explicit for Git Bash. Logs, harness and remote
snapshots are ignored `target/t7.11-*`.

| Command / check | Actual result |
|---|---|
| Full CLAUDE §5 once: fmt, all-target/all-feature Clippy, workspace tests, strict all-feature rustdoc, separate fuzz fmt/all-target build | All passed. 687 Rust tests/doctests, zero failures; 18 ignored tests excluded from this ordinary suite. Separate release/property/differential/order runs supply long-test evidence; ignored cases are not passed by the ordinary run. |
| Tool-only maturin/pytest/strict mypy | Passed build, 168 tests and typing of 15 files; one PG-module skip, not a service pass. |
| All-driver required PG18/untrusted-TLS environment | 63 Python tests, zero skips, strict mypy, Rust no-default-features and installed-extension no-SQLite/rustls/no-OpenSSL probes passed. |
| `cargo publish --workspace --dry-run --locked` | Passed packaged build verification for all publishable crates. Existing alpha.4 index-version and dry-run-aborted-upload warnings expected; no upload. |
| wasm32 Clippy and four Node smoke tests | Portable-crate and fixture-smoke-target Clippy passed with `-D warnings`; four tests passed. |
| Native/WASM conformance | 1,000 generated oracle-checked cases, 1,037 fixtures, 45,323 identical transcript lines. Native DLL creation prints the known informational linker-stdout warning. |
| Playground WASM, unit/typecheck/build/size/browser; npm audits | 170 unit tests, production/typecheck build, 578.2 KB/700 KB gzip and 72 Chromium tests passed; shipping playground and docs audits zero vulnerabilities. |
| Docs tests and strict development/no-stable builds | Nine tests and both builds passed. [Manual Playground 40975457885](https://github.com/Joshua-Fields/pemba/actions/runs/40975457885) independently passed conformance, site/browser and docs desktop/mobile/keyboard/link checks at this source; release/deploy jobs correctly skipped. |
| Verbatim README Rust quick start outside checkout | Fresh sibling project, explicit Rust 1.99.0, memory-only facade/current source path compiled and ran allowed-check/lookup assertions. The example also remains a workspace doctest. |
| Built Windows wheel outside checkout | Fresh venv asserted `0.1.0a4` and site-packages import, then passed verbatim README Python and copied SQLite/Postgres commit/forced-rollback tutorials using the artifact, not an editable import. |
| Three-store equivalence at 1,000 cases | Required PG18, `PROPTEST_CASES=1000`, `cargo test -p pemba-store-postgres --all-features --test equivalence -- --nocapture`: one property passed, zero failures/ignored, 416.72 s test time (428.11 s total). Explicitly raises the normal 32-case count. |

### Properties and fresh fuzzing

[Property run 40975454878](https://github.com/Joshua-Fields/pemba/actions/runs/40975454878)
passed all three jobs at source `73b8b8f`, with `cases=1000` and
`postgres_cases=1000`: 13 full memory/SQLite properties, 18 trace tests and four core
trace tests; each PG15/18 job passed 25 selected tests with no ignored tests in that
selection. Both PostgreSQL store selections actually ran against their service.
The printer harness fixes its count at 512, so two focused `print_roundtrip` runs
with distinct `PROPTEST_RNG_SEED=7111001` and `7112002` supply **1,024 cases per
printer property** (three tests passed per invocation). Schema robustness already
uses a 2,048-case floor. No count was silently reduced or harness changed.

Review identified one additional ignored backend property outside those workflow
selections. `PROPTEST_CASES=1000 cargo test -p pemba-store-pysqlite --test prop5 -- --ignored`
passed its fixed 1,000-case memory/PySqlite property: one passed, zero failed/ignored,
20.02 s. It used the isolated tool-only Python interpreter for embedded CPython and
the host's SQLite library. This is fresh backend evidence, not an ordinary-suite skip
relabelled as a pass.

Fresh sanitizer-enabled fuzz runs used isolated Linux Docker `rust:bookworm`, image
`sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b`,
rustc `1.101.0-nightly (92ffebece 2026-10-28)` and cargo-fuzz 0.13.2. A git archive's
PAX commit header confirms exact source `73b8b8f`. Each command was
`cargo +nightly fuzz run <target> <isolated-corpus> -- -max_total_time=610 -print_final_stats=1 -artifact_prefix=<isolated-artifacts>/`.
The three targets ran concurrently with separate corpus/artifact directories after
the instrumented build. Project source and toolchain pins were unchanged.

| Target | Actual fuzzer duration | Executions | Result / retained corpus |
|---|---:|---:|---|
| `fuzz_schema_parse` | 611 s (611.698 s command elapsed) | 6,146,736 | Exit 0; no crashes; 6,412 retained inputs, 4,649 absent from the starting corpus |
| `fuzz_tuple_parse` | 611 s (611.345 s elapsed) | 121,981,282 | Exit 0; no crashes; 1,003 retained inputs, 138 absent from the starting corpus |
| `fuzz_check_vs_reference` | 611 s (611.457 s elapsed) | 194,495 | Exit 0; no crashes; 2,118 retained inputs, 438 absent from the starting corpus |

All crash-artifact directories are empty and the container exited zero. Completed
runs span 2026-10-29 23:49:41–23:59:53 UTC. Full retained corpus, SHA256 manifest,
commands, environment and run logs are preserved in the local user deliverable
`t7.11-fuzz-evidence-73b8b8f.tar.gz` (3,281,169 bytes), SHA256
`5762b429cc2b87047da7916c7ab9ce9f35efed040e29290e22f19b2f0766403a`.
Existing committed seeds remain intact; no crash regression input was found.
This archive is local test evidence, not a package/release upload. `09` §4 lists a
manual `fuzz.yml`, but that file is absent: no remote fuzz-workflow pass is claimed.
These fresh local Linux executions fulfill the duration requirement independently.

### Semantic questions, mutants, differences and benchmarks

Independent source review examined all 63 OPEN_QUESTIONS entries by heading range.
No unresolved release blocker was found. Historical blocking OQ-001/021/024/032/045
have explicit resolutions; OQ-044's decision and T7.2 implementation are complete.
OQ-058–060 decisions are resolved. Remaining nonblocking reviews include OQ-008/014/
017/019/022–023/031/033–040/041–043/050/061–063 and formally unclosed SpiceDB
observations OQ-004–007; they are not silently marked resolved. In particular OQ-062
allows the documented no-rename path, not an implemented repair command. OQ-063 keeps
no-stable/legacy presentation. Human reporting/account/publisher confirmations in
T7.8 remain prerequisites for release sign-off/publication.

Reviewed `docs/MUTANTS.md`: its 2026-10-19 `d6f9fd3` baseline had zero survivors
(evaluator 78 caught/27 unviable; validator 72 caught/25 unviable). Unviable means
compilation failure, not equivalent behavior. Six former real gaps retain targeted
assertions for bulk stats, negated memo hits, `.all` depth, two span mutants and
direct-cycle components; these tests ran in current suites. The validator is
unchanged since that baseline; evaluator tracing/memo code has changed. No new
mutation run or transfer of old counts to this candidate is claimed, and review
found no demonstrated untested correctness gap.

KD-1/2/3 detector review matches the documented cycle/depth, narrowly classified
wildcard-schema and wildcard/subject-set exclusion shapes. KD-3 retains typed
fail-closed ambiguity and the agreeing no-wildcard re-check; the unsafe former
true/false direction stays unexplained. KD-2 skips do not count toward the required
500 comparisons. Focused negative tests, the wildcard fixture, error/memo/lookup
regressions and generated non-vacuity assertions remain. Review limitations: two
errors are intentionally considered agreement, KD-1's cycle criterion is case-level,
and dedicated probe-control-flow/arrow-branch unit tests are absent; no broader
suppression bug was demonstrated. Fresh differential outcomes are recorded separately.

Benchmark artifacts/scenarios/datasets are unchanged. Reviewed provenance and source
diffs from the recorded Rust/Python/Postgres baselines: request constructors moved
outside timed check calls; native clock alias still uses `std::time::Instant`;
OQ-054's equivalent below-head planner fix is documented. The overall generated
marker includes uncommitted changes, so it is not a reproducible whole-tree baseline
SHA. Historical same-runner point increases (for example wide groups +5.0%, exclusion
below head +4.2%, direct below head +3.1%, unchanged round trips) lack repeated-run
distributions; review cannot declare significance or conclusively attribute them to
noise. They are not new candidate measurements. The recorded batching improvements
and round-trip costs remain context-qualified. No fresh latency run, blanket
no-regression assertion or new threshold is claimed. OQ-040's cross-language write
comparison restriction and SpiceDB transport-stall caveat remain in force.

### Rehearsal artifact SHA256s

All six artifacts are from the exact-source manual wheel run above; these identify
local downloaded rehearsal builds, not registry content.

| Artifact | SHA256 |
|---|---|
| manylinux aarch64 wheel | `caa41ffcfc70616e839af8a07dafeb29c3133b8e1fd5c7a85425d489a78066b8` |
| manylinux x86_64 wheel | `faa391f760c32c5e3f2f5be49aec8aeaf6377315b9d3563c8e42418aceeb1213` |
| macOS arm64 wheel | `9585ea1ce2b5726dd67e97659a3f186e16affb10ef97513f535e5658896d637d` |
| macOS x86_64 wheel | `9b40910272401350f019651709b34ae4e380316e4cbf8cc28c0aac3fe4354ef7` |
| Windows x86_64 wheel | `6b9ec1c0281ebbf4ae46650c00efb2e6508324446529cb77efaef70b717c70e0` |
| sdist | `619ecf3dafab438e44d76a487c8bab298e9efb845204678c64f6f93e29c39294` |

### T3.7 coverage by acceptance row

| T3.7 requirement | Rehearsal coverage / remaining human boundary |
|---|---|
| README pitch, install and Python/Rust quick starts | Topics present; current-source verbatim Rust quick start ran outside checkout, Python README ran against built wheel outside checkout, packaged artifacts/imports and all-platform smoke verified above. Published stable installation is still T7.16. |
| Transaction example, schema/differences, consistency, limits, service alternative and credits | README/linked T7.4–T7.5 guides retain all named topics; fresh ordinary/required-PG tests and installed-wheel tutorials check commit/forced rollback, doc snippets, denial/error and pending semantics. Docs link/build/browser checks passed. |
| Public rustdoc and Python docstrings | T7.3 contract audit retained; fresh strict all-feature rustdoc/doctests, Python docstring/stub/error tests and mypy passed. docs.rs publication success cannot be observed before publication; candidate builds and public artifact verification remain separate. |
| CI, differential testing and blocking questions | Exact-source workflow/local matrices and semantic audit above; both differential legs completed successfully with 19 compared fixtures and 500 compared generated cases each, as detailed below. No unresolved release blocker found. |
| Final project name and rename | ADR-012/OQ-001 choose pemba; current manifests, package metadata, wheels, installed import and CLI use pemba. Historical codename text is intentional, not an unfinished rename. |
| T3.9 public API audit | T7.2's 98 declarations/nine publishable crates inventory and approved extensible constructors/check_str remain; current downstream quick start, public API tests and doctests compile. No new API decision. |
| Human release gate | Rehearsal evidence is not T7.13's stable-version candidate verification or T7.14 sign-off. Reporting/account/publisher confirmations, exact-candidate approval, release-PR merge, tags/publication and launch remain human actions. |

Fresh independent review found the visibility-lag wording error corrected above and
the extra PySqlite property coverage gap closed by its targeted run. Artifact hashes,
exact sources, platform/property/fuzz counts, retained corpus and old-report limitations
matched inspected evidence. No runtime/test/workflow source changed and no new
specification ambiguity requiring an OPEN_QUESTIONS entry was identified.

### Completed differential matrix and task disposition

[Differential run 40975451586](https://github.com/Joshua-Fields/pemba/actions/runs/40975451586)
completed both jobs successfully at source `73b8b8f`, using pinned SpiceDB 1.56.2,
`PEMBA_DIFFTEST_REQUIRED=1`, `cases=500` and the selected memory/Postgres backend.
The PostgreSQL service was used; no backend suite was skipped. Summed comparison
logs establish the actual minimum, rather than treating attempted/KD-2-skipped
cases as comparisons:

| Backend | Generated attempts / actually compared | Generated questions | Fixtures compared / questions | Result |
|---|---:|---:|---:|---|
| Memory | 663 / 500 | 500,106 | 19 / 23,990 | Two tests passed; zero failed/ignored; no unexplained mismatch |
| PostgreSQL | 665 / 500 | 499,517 | 19 / 23,990 | Two tests passed; zero failed/ignored; no unexplained mismatch |

Differences in attempted count are explained by KD-2's documented nondeterministic
schema-refusal probes; each backend still reached 500 **compared** generated cases.
PG ran 2026-10-29 23:47:07–2026-10-30 00:08:08 UTC. No new failing case required a
regression fixture. Existing KD-1/2/3 differences remain explicitly classified, not
newly waived. All required rehearsal runs completed with their actual qualifications
above; no failing/unavailable/skipped check was called passed.

This PR changes only the evidence index; all tested source/runtime/package bytes
remain from `73b8b8f`. Evidence-only PR CI is a separate merge check, not a replacement
for this exact-source matrix. Implementation merge is authorized after its applicable
checks finish; T7.12 preparation follows that merge. The eventual release PR stays
open for T7.13 stable-version verification and explicit T7.14 human approval/merge.
No release approval, tag, publication, deployment or announcement occurred here.

## T7.12 stable release preparation

Refs: T7.12 all AC; `09` §4.1; ADR-014; T3.7; OQ-057; CHANGELOG history.
Base main `77a63afbcc57023d7785f0115c1c9533fb5ec69d`: [T7.11 PR #91](https://github.com/Joshua-Fields/pemba/pull/91)
merged after [PR CI 40978150027](https://github.com/Joshua-Fields/pemba/actions/runs/40978150027)
completed with six success checks and nine intentionally code-filtered skips. Its
full exact-source rehearsal matrix above is complete. Required T7.1 decisions remain.

### Prepared changes and package scope

Workspace version and all nine versioned internal dependencies are `0.1.0`;
Python obtains the same version dynamically through maturin. `cargo update -w` ran
at the root and in the separate fuzz workspace. A TOML structural comparison against
main confirms **only own package version fields changed**: 16 root-lock entries,
seven fuzz-lock entries; no external version, checksum, source or dependency changed.

CHANGELOG has an empty Unreleased section and dated `0.1.0` section with alpha-to-stable
request/output constructor migration, `PendingRevision`/caller-commit migration and
removed Python integer conversions, plus guides/site/release changes. Historical alpha
notes remain intact. Both READMEs are byte-identical, use stable install commands and
the frozen public contract, and distinguish a versioned candidate from publication.
Installation/tutorial/troubleshooting and roadmap status agree; Python metadata links
the approved docs URL. No public API, on-disk format, supported scope or floor changes.

Existing release classification/extraction is reused. The added real-changelog
regression was observed failing before the stable section existed, then passed:
stable `0.1.0` notes include both migrations and classify as not prerelease, while
alpha/rc classify as prerelease. No packaging workflow or deployment pipeline rebuilt.

### Derived crates.io publication order — HUMAN ONLY

Derived from current `cargo metadata --no-deps` and non-dev dependency edges, not the
pre-Postgres historical list. Each crate's registry version must be available before
publishing a dependent crate. This is a plan; no publication has occurred.

| Order | Publishable crate | Internal production dependencies |
|---:|---|---|
| 1 | `pemba-schema` | None |
| 2 | `pemba-core` | schema |
| 3 | `pemba-sqlite-common` | core |
| 4 | `pemba-store-memory` | core |
| 5 | `pemba-store-postgres` | core |
| 6 | `pemba-fixtures` | core, schema, memory |
| 7 | `pemba-store-sqlite` | core, sqlite-common |
| 8 | `pemba` | core, memory, sqlite |
| 9 | `pemba-cli` | pemba, fixtures, schema |

Keep `pemba-testkit`, `pemba-difftest`, `pemba-store-pysqlite`, `pemba-py`,
`pemba-benches`, `pemba-playground` and `pemba-playground-conformance` unpublished on
crates.io (`publish=false`). Python-only source dependencies remain in the sdist;
the browser WASM crates gain no distribution/API promise. Docs.rs feature metadata,
LICENSE/NOTICE, README, `.pyi`/`py.typed` and packaged dependency contents are verified
with the candidate checks; a manifest declaration alone is not a package-build pass.

### Human publishing sequence and immutable recovery

1. **T7.14 human gate:** review T7.13's exact stable candidate SHA, every T3.7 row,
   migrations/limitations, security findings and current green required checks.
   Complete the outstanding authenticated PyPI ownership/publisher/access/2FA and
   private-report form/notification confirmations from T7.8. Record explicit approval,
   then human merges the release PR and verifies intended main matches approved content.
   Any substantive change returns to verification and new exact-candidate approval.
2. **T7.15 human only:** tag `v0.1.0` on that approved main release commit. The existing
   tag workflows build/test wheels and sdist, create the non-prerelease GitHub Release
   from the stable changelog section, attach the immutable playground archive and
   dispatch the existing single Pages assembly. An existing tag/asset is never replaced.
3. Human publishes the nine crates in the dependency order above using the approved
   source/package contents and locked dependencies. Check each registry version and
   availability before the next dependent upload; record crate versions and checksums.
   Do not interpret a dry-run's upload wording as an actual upload.
4. After tag build/sdist/all-platform/required-PG smoke tests pass, human reviews the
   protected `pypi` environment and approves trusted publishing of **those artifacts**.
   Never bypass its reviewer or substitute a locally rebuilt file. Record tag SHA,
   workflow/artifact/release URLs and registry versions. Authenticated account settings
   are a human verification requirement, not inferred from a prior successful upload.
5. **T7.16:** independently verify public packages, docs.rs, quick starts/tutorials,
   migration/status links, docs and all playground routes/share links. Only after this
   succeeds may human approve/post launch materials and activate triage in T7.17.

If a pre-publication check fails, stop and fix/review it; rerun affected evidence before
approval. After any partial publication, stop subsequent uploads, PyPI approval and
launch. Inventory exactly which immutable versions/files exist and their original
source/digests. For a transient transport/registry-delay failure with unchanged approved
source, human may retry only a confirmed missing artifact in order after availability
checks. Never blindly rerun a batch uploader over existing versions/files.

A source/packaging defect after publication needs a separately reviewed corrected
version/recovery plan and fresh verification/approval. Do not overwrite a registry
version, delete/recreate or move a tag, or rebuild an already attached playground
archive. A partial PyPI upload needs human reconciliation of original digests and a
reviewed missing-file/new-version recovery decision; no bypass or invented automation
is approved here. A Pages failure keeps launch blocked; recover through the existing
single assembler using original versioned assets, with reviewed fixes as needed.
Documentation of this sequence authorizes none of its human actions.

### T7.12 local verification

Prepared source [`5015775232b85bd2d66599a7bd24fa7f25c352c2`](https://github.com/Joshua-Fields/pemba/commit/5015775232b85bd2d66599a7bd24fa7f25c352c2);
subsequent T7.12 edits record evidence only. Windows x86_64, Rust 1.99.0,
CPython 3.12.10, Node 24.20.0, wasm-bindgen 0.2.129; required disposable PG18.6
and untrusted-TLS services on loopback 57532/57533. Initial 57832/57833 bindings were
unavailable before tests started; containers were recreated on free ports and readiness
checked. Separate tool-only and CI-pinned all-driver venvs retained. Logs/harness:
ignored `target/t7.12-*`.

| Check | Actual result |
|---|---|
| Lockfile structural comparison and manifest-derived inventory | Passed: root 16/fuzz seven own version fields only; nine publishable/seven unpublished crates and dependency order above. All nine versioned workspace dependencies use 0.1.0; maturin obtains Python 0.1.0 dynamically. |
| Focused release regression and memory-only facade doctest | New stable notes test failed before implementation, then all 19 release-script tests passed. Stable=false, alpha/rc=true classification verified through existing CLI. Facade memory-only doctest passed. |
| Docs tests and strict development/stable builds | Nine tests and both builds passed; stable candidate build metadata says 0.1.0. These are local builds, not a deployed stable site or proof of publication. |
| Full CLAUDE §5 once | Fmt, all-target/all-feature Clippy with `-D warnings`, 687 Rust tests/doctests, strict all-feature rustdoc and separate fuzz fmt/all-target build passed. Required PG18 ran; 18 ignored tests excluded (not passed by this ordinary suite). Release-length repetition belongs to T7.13. |
| Tool-only maturin/pytest/mypy | Built/installed 0.1.0; 168 tests and strict typing of 15 files passed. One PG-module skip, not a service pass. Import metadata asserted 0.1.0. |
| Separate required PG18/all-driver/untrusted-TLS environment | 63 Python tests, zero skips, strict typing, Rust no-default-features and installed-extension no-SQLite/rustls/no-OpenSSL probes passed. |
| wasm32 lint/smoke and conformance | Both Clippy commands passed with `-D warnings`; four smoke tests passed; 1,000 oracle-checked generated cases, 1,037 fixtures and 45,323 identical native/WASM transcript lines. Known informational native linker-stdout message retained, not a warning-free-build claim. |
| Playground and npm audits | 171 unit tests, typecheck/production build, 578.1 KB/700 KB gzip and 72 Chromium tests passed. Shipping playground and full docs audits report zero vulnerabilities. |
| `cargo publish --workspace --dry-run --locked` | Passed packaged builds of all nine publishable crates at 0.1.0. Dry-run-aborted-upload warnings are expected; no upload occurred. |
| Packaged Rust contents | Nine staged `.crate` archives have 0.1.0 manifests, LICENSE/NOTICE/README, preserved docs metadata and normalized production dependencies with versions and no local paths. Private/test/bench/playground crates remain unpublished. |
| `maturin sdist` and contents | Built `pemba-0.1.0.tar.gz`; all Python Rust dependency manifests, lock, `.pyi`, `py.typed`, licenses and source-only contents verified (no extension/debug/bytecode artifacts). Metadata warnings concern the intentionally unpublished testkit/pysqlite/py Rust manifests, not missing Python project URLs or a failed build. Rebuild-away verification will run on the exact T7.13 candidate. |
| Copies and independent review | All 20 LICENSE/NOTICE copies and Python README mirror match. Review found remaining alpha-only support wording; corrected in both READMEs and the related guide, preserving best-effort policy. No remaining actionable source findings. |

The inspection helper initially compared absent Cargo metadata as `null` versus an
empty table; normalizing those equivalent absent values corrected the helper and all
package checks passed. This was not a source/package fix or a waived check.
Local sdist SHA256: `425dfe622706668dd5f845ac6c8e6bb7277318e7952c38cbe646d01b1256bce5`.

T7.12 prepares a reviewable release PR; its remote checks are not observed at evidence
preparation. All release PR checks and the fresh **stable-version** T7.13 matrix must
pass before T7.14 approval. Alpha rehearsal evidence is not substituted for that final
matrix. This release PR must remain unmerged for explicit human sign-off/merge; no tag,
registry upload, deployment or announcement is authorized by these preparations.

## T7.13 exact stable candidate verification record

The canonical exact candidate SHA, per-job results, workflow URLs and artifact digests
are recorded in the **verification matrix in [release PR #92's description](https://github.com/Joshua-Fields/pemba/pull/92)**.
That record must explicitly state completion, identify the full 40-character candidate
SHA and match the PR's current head before it can supply T7.14 review evidence.
An in-progress, stale, skipped, unavailable or failed entry is not a pass.

This index is committed **before** freezing and testing that candidate. Recording
observed results in the linked PR description/local evidence manifest afterwards
avoids changing the candidate merely to write its own SHA/results into a tracked
file. The tested commit therefore includes this verification index. No subsequent
candidate edit is permitted without rerunning affected checks and recording the new
SHA. GitHub workflow metadata and downloaded artifact manifests independently tie
each result to the candidate; source-tree identity is checked for PR merge-ref jobs.

Required candidate version is **0.1.0**, Rust and Python. Alpha rehearsal results above
are historical context and cannot satisfy this matrix. The fresh record must cover:

| Requirement | Required final-candidate evidence |
|---|---|
| Full local green / package contents | CLAUDE §5 once; strict rustdoc, ordinary tests and explicit ignored-property/service runs; locked workspace publish dry run; nine packaged crates build without local production paths; licence/README/docs.rs metadata and Python stubs/typing/sdist dependencies checked. |
| Wheels and sdist | Build-only five-platform wheels on CPython 3.11/3.14; installed-extension SQLite/rustls/no-OpenSSL probes; isolated sdist rebuild away from checkout; installed version/path assertions. No publish job. |
| Native/platform/oldest/runtime services | Current required CI, oldest CPython/SQLite, required PG15/18 and all supported Python drivers/TLS, conformance/equivalence and revision-order/constraint/retry paths. Known skips and report-only latency are separately identified. |
| Properties/differential | At least 1,000 cases for full properties, host-Python SQLite and three-store equivalence; schema printer aggregate counts explicit; memory and PG difftests each prove fixtures plus at least 500 actually compared generated cases with required-service flags and known-difference review. |
| Fuzz/mutation/benchmarks | Fresh three-target runs of at least 600 seconds with exact source, no crashes and retained corpus; existing mutant disposition and benchmark changes reviewed without inventing fresh old-report results or a numerical threshold. |
| Docs/playground | Strict docs link/snippet/build and desktop/mobile/keyboard checks; wasm32 lint/smoke, native/WASM/oracle conformance, playground unit/build/size/browser tests. Candidate metadata must identify this source/version; no deployment. |
| Isolated installation/tutorials | Built stable wheel and sdist installed outside source; quick starts and SQLite/Postgres commit/forced-rollback examples, API/stubs/import/version/CLI checks. Packaged Rust crate verification and an outside-checkout Rust quick start. |
| T3.7/T3.9 and questions | Every T3.7 row mapped to fresh technical evidence and human approval boundary; 98-declaration audit/constructors retained, final name/rename complete and no unresolved release-blocking question. |

The existing T3.7 coverage map, T7.2 public inventory, semantic question audit and
T7.8 human account/reporting requirements remain references; the final record must
confirm their applicability to this exact candidate. T7.14 must review all rows and
the current SHA, then supply explicit human approval/merge. T7.15 tagging/publishing,
T7.16 public-artifact verification and T7.17 launch/activation are not executed or
approved by T7.13. No tag, registry upload, deployment or announcement is needed to
complete this verification task.
