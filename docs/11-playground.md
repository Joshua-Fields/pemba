# 11 — Playground (design)

Status: **proposed** (T6.0, 2026-10-28). Nothing here is built yet. WASM stays out of
scope (`CLAUDE.md` §6) until T6.0 is accepted and the amendment in §2 lands.

Evidence: a throwaway spike on the unmerged branch `t6.0-playground-spike`,
`spike/playground-wasm/` (wasm-bindgen 0.2.129, rustc 1.99.0, Chromium from Playwright
1.64.1, Windows 11). Its README has the commands and raw numbers.

---

## 0. Summary and decisions needed

1. **Run the real engine as WASM in the browser, served as static files from GitHub
   Pages.** No server, no running cost, and nothing the user types leaves their machine.
   The spike ran all 18 fixtures (273 assertions) correctly in Chromium. A check takes
   7.5 µs at p50 there, against 5.3 µs natively.
2. **Lift the WASM exclusion for the playground build only** (§2). This is not a
   published npm/WASM package and comes with no stability promise.
3. **Frontend: TypeScript + Vite + CodeMirror 6, no framework** (§4).
4. **Correctness: CI runs the deployed `.wasm` over the fixture corpus plus 1 000
   generated cases and requires its answer transcript to match the native one byte for
   byte** (§6).

Decisions for the maintainer:

- **D1.** Accept the WASM-in-browser approach and the §2 amendment to `CLAUDE.md` §6.
- **D2.** The clock on wasm32 (§1.2). Recommended: `web-time` as a wasm32-only dependency
  of `pemba-core`. The alternative is a zero-duration clock with no new dependency.
- **D3.** Which builds deploy (§5). Recommended: deploy each release tag, so the
  playground matches what `pip install` gives, and deploy `main` to `/next/`.
- **D4.** The URL. The default is `joshua-fields.github.io/pemba/`. A custom subdomain
  (e.g. `play.pemba.dev` or one under `joshuafields.uk`) would need a CNAME.

---

## 1. Spike results

### 1.1 Does it build?

`pemba-core`, `pemba-schema`, `pemba-store-memory` and `pemba-fixtures` (with
`serde-saphyr`) **compile for `wasm32-unknown-unknown` unchanged**. They have no threads,
no file access on the paths used, and no `getrandom`. `std` `HashMap` works on wasm32
(fixed hash keys), and `Mutex` works single-threaded.

**One runtime blocker.** `std::time::Instant::now()` is unimplemented on
`wasm32-unknown-unknown` and traps (`RuntimeError: unreachable`). `Engine::check`,
`lookup_*` and `check_with_trace` all call it (`engine.rs`, `trace/record.rs`), so every
query failed while schema compile and writes succeeded. `SystemTime::now()`
(`write.rs::now_iso8601`) would also trap, but only the SQL stores reach it.

### 1.2 The fix, and D2

The spike swaps the type with
`#[cfg(target_arch = "wasm32")] use web_time::Instant;` in those two files. After that,
everything worked. `web-time` 1.1.0 (MIT/Apache-2.0) re-exports `std::time` on native
targets and uses `performance.now()` on the web. It would be a
`[target.'cfg(target_arch = "wasm32")'.dependencies]` entry, so native builds and wheels
do not change.

The alternative is a private `Clock` that returns zero durations on wasm32. It adds no
dependency, but the trace's `duration_ns` and `stats.elapsed_ns` would read 0 in the
playground. Browser timers are coarse anyway: without cross-origin isolation, which
GitHub Pages cannot enable, `performance.now()` is rounded to about 0.1 ms. So the
playground hides per-node durations either way. I recommend `web-time` because it keeps
one code path and real (if coarse) totals.

### 1.3 Size

Release profile: `opt-level = "s"`, LTO, one codegen unit, `panic = "abort"`, symbols
stripped. Sizes are measured after `wasm-bindgen --target web`.

| Build | raw | gzip -9 | brotli |
|---|---|---|---|
| engine + fixture runner | 1 219 KB | 402 KB | 302 KB |
| engine + fixture runner, `wasm-opt -Oz` | 1 110 KB | 422 KB | 321 KB |
| engine only (no `pemba-fixtures`) | 432 KB | 151 KB | 121 KB |
| engine only, `wasm-opt -Oz` | 392 KB | 162 KB | 128 KB |

- **`wasm-opt` does not pay.** It cuts raw size by about 9% but makes compressed size
  about 5% bigger, and pages are served compressed. Skip it, and the Binaryen
  dependency with it. Re-measure if the build profile changes.
- **The fixture runner is two thirds of the binary.** Most of that is YAML parsing in
  `serde-saphyr`. Still, 402 KB gzip is acceptable for a tool page (CodeMirror adds
  about 150 KB), and running assertions through `pemba-fixtures` gives exactly
  `pemba test` semantics. So: ship one module, and set a CI size budget (§5). If size
  ever matters, split the fixture runner into a second module loaded on first use.
- The JS glue is 18 KB, and instantiation takes 12 ms.

### 1.4 Latency (google_docs fixture: 42 tuples, its 32 distinct checks)

Each figure is the mean per call, including the JSON-string boundary: 200 calls per
query in the browser, 2 000 natively.

| | WASM (Chromium) | native |
|---|---|---|
| `check` p50 / min / max over the 32 queries | 7.5 / 2.5 / 22 µs | 5.3 / 1.3 / 11.6 µs |
| `check_with_trace` | 55 µs | 32 µs |
| `lookup_resources` (`document#view@user:alice`) | 38 µs | 26 µs |
| whole fixture through `run_fixture` | 2 ms | — |

WASM is 1.4–1.9× native, far below anything a person can perceive. Speed is not a
concern for the playground. The engine still runs in a Web Worker (§3) so that a
pathological input cannot freeze the page.

---

## 2. Scope change: WASM for the playground only

Proposed new wording for the WASM item in `CLAUDE.md` §6:

> WASM, except the playground build (`playground/`, `docs/11-playground.md`): not
> published to npm or crates.io, no API or stability promise. A supported WASM package
> stays out of scope.

What this commits us to:

1. **`pemba-core`, `pemba-schema`, `pemba-store-memory` and `pemba-fixtures` keep
   compiling and running on `wasm32-unknown-unknown`.** A CI job enforces this (T6.1),
   so new code there cannot use `std::time::Instant`, threads or the file system without
   a `cfg` gate. SQLite, Postgres, the CLI and Python are unaffected.
2. **New dependencies.** `web-time` (D2) in core, wasm32 only. `wasm-bindgen` in the
   unpublished playground crate only. Each is a one-line addition to `09` §2.
3. **A JavaScript toolchain**: Node, Vite, TypeScript, CodeMirror, Playwright and
   Vitest, all pinned with a lockfile and updated by Dependabot. `cargo deny` does not
   cover npm, so CI also runs `npm audit --omit=dev` (production dependencies only).
4. **A public URL people will cite in bug reports.** Every page therefore shows the
   engine version and commit it was built from.
5. **ADR-016** records this decision (T6.0).

## 3. How it runs

```
main thread (TS + CodeMirror)  ──postMessage──▶  Web Worker
  editors, results, explain tree                    pemba_playground.wasm
  URL fragment state                                  Engine<MemoryStore>
```

- **Every run starts fresh.** The worker builds a new memory engine, then writes the
  schema, then the relationships (one `Touch` batch per line, as fixtures do), then runs
  the query. Editors are re-run 300 ms after the last keystroke. There is no
  incremental state, so the result never depends on edit history.
- **Fail closed in the UI.** Any engine error is shown with its code and message
  (`06` §1.1) in place of the answer. It is never shown as "denied".
- **Limits.** `EngineConfig::default()`, plus a cap of 10 000 relationship lines and 1 MB
  per editor. A worker that does not answer within 5 s is terminated, and the page says
  so.

### 3.1 The WASM API (`playground/wasm`, crate `pemba-playground`)

The crate exposes a small JSON-in/JSON-out surface over the library. It adds no
semantics of its own: `compile`, `run` (schema + relationships + one query of any kind),
`run_assertions` (an assembled fixture through `pemba_fixtures::run_str`), and
`parse_fixture` (for import). It is a workspace member with `publish = false`, so it
shares the workspace version and `Cargo.lock`.

Diagnostic spans are converted from byte offsets to **UTF-16 offsets** on the Rust side.
CodeMirror positions are UTF-16, and `SchemaError::col` counts `char`s, which disagree on
astral characters.

## 4. UX and frontend

- **Three editors.**
  1. **Schema**: a small CodeMirror `StreamLanguage` mode for the DSL, with diagnostics
     shown inline from `SchemaError` (squiggle, message, `help`, code).
  2. **Relationships**: one per line, `//` comments allowed, and a bad line marked with
     its parse error.
  3. **Assertions**: fixture YAML (`assertTrue`, `assertFalse`, `expectedLookups`,
     `expectCheckErrors`). Results appear per line (pass or fail, with the runner's
     message).
- **Query bar.** A mode picker (check / lookup resources / lookup subjects) and one
  input in the CLI syntax (`resource#perm@subject`, `type#perm@subject`,
  `type:id#perm@subject_type`). Results show the answer and revision. Lookups list the
  ids, and lookup_subjects shows the wildcard and its exclusions.
- **Explain.** For a check, a collapsible tree rendered from the trace JSON (`06` §3.1).
  It shows ✓/✗ per node, the matched tuple on direct hits, and memo hits as links to the
  node shown in full. Cycles show their pass count, and `truncated` is shown as a banner.
  A "copy as text" button gives the `pemba check --explain` rendering
  (`CheckTrace::render`).
- **Examples** (a menu): `google_docs`, `github_like` (both from `testdata/fixtures`) and
  `agent_authz`, which becomes a new fixture mirroring `examples/agent_authz.py`.
  Examples are fixtures, so `cargo test` keeps them correct.
- **Shareable links.** The state `{v:1, engine:"0.1.0-…", schema, relationships,
  assertions, query}` is turned into JSON, compressed with deflate-raw (the browser's
  built-in `CompressionStream`, no library), and stored base64url-encoded in
  `#s=<…>`. The fragment is never sent to any server. A link made with a different
  engine version opens with a notice. The whole google_docs fixture (7 KB) encodes to
  2.2 KB, well under common URL limits.
- **Import/export fixture YAML.** Import parses with `pemba-fixtures` (in WASM). Export
  writes `schema: |-`, `relationships: |-` and the assertions verbatim. A test round-trips
  every corpus fixture through export → import → run.
- **Stack.** TypeScript (strict), Vite, CodeMirror 6 (`codemirror`,
  `@codemirror/lang-yaml`, `@codemirror/lint`), Vitest for units and Playwright for
  end-to-end tests. There is no UI framework: the UI is three editors, a form and a tree,
  and plain DOM code is easier to keep working than a framework upgrade cycle. Styling
  is one hand-written CSS file, with light and dark themes from `prefers-color-scheme`.

## 5. Repository, CI and deployment

```
playground/
  wasm/        Rust crate pemba-playground (workspace member, publish = false)
  web/         package.json, vite.config.ts, src/*.ts, e2e/*.spec.ts
  conformance/ transcript runner (§6)
```

- **`ci.yml` gains a `wasm` job** on every PR:
  `cargo clippy --target wasm32-unknown-unknown -p pemba-core -p pemba-schema
  -p pemba-store-memory -p pemba-fixtures -p pemba-playground -- -D warnings`.
  This is commitment 1 in §2.
- **New workflow `playground.yml`**, run on PRs touching `crates/`, `playground/` or
  `testdata/`. It builds the release `.wasm` (wasm-bindgen CLI pinned to the crate's
  version), runs the §6 conformance check, runs `vite build`, enforces the **size budget
  of 700 KB gzip for the whole page**, and runs the Playwright tests. It uploads `dist/`
  as an artifact.
- **Deploy (D3).** `actions/deploy-pages`:
  - Each `v*` tag publishes to the site root.
  - Each push to `main` publishes to `/next/`.
  - The page footer shows the version and short commit, injected at build time from
    `cargo metadata` and `git rev-parse`.
  - Release checklist (`09` §4.1) gains one line: confirm the Pages deploy for the tag.
- **Keeping in sync.** The playground is built from the same commit as the engine, from
  the workspace, with no published package in between, so it cannot drift. What can drift
  is the deployed site against what users installed; D3 handles that.

## 6. Correctness: the WASM build must agree with the library

Golden rule 1 applies to the playground. A playground that answers differently from the
library is worse than none. CI proves agreement in three layers:

1. **Transcript equality, native vs WASM.**
   - `playground/conformance` takes a corpus and writes a transcript: for every fixture,
     every check, lookup, `expectCheckErrors` code, assertion report, and trace JSON with
     `duration_ns` and `elapsed_ns` removed. Lines are in a fixed order.
   - The same Rust function runs natively (`cargo run`) and inside the release `.wasm`
     (driven from Node through wasm-bindgen's `--target nodejs` glue over the identical
     `.wasm` bytes). CI diffs the two transcripts and fails on any difference.
   - The corpus is `testdata/fixtures/**` plus **1 000 cases from `pemba-testkit`'s
     generator** (cyclic data included), serialised as fixtures.
   - This catches target differences, such as 32-bit `usize` truncation and `HashMap`
     iteration order leaking into output.
2. **The library against the oracle.** The 1 000 generated cases are also run natively
   through the testkit fixture runner, so each one is checked against the reference
   evaluator as well. Together with layer 1, this makes WASM = native = oracle.
3. **The page against WASM.** Playwright loads the built site, opens each example, runs
   its assertions, and compares the page's displayed answers with the transcript.
   This catches UI bugs such as showing the wrong field or treating an error as denied.

---

## 7. Rejected alternative: a small hosted backend

A server (for example, the CLI behind an HTTP endpoint on a small VM) avoids WASM. It is
the wrong trade here:

- It costs money and needs operating.
- User schemas leave their machine, which matters for authorisation models.
- It needs rate limits and input limits against abuse, because evaluation cost is
  input-controlled.
- It adds latency.
- It brings server mode, which `CLAUDE.md` §6 also excludes.

The spike shows WASM costs one small `cfg` and runs fast enough. The backend's only
advantage would be no JavaScript toolchain, and the frontend needs that toolchain anyway.
