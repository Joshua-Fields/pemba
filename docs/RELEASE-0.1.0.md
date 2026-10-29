# 0.1.0 release evidence

This is the evidence index required by M7. No stable release is approved or published.
T7.2–T7.4 implementation evidence is recorded here; T7.5–T7.17 remain separate
tasks and gates. T7.3 is merged; T7.4 requires PR review/merge.

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
