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
│   ├── warden-store-sqlite/
│   │   └── migrations/         # 0001_init.sql, ...
│   ├── warden/                 # facade: re-exports core + stores behind features
│   ├── warden-cli/             # binary `warden`
│   ├── warden-testkit/         # reference evaluator, generators, fixture runner, conformance suite (publish = false)
│   ├── warden-difftest/        # SpiceDB differential tests (publish = false)
│   └── warden-py/              # PyO3 bindings + python/warden/*.py, *.pyi, tests/
├── testdata/
│   └── fixtures/               # YAML fixtures (docs/07 §3), regressions/
├── benches/                    # criterion benches, spicedb/ comparison scripts, RESULTS.md
├── fuzz/                       # cargo-fuzz targets + corpus
└── .github/workflows/          # ci.yml, wheels.yml, difftest.yml, fuzz.yml (manual)
```

Dependency direction (no cycles, enforced by review):
`schema` ← `core` ← `store-*` ← `warden` ← `cli`, `py`; `testkit` depends on `core` + stores
(dev-dependency elsewhere, except `cli`, whose `warden test` runs the fixture runner, until
T3.3 moves the fixture format and runner into a small publishable crate: OQ-024).

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

- `ci.yml` (push/PR): fmt, clippy, test (ubuntu, macos, windows; property 1 at 64 cases
  plus saved regression seeds), property 1 at ≥ 1 000 cases (`prop1` job, ubuntu,
  `--ignored` in `warden-testkit`'s `properties` test), `cargo deny check`,
  `cargo doc`, Python tests (`maturin develop`, `pytest`, `mypy --strict` on the tests and
  sample) on ubuntu, macos and windows, coverage report (ubuntu).
- `wheels.yml` (tag + manual): maturin-action matrix for ADR-014 platforms; smoke test;
  upload artifacts. Publishing to PyPI/crates.io is a manual human step.
- `proptest.yml` (manual): the same property 1 run at a larger `PROPTEST_CASES` (an
  input), for soaking. Test builds use `opt-level = 3` for `warden-core`, `warden-testkit`,
  `warden-store-memory` and `warden-schema` (`[profile.test.package.*]`).
- `difftest.yml` (manual + weekly): SpiceDB in Docker, `cargo test -p warden-difftest -- --ignored`.
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
