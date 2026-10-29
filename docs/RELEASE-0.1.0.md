# 0.1.0 release evidence

This is the evidence index required by M7. No stable release is approved or published.
Only T7.2 is implemented here; T7.3–T7.17 remain separate tasks and gates.

## Preceding gates

| Requirement | Source | Completed task / evidence | Revision / result |
|---|---|---|---|
| Ordered M7 plan | T7.0 | [PR #80](https://github.com/Joshua-Fields/pemba/pull/80), merged | main `fee1cac`; merged 2026-10-29 |
| Maintainer scope and publication decisions | T7.1; OQ-044, OQ-058–OQ-060 | [PR #81](https://github.com/Joshua-Fields/pemba/pull/81), merged | main `8852eba`; merged 2026-10-29 |
| Previous implementation PR merged | M7 ordering | [PR #78](https://github.com/Joshua-Fields/pemba/pull/78), alpha.4 release preparation | merged; earlier release baseline only, not new verification evidence |

T7.1 approves sync Postgres scope, only Rust `check_str`, the extensible-type audit,
VitePress routes/content policy, and best-effort owner-led security/triage. Private
vulnerability reporting activation/verification remains human work at its later gates.
PR #79 is still open and unrelated to the T7.2 dependency gate; reassess it in T7.6/T7.11.

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
T7.3 and all later tasks have not started.
