# T6.0 playground spike (unmerged)

Proves the engine runs in a browser. Not production code; see `docs/11-playground.md`.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
npm i && npx playwright install chromium
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir web/pkg target/wasm32-unknown-unknown/release/pemba_playground_spike.wasm
node web/run.mjs                              # headless Chromium: fixtures + timings
cargo run --release --example native         # native baseline
```

## Results (2026-10-28, Windows 11, Chromium from Playwright 1.64.1, rustc 1.99.0)

Blocker found: `std::time::Instant::now()` traps (`unreachable`) on wasm32-unknown-unknown,
so every `check`/`lookup_*` failed while schema compile and writes worked. Fixed on this
branch with `web-time` under `cfg(target_arch = "wasm32")` in `pemba-core` (engine.rs,
trace/record.rs). Nothing else needed changing: no threads, fs, getrandom or
`SystemTime` on the memory-store path (`now_iso8601` is SQL-store only).

| .wasm (opt-level s, LTO, panic=abort) | raw | gzip -9 | brotli 11 |
|---|---|---|---|
| engine + fixture runner, after wasm-bindgen | 1 219 KB | 402 KB | 302 KB |
| same, after `wasm-opt -Oz` | 1 110 KB | 422 KB | 321 KB |
| engine only (no pemba-fixtures), after wasm-bindgen | 432 KB | 151 KB | 121 KB |
| engine only, after `wasm-opt -Oz` | 392 KB | 162 KB | 128 KB |

JS glue: 18 KB. Instantiate: 12 ms.

Correctness: all 18 fixtures in `testdata/fixtures` (273 assertions) pass through
`run_fixture` in the browser.

Latency, google_docs fixture (42 tuples), 32 distinct checks, mean of 200 (browser) /
2000 (native) calls each, including the JSON-string boundary:

| | WASM (Chromium) | native |
|---|---|---|
| check p50 / min / max | 7.5 / 2.5 / 22 µs | 5.3 / 1.3 / 11.6 µs |
| check_with_trace | 55 µs | 32 µs |
| lookup_resources (document#view, user:alice) | 38 µs | 26 µs |
| whole google_docs fixture | 2 ms | |
