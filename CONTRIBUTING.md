# Contributing to pemba

Start with [CLAUDE.md](CLAUDE.md), then find one task and its acceptance criteria in
[the milestones](docs/08-milestones.md). Read only that task's referenced specification
sections. The specifications govern behavior; the more specific specification wins.
Record genuine ambiguities in [OPEN_QUESTIONS](docs/OPEN_QUESTIONS.md), choose the
conservative behavior and flag it in the PR. Changing an accepted ADR requires human
sign-off. This guide describes development from source, not installation of a release.

## Prerequisites and clean checkout

Use Git, rustup, a C/C++ toolchain and Python with `venv`/pip. On Windows use Visual
Studio Build Tools with Desktop development with C++, the Windows SDK and the x64
Native Tools command prompt (PowerShell launched from it also works). On Linux install
the compiler/build essentials and Python development headers; on macOS install Xcode
Command Line Tools. Rust's bundled SQLite and native dependencies need a C compiler.

- Rust **1.99.0**, with rustfmt and Clippy, comes from `rust-toolchain.toml` automatically.
  Do not introduce a second toolchain pin or change the edition during a task.
- CPython **3.11 or newer**, with SQLite **3.26.0 or newer**. CI tests the 3.11 floor;
  use **3.12** for the separate Postgres driver environment (the pinned Django needs it).
- Node **24**, matching `actions/setup-node` in `playground.yml`; `npm ci` uses the
  committed locks. Do not use `npm install` to silently update dependencies.
- Git Bash on Windows for the existing `.sh` playground/conformance scripts. Use one
  native Windows Rust/Python/Node installation together; WSL has its own toolchains.
- Docker is needed only for service-backed tests below. Use disposable databases.

Commands below start at the repository root. `sh` blocks use Bash (Git Bash on Windows);
PowerShell alternatives are supplied where shell syntax differs.

```sh
git clone https://github.com/Joshua-Fields/pemba.git
cd pemba
git checkout main
git pull --ff-only
git switch -c t7.7-developer-onboarding  # replace with your one task's name
rustc --version
node --version
python --version
python -c "import sqlite3; print(sqlite3.sqlite_version)"
cargo build --locked -p pemba-cli
```

Create a fresh venv containing only the CI test tools and the extension; extra shared
packages can hide missing dependencies. These versions match `ci.yml`'s Python job.

```sh
python -m venv .venv
source .venv/bin/activate  # Windows Git Bash: source .venv/Scripts/activate
python -m pip install maturin==1.15.0 pytest==9.1.1 mypy==2.4.0
export PYO3_PYTHON="$(python -c 'import sys; print(sys.executable)')"
export PYTHONHOME="$(python -c 'import sys; print(sys.base_prefix)')"
# Linux embedding tests also need the interpreter's shared library:
# export LD_LIBRARY_PATH="$(python -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
maturin develop --manifest-path crates/pemba-py/Cargo.toml
python -c "import pemba; print(pemba.__file__); print(pemba.Engine.memory())"
```

In PowerShell, activate with `. .venv/Scripts/Activate.ps1` (or invoke the venv's
executables directly if script execution is restricted), and use:

```powershell
$env:PYO3_PYTHON = (python -c "import sys; print(sys.executable)")
$env:PYTHONHOME = (python -c "import sys; print(sys.base_prefix)")
```

The PyO3 variables mirror CI: `pemba-store-pysqlite` tests embed that interpreter.
`maturin develop` needs the activated venv; always use its `python -m pytest/mypy`.
Keep the default repository `target/` for the playground scripts, which read artifacts
there. Clear inherited `PEMBA_PG_*` and `PEMBA_SPICEDB_*` settings for a tool-only run;
configure them explicitly for a service run. Never source a shared secret-filled `.env`.

## Workspace map

| Location | Responsibility |
|---|---|
| `crates/pemba-schema`, `crates/pemba-core` | DSL/IR, model, store traits and evaluator |
| `crates/pemba-store-*`, `crates/pemba-sqlite-common` | Memory, Rust SQLite, Python SQLite, Postgres and driver-free SQLite SQL |
| `crates/pemba`, `crates/pemba-cli`, `crates/pemba-py` | Rust facade, CLI, Python binding/stubs/tests |
| `crates/pemba-fixtures`, `crates/pemba-testkit`, `crates/pemba-difftest` | Fixture runner, independent oracle/generators, SpiceDB comparisons |
| `testdata/fixtures/`, `examples/` | Shared scenarios/regressions and executable tutorials |
| `playground/wasm`, `playground/web`, `playground/conformance` | Browser-only WASM, frontend and native/WASM transcript comparison |
| `docs/guides/`, `docs/site/` | User guides and generated VitePress views |
| `fuzz/`, `benches/`, `.github/workflows/` | Separate fuzz workspace, benchmarks and authoritative CI setup |

Dependency direction, approved dependencies and lint policy are in
[tooling §§1–3](docs/09-repo-and-tooling.md#1-layout). The reference evaluator must stay
independent of the optimized evaluator. Python must use the host's single SQLite copy;
do not link rusqlite into the extension. WASM is for the playground only, with no
supported package/API promise.

## First fixture and focused loop

```sh
cargo run --locked -q -p pemba-cli -- test testdata/fixtures/direct.yaml
cargo test -q -p pemba-testkit --test fixtures
cargo test -q -p pemba-schema
cargo clippy -p pemba-schema --all-targets -- -D warnings
cd crates/pemba-py
python -m pytest -q tests/test_readme_example.py
cd ../..
npm ci --prefix docs/site
npm test --prefix docs/site
npm run build --prefix docs/site
```

The CLI checks fixture answers with the engine; the testkit fixture suite runs the
corpus with the independent oracle on memory and SQLite. For a bug, first save a minimal
failing YAML under `testdata/fixtures/regressions/`, or add a focused test for behavior
outside the fixture format. Keep proptest's shrunk seeds in `proptest-regressions/`.
See [testing §3](docs/07-testing.md#3-fixture-files) for YAML keys, expected errors and
atomic write steps. Assert denial separately from errors; errors must never turn into
an allow or denial. Write tests first where the spec gives exact behavior. Review
diagnostic/snapshot changes intentionally; do not accept all snapshots blindly.

While iterating, test/lint the touched crate and relevant dependents. A public core API
change also needs facade, testkit and Python coverage. For frontend changes, build WASM
before unit tests; see the next section. Avoid rerunning the entire workspace per edit.

## Required checks before a PR

After focused checks are green, run [CLAUDE.md §5](CLAUDE.md#5-local-green-definition)
**once** before opening/updating the PR, fixing failures. From the root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
# When slow/property tests are touched (requires their services; see below):
cargo test --workspace --all-features -- --ignored
cargo doc --workspace --no-deps
(cd fuzz && cargo fmt -- --check)
(cd fuzz && cargo check --all-targets)
cd crates/pemba-py
maturin develop
python -m pytest -q -rs
python -m mypy --strict tests
cd ../..
```

`fuzz/` is a separate workspace: root `--workspace`/`--all` does not reach it. Its build
check uses pinned stable Rust; actual fuzzing needs nightly. For documentation/API
changes also run CI's strict all-feature docs:
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`
(PowerShell: set `$env:RUSTDOCFLAGS = '-D warnings'` first).
CI also checks root LICENSE/NOTICE copies and the mirrored Python README byte for byte.
Keep these copies synchronized when editing the originals. Run `cargo deny check` for
dependency changes, matching CI's advisory/licence policy.

For docs/site changes, run the docs commands above, then its no-stable build and browser
harness from [the site README](docs/site/README.md). `CONTRIBUTING.md` is the contributor
page source; do not maintain a second copy. Warnings, broken local links/anchors/assets
fail the build. Existing deployment uses one Pages assembly; never deploy it manually
as part of onboarding.

For changes that affect engine answers, fixtures or the playground, reproduce the
`wasm` job in `ci.yml` and `conformance`/`site` jobs in `playground.yml`:

```sh
rustup target add wasm32-unknown-unknown
# Read the version from Cargo.lock (also how CI obtains it), never from memory:
version=$(cargo metadata --format-version 1 --locked | node -e 'let s="";process.stdin.on("data",d=>s+=d);process.stdin.on("end",()=>console.log(JSON.parse(s).packages.find(p=>p.name==="wasm-bindgen").version))')
cargo install wasm-bindgen-cli --version "$version" --locked
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
cargo clippy --locked --target wasm32-unknown-unknown -p pemba-core -p pemba-schema -p pemba-store-memory -p pemba-fixtures -p pemba-playground -- -D warnings
cargo clippy --locked --target wasm32-unknown-unknown -p pemba-fixtures --test wasm_smoke -- -D warnings
cargo test --locked --target wasm32-unknown-unknown -p pemba-fixtures --test wasm_smoke
bash playground/conformance/run.sh
npm ci --prefix playground/web
npm run wasm --prefix playground/web
npm test --prefix playground/web
npm run build --prefix playground/web
npm run size --prefix playground/web
npx --prefix playground/web playwright install chromium  # Linux CI: --with-deps
npm run e2e --prefix playground/web
npm audit --omit=dev --prefix playground/web
```

Conformance generates 1,000 reference-checked cases and compares native and WASM
transcripts byte for byte. The frontend size budget is 700 KB gzip. A missing generated
WASM module means the prerequisite build has not run, not a test to skip.

## Service-backed checks

Use disposable services from `ci.yml`/`difftest.yml`, never an application database.
Postgres tests create/drop databases, roles and schemas; their admin URL needs those
privileges, while tests also exercise restricted application roles. CI runs Postgres
**15 and 18**; a local run against one version does not prove the other passed.

```sh
docker run -d --rm --name pemba-dev-pg -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1:5432:5432 postgres:18
docker exec pemba-dev-pg pg_isready -U postgres  # wait for readiness before tests
export PEMBA_PG_URL=postgres://postgres@127.0.0.1:5432/postgres
export PEMBA_PG_REQUIRED=1
cargo test -p pemba-store-postgres --all-features
cargo test -p pemba-store-postgres --no-default-features
# On a separate disposable server with an untrusted TLS certificate, as in ci.yml:
# export PEMBA_PG_TLS_URL=postgres://postgres@127.0.0.1:5433/postgres
```

PowerShell environment syntax is `$env:PEMBA_PG_REQUIRED = '1'` and
`$env:PEMBA_PG_URL = 'postgres://postgres@127.0.0.1:5432/postgres'`.
`PEMBA_PG_TLS_URL` must point to the separate TLS-enabled server: the rejection test
expects its certificate to be untrusted. Copy the TLS container setup from `ci.yml`'s
Postgres job; do not weaken verification to make it pass.

Keep the tool-only venv above intact. In a **second**, fresh Python 3.12 venv, activate
it, set the PyO3 variables as above and install the same CI tools plus CI's drivers:

```sh
python -m pip install maturin==1.15.0 pytest==9.1.1 mypy==2.4.0 "psycopg[binary]==3.3.6" psycopg2-binary==2.9.13 sqlalchemy==2.1.6 django==6.1.2 greenlet==3.5.6
maturin develop --manifest-path crates/pemba-py/Cargo.toml
cd crates/pemba-py
python -m pytest -q -rs tests/test_postgres.py tests/test_errors.py
python -m mypy --strict tests
python ci/check_tls.py
python ci/check_no_sqlite.py
cd ../..
docker stop pemba-dev-pg
```

Without `PEMBA_PG_URL`, Rust tests can report success after printing “not run” and Python
skips the module; **neither is a Postgres pass**. `PEMBA_PG_REQUIRED=1` fails on a missing
URL (and missing required psycopg); install every driver above to avoid optional-driver
skips. A missing TLS URL is still a skip. `PEMBA_CLI` can point Python tests to the
absolute compiled CLI executable when a custom Cargo target directory prevents
discovery. Report all skips and limitations in PR evidence.

For evaluator/schema/fixture changes use the pinned SpiceDB setup in `difftest.yml`:
`PEMBA_SPICEDB_URL=http://127.0.0.1:8443`, `PEMBA_SPICEDB_KEY=difftest`,
`PEMBA_DIFFTEST_REQUIRED=1`, and `PEMBA_DIFFTEST_STORE=memory` or `postgres` (also needs
`PEMBA_PG_URL`). Run `cargo test -p pemba-difftest --test difftest -- --ignored --nocapture --test-threads=1`.
`PEMBA_DIFFTEST_CASES` raises the minimum 500 compared generated cases. Without the
required flag an unavailable SpiceDB can look green without comparing anything.

## Longer verification and review boundaries

Release rehearsal is a separate gate, not a reason to run all release jobs on every
docs edit. T7.11/T7.13 and [the release checklist](docs/09-repo-and-tooling.md#41-release-checklist)
require exact-commit evidence for all platforms/wheels/sdist, package dry runs, oldest
Python/SQLite, all Postgres/driver combinations, ≥1,000-case properties, revision-order
tests, both ≥500-case differential backends, each fuzz target ≥10 minutes, mutation
review and docs/playground checks. Record commands, source SHA, actual counts, workflow
URLs and limitations in [the release evidence index](docs/RELEASE-0.1.0.md).

For focused long checks: `cargo test -p pemba-testkit --test properties -- --ignored`;
`cargo test -p pemba-store-postgres --all-features --test revision_order -- --ignored --nocapture`.
`PROPTEST_CASES` raises case counts; `PEMBA_PG_STRICT_LATENCY=1` restores tight local
latency thresholds (correctness remains strict). Actual fuzzing uses `cargo +nightly
fuzz run <target>` from `fuzz/`; target names and corpus policy are in
[testing §5](docs/07-testing.md#5-fuzzing-fuzz-nightly-toolchain).
Do not call unavailable, skipped or failing checks passed. Docs-only CI skips heavy
jobs; its green status is not release evidence.

Use one task per branch/PR (`t7.<N>-<short-description>` for M7), conventional commits,
and a PR targeting `main` listing acceptance coverage, tests, limitations and decisions.
No handwritten `unsafe`, no library `unwrap`/`expect`/`panic`, and doc comments on every
public item. Correctness and fail-closed behavior take priority over speed; optimizations
need oracle agreement tests. API/on-disk format changes need a matching spec/migration
update and explicit notice. Justify every dependency, check its current version and
follow the approved list/licences; new async library runtimes, ORMs or incompatible
licences require asking first. Do not copy or translate SpiceDB code: it is a design
reference only. Any adaptation requires prior human approval, retained attribution
and a NOTICE entry (ADR-011).

Never commit generated artifacts, `target/`, venvs, databases, credentials or `.env`.
Do not expose secrets to fork workflows. Stop after the requested PR; merging, release
approval, tags, package uploads, deployment and announcements remain human actions.
