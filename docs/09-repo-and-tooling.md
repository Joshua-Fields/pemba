# 09 — Repository layout, tooling and conventions

## 1. Layout

```text
warden/
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
│   ├── warden-schema/          # lexer, parser, validator, IR, printer. No I/O.
│   ├── warden-core/            # model types, store traits, write logic, evaluator, Engine
│   ├── warden-store-memory/
│   ├── warden-sqlite-common/   # driver-free SQLite: SQL text, migration runner, row decoding
│   │   └── migrations/         # 0001_init.sql, ...
│   ├── warden-store-sqlite/    # rusqlite (bundled SQLite), for Rust
│   ├── warden-store-pysqlite/  # CPython's sqlite3, for warden-py (ADR-006, publish = false)
│   ├── warden/                 # facade: re-exports core + stores behind features
│   ├── warden-cli/             # binary `warden`
│   ├── warden-fixtures/        # fixture format + engine-only runner (`warden test`), with an oracle hook
│   ├── warden-testkit/         # reference evaluator (the fixture oracle), generators, conformance suite (publish = false)
│   ├── warden-difftest/        # SpiceDB differential tests (publish = false)
│   └── warden-py/              # PyO3 bindings + python/warden/*.py, *.pyi, tests/, ci/
├── testdata/
│   └── fixtures/               # YAML fixtures (docs/07 §3), regressions/
├── benches/                    # criterion benches, spicedb/ comparison scripts, RESULTS.md
├── fuzz/                       # cargo-fuzz targets + corpus
└── .github/workflows/          # ci.yml, wheels.yml, difftest.yml, fuzz.yml (manual)
```

Dependency direction (no cycles, enforced by review):
`schema` ← `core` ← `sqlite-common` ← `store-*` ← `warden` ← `cli`, `py`; `py` uses
`warden` without its `sqlite` feature plus `store-pysqlite`, and must never link rusqlite
(CI checks the built extension); `fixtures` depends on `core`, `schema` and
`store-memory`, and `cli` depends on it for `warden test`; `testkit` depends on `core`,
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

Not allowed without asking: async runtimes (`tokio`, `async-std`) in library crates, ORMs,
anything with a non-permissive licence (`deny.toml` allows MIT, Apache-2.0, BSD-2/3,
ISC, Unicode-DFS/Unicode-3.0, Zlib).

## 3. Lints and conventions

`[workspace.lints]`:
```toml
[workspace.lints.rust]
unsafe_code = "forbid"          # overridden to "allow" only in warden-py if PyO3 requires
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
(the wheel builds and SpiceDB runs are slow and change rarely).

The repository is public. Never commit secrets (tokens, keys, `.env`, credentials in test
data). CI must not print secrets: pass them through `secrets.*` into `env`, never echo
them, never `set -x` around them, and do not expose them to workflows triggered from forks.

- `ci.yml` (push to main, PR): fmt, clippy, test (ubuntu, macos, windows; property 1 at 64
  cases plus saved regression seeds), properties 1 to 5 and schema change at ≥ 1 000 cases
  each (`prop1` job, ubuntu only, `--ignored` in `warden-testkit`'s `properties` test),
  `cargo deny check`, `cargo doc`, Python tests (`maturin develop`, `pytest`,
  `mypy --strict` on the tests and sample) on ubuntu, macos and windows, the oldest supported combination (Rocky Linux 8: CPython 3.11, SQLite
  3.26.0, `python-oldest`), and `cargo publish --workspace --dry-run` for every
  publishable crate (`publish`). Publishing to PyPI/crates.io is a manual human step.
- `wheels.yml` (`v*` tags + manual only, not PRs): maturin-action builds one abi3
  (`cp311-abi3`) wheel per ADR-014 platform (manylinux 2_17 x86_64 and aarch64, macOS
  arm64 and x86_64, Windows x86_64), checks it contains no SQLite
  (`ci/check_no_sqlite.py`), then installs each wheel on its platform (aarch64 under QEMU,
  macOS x86_64 under Rosetta 2) with CPython 3.11 and 3.14 and runs the whole pytest suite
  against it. Wheels are uploaded as artifacts only. Run it by hand after changing the
  Python packaging (`pyproject.toml`, `warden-py/Cargo.toml`, `ci/`).
- `proptest.yml` (weekly + manual): the same property tests at a raised
  `PROPTEST_CASES` (10 000 on the weekly run; an input on a manual run), for soaking.
  Test builds use `opt-level = 3` for `warden-core`, `warden-testkit`,
  `warden-store-memory` and `warden-schema` (`[profile.test.package.*]`).
- `difftest.yml` (weekly + manual): SpiceDB `serve-testing` (`authzed/spicedb:v1.56.2`) in
  Docker, `cargo test -p warden-difftest --test difftest -- --ignored` (`07` §4).
- `oq032-probe.yml` (manual, and PRs touching the probe): the T3.2 SQLite-copies probe.
- `fuzz.yml` (manual): nightly toolchain, each target for a fixed time.
- Weekly: `cargo-mutants` on `warden-core::eval` and `warden-schema` validator.

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
