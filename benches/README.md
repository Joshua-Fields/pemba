# Benchmarks

Scenarios: `docs/07-testing.md` §7. Datasets: `src/lib.rs`, the single definition shared
by all three harnesses. Design choices: `docs/OPEN_QUESTIONS.md` OQ-038.

| What | Command (from the repo root) | Output |
|---|---|---|
| Rust, memory and SQLite stores | `cargo bench -p pemba-benches` | `target/criterion/`, `target/bench-latency/` |
| Rust, Postgres too (T5.12) | the same, with `PEMBA_PG_URL` set (a server it may create schemas on) | as above, plus `target/bench-postgres-version.txt` |
| Python, memory and SQLite engines | `cd crates/pemba-py && maturin develop --release && pytest bench/test_scenarios.py --benchmark-json=../../target/bench-py.json --benchmark-save-data` | `target/bench-py.json` |
| SpiceDB (needs Docker) | `python benches/spicedb/run.py` | `target/bench-spicedb.json` |
| Results table | `python benches/results.py` | `benches/RESULTS.md`, `benches/RESULTS.json` |

The Python and SpiceDB benches generate the datasets into `target/bench-data/` on first use
(`cargo run -p pemba-benches --bin bench-data -- target/bench-data`).

A quick check that every bench builds and gets the right answers, without timing anything:
`cargo test -p pemba-benches --benches`.

On Postgres every scenario runs in a fresh schema, and each check and lookup also runs as
`<scenario>_below_head`: the same read at `Exact(head - 1)`, through the batch map
(`docs/10-postgres.md` §1.6.2, F12). The 1,000-tuple commit cost is reported by
`cargo test -p pemba-store-postgres --test revision_order -- --nocapture`.

Never publish a number without the script and machine that produced it: `RESULTS.md`
records both.
