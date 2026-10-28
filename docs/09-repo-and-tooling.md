# 09 — Repository layout, tooling and conventions

## 1. Layout

```text
pemba/
├── CLAUDE.md
├── README.md
├── LICENSE                     # Apache-2.0
├── NOTICE
├── Cargo.toml                  # [workspace], shared [workspace.lints], [workspace.dependencies]
├── rust-toolchain.toml         # pinned stable channel
├── rustfmt.toml
├── deny.toml                   # cargo-deny: licences, advisories, duplicates
├── docs/                       # this spec + OPEN_QUESTIONS.md + MUTANTS.md
├── crates/
│   ├── pemba-schema/          # lexer, parser, validator, IR, printer. No I/O.
│   ├── pemba-core/            # model types, store traits, write logic, evaluator, Engine
│   ├── pemba-store-memory/
│   ├── pemba-sqlite-common/   # driver-free SQLite: SQL text, migration runner, row decoding
│   │   └── migrations/         # 0001_init.sql, ...
│   ├── pemba-store-sqlite/    # rusqlite (bundled SQLite), for Rust
│   ├── pemba-store-pysqlite/  # CPython's sqlite3, for pemba-py (ADR-006, publish = false)
│   ├── pemba/                 # facade: re-exports core + stores behind features
│   ├── pemba-cli/             # binary `pemba`
│   ├── pemba-fixtures/        # fixture format + engine-only runner (`pemba test`), with an oracle hook
│   ├── pemba-testkit/         # reference evaluator (the fixture oracle), generators, conformance suite (publish = false)
│   ├── pemba-difftest/        # SpiceDB differential tests (publish = false)
│   └── pemba-py/              # PyO3 bindings + python/pemba/*.py, *.pyi, tests/, ci/
├── testdata/
│   └── fixtures/               # YAML fixtures (docs/07 §3), regressions/
├── benches/                    # criterion benches, spicedb/ comparison scripts, RESULTS.md
├── examples/                   # runnable Python examples, each run by pemba-py/tests/test_examples.py
├── fuzz/                       # cargo-fuzz targets + corpus
├── playground/                 # browser playground (docs/11-playground.md): wasm/ (crate pemba-playground, publish = false), web/ (Vite + TS), conformance/
└── .github/workflows/          # ci.yml, wheels.yml, difftest.yml, fuzz.yml (manual)
```

Dependency direction (no cycles, enforced by review):
`schema` ← `core` ← `sqlite-common` ← `store-*` ← `pemba` ← `cli`, `py`; `py` uses
`pemba` without its `sqlite` feature plus `store-pysqlite`, and must never link rusqlite
(CI checks the built extension); `fixtures` depends on `core`, `schema` and
`store-memory`, and `cli` depends on it for `pemba test`; `testkit` depends on `core`,
the stores and `fixtures`, and is only ever a dev-dependency (OQ-024). Its workspace entry
has no version, so `cargo publish` strips it from published crates' dev-dependencies, and
`cargo deny` (`allow-wildcard-paths`) still refuses it as a normal dependency of a
published crate.

Facade features: `sqlite` (default), `memory` (default).

## 2. Approved dependencies

Check crates.io for the current version when adding; record the reason in the summary.

| Purpose | Crate |
|---|---|
| Errors | `thiserror` |
| Tracing | `tracing` (+ `tracing-subscriber` in CLI/tests only) |
| Interned/shared strings | `std::sync::Arc<str>` first; `smol_str` only if benchmarks justify |
| Hashing | `ahash` or `rustc-hash` for hot-path maps (justify with a bench) |
| Schema hash | `sha2` |
| Lexer (optional) | `logos` — or hand-written |
| Snapshot tests | `insta` |
| Property tests | `proptest` |
| Fuzzing | `cargo-fuzz`, `arbitrary` |
| SQLite | `rusqlite` with `bundled` |
| Connection pool (optional) | `r2d2` + `r2d2_sqlite`, or hand-rolled |
| Schema hot-swap | `arc-swap` |
| YAML fixtures | `serde-saphyr` (`deserialize` feature only) + `serde` (derive); chosen in T1.5 because `serde_yaml` is archived, `serde_yaml_ng` / `serde_norway` are unmaintained forks of it with no release since 2024, and `serde_yml` has unsoundness concerns; `serde-saphyr` is pure Rust (`forbid(unsafe_code)`), actively released, and needs no `unsafe-libyaml` |
| CLI | `clap` (derive), `anstream` or `owo-colors` |
| Python | `pyo3` (abi3 feature), built with `maturin` |
| HTTP (difftest only) | `ureq` |
| Benchmarks | `criterion` |
| Clock on wasm32 | `web-time`, in `pemba-core` under `cfg(target_arch = "wasm32")` only (ADR-016): `std::time::Instant::now` traps on `wasm32-unknown-unknown`; on native targets it re-exports `std::time`, so native builds are unchanged |
| JSON (playground API only) | `serde_json`, in `pemba-playground` only (T6.2); already in the tree as a dev-dependency of `pemba-core` and `pemba-difftest` |
| WASM bindings (playground only) | `wasm-bindgen`, in `pemba-playground` only (ADR-016); the CLI pinned to the same version in CI |
| Playground frontend (npm, `playground/web`) | `typescript`, `vite`, `codemirror` + `@codemirror/lang-yaml` + `@codemirror/lint`, `vitest`, `@playwright/test`; lockfile committed, Dependabot, `npm audit --omit=dev` in CI |

Not allowed without asking: async runtimes (`tokio`, `async-std`) in library crates, ORMs,
anything with a non-permissive licence (`deny.toml` allows MIT, Apache-2.0, BSD-2/3,
ISC, Unicode-DFS/Unicode-3.0, Zlib).

## 3. Lints and conventions

`[workspace.lints]`:
```toml
[workspace.lints.rust]
unsafe_code = "forbid"          # overridden to "allow" only in pemba-py if PyO3 requires
missing_docs = "warn"
unreachable_pub = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
module_name_repetitions = "allow"
must_use_candidate = "allow"
```
Tests/benches: `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`.

Other conventions:
- Public types `#[non_exhaustive]` where future fields/variants are likely (errors, configs, responses).
- Builders for configs; no public struct literal construction of configs.
- No global state; no `lazy_static`/`once_cell` singletons for engine data.
- Module docs (`//!`) explain the module's role and link to the spec section it implements,
  e.g. `//! Implements docs/04-evaluation.md §3`.

## 4. CI workflows

Every workflow has a concurrency group per workflow and ref. On pull requests a newer
push cancels the older run (`cancel-in-progress` only for `pull_request` events); runs on
main, tags, schedules and manual dispatches always finish. Only `ci.yml` runs on every PR
(the wheel builds and SpiceDB runs are slow and change rarely); `difftest.yml` runs on the
PRs that can change an answer (below).

The repository is public. Never commit secrets (tokens, keys, `.env`, credentials in test
data). CI must not print secrets: pass them through `secrets.*` into `env`, never echo
them, never `set -x` around them, and do not expose them to workflows triggered from forks.

- `ci.yml` (push to main, PR): fmt, clippy, test (ubuntu, macos, windows; property 1 at 64
  cases plus saved regression seeds), properties 1 to 5 and schema change at ≥ 1 000 cases
  each (`prop1` job, ubuntu only, `--ignored` in `pemba-testkit`'s `properties` test),
  `cargo deny check`, `cargo doc`, Python tests (`maturin develop`, `pytest`,
  `mypy --strict` on the tests and sample) on ubuntu, macos and windows, the oldest supported combination (Rocky Linux 8: CPython 3.11, SQLite
  3.26.0, `python-oldest`), and `cargo publish --workspace --dry-run` for every
  publishable crate (`publish`), and that the published crates and `pemba-py` carry
  byte-identical copies of the root `LICENSE` and `NOTICE` (`package-files`). Publishing to
  crates.io is a manual human step. The `wasm` job (ADR-016, T6.1) runs `cargo clippy
  --target wasm32-unknown-unknown` on `pemba-core`, `pemba-schema`, `pemba-store-memory`,
  `pemba-fixtures` and `pemba-playground`, then `pemba-fixtures`' `wasm_smoke` test under Node
  (`wasm-bindgen-test-runner`, the version in `Cargo.lock`): a check, a
  `check_with_trace`, both lookups and a fixture, so a call that traps on wasm32 fails CI.
- `playground.yml` (pushes to main, manual, and PRs touching `crates/`, `playground/`,
  `testdata/`, `Cargo.toml`, `Cargo.lock` or the workflow itself): the playground
  conformance check (`docs/11-playground.md` §6, T6.3), `playground/conformance/run.sh`.
  It generates 1 000 cases as fixtures and checks each against the reference evaluator.
  It then writes the transcript of those cases and `testdata/fixtures` natively and from
  the `wasm-release` `.wasm` under Node, and fails on any byte of difference. On failure,
  the transcripts are uploaded as an artifact. It takes about a minute after the build.
  The `site` job (T6.4) works in `playground/web`:
  - `npm ci`, then `npm audit --omit=dev` (only what ships to browsers);
  - type-check and Vitest;
  - `vite build`, with a **700 KB gzip budget** for the whole site (`scripts/size.mjs`);
  - a Playwright smoke test against the build;
  - it uploads `dist/` as an artifact.
  The workflow also runs on `v*` tags (T6.9, `docs/11-playground.md` §5.1). The `release`
  job (tags only, after `conformance` and `site`) checks the tag names the workspace
  version, creates the tag's GitHub Release or updates it if it exists (pre-release only
  for a version with a prerelease part such as `-alpha.N`, `scripts/release.mjs
  prerelease`; notes from the version's `CHANGELOG.md` section, `scripts/release.mjs
  notes`; a release made by hand keeps its title), and only then attaches
  `playground-<version>.tar.gz`, built with `base` `/pemba/v/<version>/`. An asset
  already attached is never rebuilt; a re-run only refreshes the notes. The
  `deploy` job (tags and pushes to main, after `release` on a tag) assembles the whole
  Pages site: every release's `playground-*.tar.gz` unpacked at `/v/<version>/`, the
  newest release (`scripts/release.mjs newest`) rebuilt from its tag with `base`
  `/pemba/` at the root (a redirect to `/next/` before the first one, OQ-057), and main
  at `/next/`. Its workflow test, `scripts/check-site.mjs`, fails unless every release
  asset has its `/v/<version>/`, each page's own URLs stay under its path, the root's
  footer version is the newest release, and only `/next/` carries the "development
  build (main)" label; then `actions/deploy-pages` publishes it.
  One deploy runs at a time (`concurrency: pages`). The page footer shows the version and
  commit (`vite.config.ts`).
  Dependabot (`.github/dependabot.yml`) updates the npm dependencies weekly. Cargo
  crates are still updated by hand.
- `wheels.yml` (`v*` tags, manual, and pull requests touching `Cargo.lock`, any
  `Cargo.toml`, `crates/pemba-py/pyproject.toml` or the workflow itself, so a new native
  dependency fails on its PR, not at release): maturin-action builds one abi3
  (`cp311-abi3`) wheel per ADR-014 platform (manylinux 2_17 x86_64 and aarch64, macOS
  arm64 and x86_64, Windows x86_64), checks it contains no SQLite
  (`ci/check_no_sqlite.py`) and that its Postgres TLS is rustls with no OpenSSL
  (`ci/check_tls.py`, T5.10), then installs each wheel on its platform (macOS x86_64
  under Rosetta 2) with CPython 3.11 and 3.14 and runs the whole pytest suite against it.
  Linux aarch64 is built and tested natively on an `ubuntu-24.04-arm` runner, in the
  manylinux2014 aarch64 image: the cross-compiling image's GCC could not build `ring`'s
  ARM assembly (0.1.0-alpha.3). `smoke-postgres` installs the Linux x86_64 wheel and runs
  `tests/test_postgres.py` with every driver against a Postgres 15 and 18 service
  container, plus a TLS server with an untrusted certificate that the wheel must refuse.
  It also builds an sdist and checks a wheel built from it imports. On a `v*`
  tag, `publish` uploads the wheels and sdist to PyPI by trusted publishing, after every
  smoke test passes and a required reviewer approves the `pypi` environment. A manual run
  builds and tests only. Run it by hand after changing the
  Python packaging (`pyproject.toml`, `pemba-py/Cargo.toml`, `ci/`).
- `proptest.yml` (weekly + manual): the same property tests at a raised
  `PROPTEST_CASES` (10 000 on the weekly run; an input on a manual run), for soaking.
  Test builds use `opt-level = 3` for `pemba-core`, `pemba-testkit`,
  `pemba-store-memory` and `pemba-schema` (`[profile.test.package.*]`).
- `difftest.yml` (weekly + manual, and pull requests touching `crates/pemba-core`,
  `crates/pemba-schema`, `crates/pemba-difftest`, `testdata/fixtures`, `Cargo.lock` or the
  workflow itself): SpiceDB `serve-testing` (`authzed/spicedb:v1.56.2`) in Docker,
  `cargo test -p pemba-difftest --test difftest -- --ignored` (`07` §4).
- `oq032-probe.yml` (manual, and PRs touching the probe): the T3.2 SQLite-copies probe.
- `fuzz.yml` (manual): nightly toolchain, each target for a fixed time.
- Weekly: `cargo-mutants` on `pemba-core::eval` and `pemba-schema` validator.

### 4.1 Release checklist

A release PR bumps the version and nothing CI cannot check; tagging and publishing are
manual human steps after it merges.

1. Bump `version` and every internal dependency's `version` in the root `Cargo.toml`.
2. `cargo update -w` at the root **and in `fuzz/`** (`cd fuzz && cargo update -w`): `fuzz/`
   is its own workspace with its own `Cargo.lock`, which the root command does not touch,
   and a stale one fails `cargo check` there. Both diffs should change only pemba's own
   crate versions.
3. `CHANGELOG.md`: turn `[Unreleased]` into a dated section, add an empty `[Unreleased]`,
   update the version note and the compare links.
4. README status and install lines; copy `README.md` to `crates/pemba-py/README.md`
   (`package-files` checks they match).
5. Full local check (`CLAUDE.md` §5) and `cargo publish --workspace --dry-run`. The
   release PR changes `Cargo.lock`, so `wheels.yml` runs on it by itself (build and smoke
   on every platform, no upload). It must be green before merge; a manual run on the
   branch is the same thing, for when the PR run was cancelled or is stale.
6. After merge (human): tag `v<version>` on main (`wheels.yml` uploads to PyPI after
   review), then `cargo publish` the crates.
7. Pages (`playground.yml` on the tag): the GitHub Release has
   `playground-<version>.tar.gz`, the deploy is green, and both
   `https://joshua-fields.github.io/pemba/` and `…/pemba/v/<version>/` load, with
   `<version>` in their footers.

## 5. Git conventions

- Conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`, `perf:`, `chore:`).
- One task (`docs/08`) per branch/PR where practical; PR description lists the ACs met.
- Never commit generated wheels, `target/`, local `.db` files, or `.env`.

## 6. `docs/OPEN_QUESTIONS.md` format

```markdown
## OQ-007 — Does SpiceDB accept arrows over relations that allow wildcards?
- Raised in: T0.4
- Blocking: no
- Current choice: reject (E_ARROW_OVER_WILDCARD), conservative
- Resolve by: differential test in T3.4
- Resolution: (fill in)
```

Seed it with these known questions at T0.1:
1. Final project name (ADR-012) — blocking for 0.1.
2. Postgres revision ordering (ADR-005) — not blocking for 0.1.
3. Async store story before Postgres (ADR-003) — not blocking.
4. `.all()` over zero related objects (ADR-013) — resolve in T3.4.
5. Self membership semantics vs SpiceDB (`04` §2) — resolve in T3.4.
6. Arrows over wildcard relations (`03` §4.2) — resolve in T3.4.
7. Type-prefix segment rules and max type name length (`02` §1.1) — resolve in T3.4.
8. Performance targets — set after T3.5.
