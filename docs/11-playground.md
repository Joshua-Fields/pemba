# 11 — Playground (design)

Status: **accepted** (T6.0; D1–D4 resolved by the maintainer, 2026-10-28). Nothing here is
built yet. ADR-016 and the `CLAUDE.md` §6 amendment (§2) are in place.

Evidence: a throwaway spike on the unmerged branch `t6.0-playground-spike`,
`spike/playground-wasm/` (wasm-bindgen 0.2.129, rustc 1.99.0, Chromium from Playwright
1.64.1, Windows 11). Its README has the commands and raw numbers.

---

## 0. Summary and decisions

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

Decisions, resolved by the maintainer on 2026-10-28:

- **D1. Accepted.** WASM in the browser, allowed for the playground build only. It is not
  a supported, published WASM package. `CLAUDE.md` §6 carries the §2 wording, and
  ADR-016 records the decision.
- **D2. `web-time`, as a wasm32-only dependency of `pemba-core`** (§1.2). The
  zero-duration clock was rejected.
- **D3. Release tags deploy to the site root, and `main` deploys to `/next/`.** In
  addition, every shareable link carries the pemba version, and a link whose version
  differs from the deployed one shows a banner. Each release stays reachable at
  `/v/<version>/`, so an old link reproduces exactly (§4, §5.1).
- **D4. `joshua-fields.github.io/pemba/` for now.** A custom domain may come later. It
  would need only a CNAME and the Vite `base` path, so no task depends on it.

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
- **Page-side error codes** (T6.4). These are shown like engine errors, never as an
  answer:
  - `INPUT_TOO_LARGE`: a limit above;
  - `TIMEOUT`: the 5 s limit;
  - `ENGINE_CRASHED`: the WASM trapped or failed to load. The worker is replaced,
    because a trapped instance is not reused;
  - `INTERNAL`: an answer that is not exactly the expected shape.

### 3.1 The WASM API (`playground/wasm`, crate `pemba-playground`)

The crate exposes a small JSON-in/JSON-out surface over the library. It adds no
semantics of its own: `compile`, `run` (schema + relationships + one query of any kind),
`run_assertions` (an assembled fixture through `pemba_fixtures::run_str`), and
`parse_fixture` (for import). It is a workspace member with `publish = false`, so it
shares the workspace version and `Cargo.lock`.

Diagnostic spans are converted from byte offsets to **UTF-16 offsets** on the Rust side.
CodeMirror positions are UTF-16, and `SchemaError::col` counts `char`s, which disagree on
astral characters.

Contract details (T6.2; the crate docs are the reference):
- **Envelope.** Every answer has `ok`. On `ok: false`, `error` has `code`, `message` and
  `stage` (`request`, `schema`, `relationships` or `query`). Where they apply, it also
  has `diagnostics` (schema), and `line`, `start`, `end` and `at` (UTF-16, into that
  stage's text).
- **Codes.** An engine error carries `Error::code()`. A relationship the schema rejects
  carries its `E_...` code, as fixtures name it. `PARSE_ERROR` means input that does not
  parse (not an engine code). `BAD_REQUEST` means request JSON this API cannot read.
- **Writes.** `run` writes relationships as `Touch` batches of `max_write_batch` lines,
  as `pemba check --fixture` does. `run_assertions` hands one fixture to
  `pemba_fixtures::run_str`, so it keeps `pemba test`'s single batch: more than 1 000
  relationships fail there with `BATCH_TOO_LARGE`, as they do in `pemba test`.
- **Trace.** `run` returns a check's `trace` as the `pemba check --json` text, byte for
  byte. It is not an embedded object, because a deep trace (the `depth_limit` fixture)
  nests past `serde_json`'s recursion limit. The worker calls `JSON.parse` on it.
- **Lookups.** `lookup_resources` returns every page, all read at the first page's
  revision.
- **Assertion report** (T6.6). `run_assertions` also returns `assertions_line`, the
  1-based line of the joined fixture where the assertions text starts. The runner names
  YAML errors (an unknown key) by their line in the joined fixture; the page subtracts
  this to mark the assertions editor's line.

## 4. UX and frontend

- **Three editors.**
  1. **Schema**: a small CodeMirror `StreamLanguage` mode for the DSL, with diagnostics
     shown inline from `SchemaError` (squiggle, message, `help`, code).
  2. **Relationships**: one per line, `//` comments allowed, and a bad line marked with
     its parse error.
  3. **Assertions**: fixture YAML (`assertTrue`, `assertFalse`, `expectedLookups`,
     `expectCheckErrors`). Results appear per line (pass or fail, with the runner's
     message).
     - As built (T6.6): the runner reports only failures, as location and assertion
       text. The page finds the line each one names and marks it ✗, inline and in a
       gutter. A line is marked ✓ only when the page can prove the runner checked it:
       every failure was placed on a line, nothing stopped the fixture (a `fixture`,
       `schema`, `relationships` or `engine` failure), and the lines it counts as checked
       number exactly the runner's `checked`. Otherwise no line shows ✓, and the summary
       still gives the runner's verdict and every failure.
     - A step whose write failed, or succeeded against an `expectError`, has its
       assertions marked "not checked" (–), as the runner skips them.
     - A YAML error's first message line is shown with its position counted in the
       editor's lines (the rest of the message quotes the joined fixture and is
       dropped).
- **Query bar.** A mode picker (check / lookup resources / lookup subjects) and one
  input in the CLI syntax (`resource#perm@subject`, `type#perm@subject`,
  `type:id#perm@subject_type`). Results show the answer and revision. Lookups list the
  ids, and lookup_subjects shows the wildcard and its exclusions.
- **Explain.** For a check, a collapsible tree rendered from the trace JSON (`06` §3.1).
  It shows ✓/✗ per node, the matched tuple on direct hits, and memo hits as links to the
  node shown in full. Cycles show their pass count, and `truncated` is shown as a banner.
  A "copy as text" button gives the `pemba check --explain` rendering
  (`CheckTrace::render`).
  - As built (T6.7): every check asks `run` for its trace. Each node's line is the text
    `render()` writes for it (`src/trace.ts` follows `trace/render.rs`), including the
    granting path and `(not needed)`; Vitest holds the tree to the four T4.1 snapshots
    node for node (`src/trace-snapshots.json`, kept current by the `pemba-playground`
    test `trace_snapshots`). The granting path starts open and other branches closed; a
    "see above / see below" reference opens and highlights the node it names. A trace
    that is not exactly the answer's shape is an `INTERNAL` error, never a tree.
- **Examples** (a menu): `google_docs`, `github_like` (both from `testdata/fixtures`) and
  `agent_authz`, which becomes a new fixture mirroring `examples/agent_authz.py`.
  Examples are fixtures, so `cargo test` keeps them correct.
- **Shareable links.** The state `{v:1, engine:"0.1.0-…", schema, relationships,
  assertions, query}` is turned into JSON, compressed with deflate-raw (the browser's
  built-in `CompressionStream`, no library), and stored base64url-encoded in
  `#s=<…>`. The fragment is never sent to any server. The whole google_docs fixture
  (7 KB) encodes to 2.2 KB, well under common URL limits.
  - **Every link carries the pemba version** (`engine`; on `/next/` the version plus the
    short commit), so a link always names the engine that produced its answers.
  - **A version mismatch shows a banner.** When the link's version differs from the
    page's, the page still loads the state, and a banner names both versions. If
    `/v/<link version>/` exists, the banner links there with the same fragment, so the
    answers can be reproduced exactly. If it does not exist (a `/next/` build, or a
    version from before the playground), the banner says the answers may differ. The
    page never switches versions on its own.
  - As built (T6.8): the state is `{v: 1, engine, schema, relationships, assertions,
    query: {kind, text}}`, all keys required, no others. A link that does not decode,
    inflates past 4 MB, or lacks or mistypes any part shows an error and loads nothing.
    `engine` is the workspace version, injected by `vite.config.ts`; on a build whose
    `base` ends in `/next/` it is `<version>+<short commit>`. Whether `/v/<version>/`
    exists is a same-origin `HEAD` request to it (Pages answers 404 for a missing path;
    the Vite server is set to `appType: 'mpa'` so it does too); an engine with `+` is
    never probed. Loading an example or importing a file drops the fragment and the
    banner, since the page no longer shows the link's state.
- **Import/export fixture YAML.** Import parses with `pemba-fixtures` (in WASM). Export
  writes `schema: |-`, `relationships: |-` and the assertions verbatim. A test round-trips
  every corpus fixture through export → import → run.
  - As built (T6.8): the header is `|-` unless the text needs otherwise to come back
    exactly: `|` or `|+` for one or more trailing newlines, and an indentation indicator
    (`|2-`) when the first non-blank line starts with a space. No blank line precedes the
    assertions, because import gives a column-0 blank line to the key after it. Vitest
    (`src/fixture.test.ts`, against the built `.wasm`) round-trips every corpus fixture:
    the editors come back identical, the run passes with the native runner's count, and
    exporting again gives the same file. Playwright does one round trip through the
    page's buttons.
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
- **Deploy (D3).** See §5.1. The page footer shows the version and short commit,
  injected at build time from `cargo metadata` and `git rev-parse`. The release
  checklist (`09` §4.1) gains one line: confirm the Pages deploy for the tag, and confirm
  that `/v/<version>/` loads.
- **Keeping in sync.** The playground is built from the same commit as the engine, from
  the workspace, with no published package in between, so it cannot drift. What can drift
  is the deployed site against what users installed, and old links against the current
  engine. §5.1 handles both.

### 5.1 Site layout and versioned releases

```
joshua-fields.github.io/pemba/                 latest release (same build as its /v/ copy)
joshua-fields.github.io/pemba/v/<version>/     every release since the first playground release
joshua-fields.github.io/pemba/next/            the latest commit on main
```

- **Release assets are the archive.** GitHub Pages replaces the whole site on every
  deploy, so old releases must be re-published each time. On each `v*` tag,
  `playground.yml` builds the site with Vite `base` set to `/pemba/v/<version>/`. It
  creates that version's GitHub Release, or updates it if it exists. The release notes
  are the version's `CHANGELOG.md` section, and the release is marked pre-release. The
  workflow then attaches the site as `playground-<version>.tar.gz`. A release's asset
  is never rebuilt.
- **Every deploy assembles the whole site.** This runs on a tag, or on a push to
  `main`. The job downloads every `playground-*.tar.gz` release asset into
  `/v/<version>/`. It copies the newest release's build to the root, rebuilt with `base`
  `/pemba/` from the same tag. It then builds `main` into `/next/` and runs
  `actions/deploy-pages`. Because the old builds come from their assets, an old link at
  `/v/<version>/` runs the very `.wasm` that version shipped.
- **Prereleases count as releases** (all current releases are alphas). The root follows
  the newest one until 0.1.0 exists, and after that the newest stable release.
- **Size.** About 0.7 MB per version, so a hundred releases still fit well within the
  Pages limit of 1 GB per site. Pruning can wait until it matters.

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

As built (T6.3), `playground/conformance/run.sh` runs layers 1 and 2. CI runs it as
`playground.yml`.
- **Generation.** `pemba-playground-conformance generate` draws, from a fixed seed, two
  in five cases from each `SubSides` mode and one in five from `cyclic_case`.
  - Each case becomes a fixture. Every `assertTrue`, `assertFalse` and `expectedLookups`
    expectation in it is the reference evaluator's answer.
  - A check the engine refuses with an accepted error becomes an `expectCheckErrors`
    entry. The accepted errors are an ambiguous wildcard exclusion where the oracle shows
    it applies (`07` §2.3), or a default limit.
  - Lookups are left out of the fixture in three cases: they name the oracle's
    `__fresh__` stand-in (which it leaves out of lookup answers), or the engine refuses
    them for ambiguity or a limit. Properties 2 and 3 cover when those refusals are
    right.
  - Any other engine error stops the run.
  - The seed run has 12 499 checks and 15 678 lookups in 1 000 cases.
- **Transcript.** `pemba_playground::transcript(name, fixture)` is exported to WASM, so
  one Rust function writes both sides. It records the API's answer to `parse_fixture`,
  `compile`, `run_assertions`, and every top-level check (with and without explain),
  `expectCheckErrors` entry and lookup through `run`, with trace timings zeroed.
- **The `.wasm` is built with the `wasm-release` profile**, the one the site ships.
- **Negative control (local only).** Building with
  `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="--cfg pemba_conformance_sabotage"`
  makes the `.wasm` drop one id from multi-id `lookup_resources` answers, and `run.sh`
  then fails.
3. **The page against WASM.** Playwright loads the built site, opens each example, runs
   its assertions, and compares the page's displayed answers with the transcript.
   This catches UI bugs such as showing the wrong field or treating an error as denied.
   As built (T6.6), `e2e/assertions.spec.ts` loads every `testdata/fixtures` file into
   the three editors (`e2e/corpus.json`, kept current by the `pemba-playground` test
   `assertion_corpus`) and requires the native runner's check count, all passed, with
   that many lines marked ✓.

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
