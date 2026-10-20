# Benchmarks

Scenarios: `docs/07-testing.md` §7. Datasets: `src/lib.rs`, the single definition shared
by all three harnesses. Design choices: `docs/OPEN_QUESTIONS.md` OQ-038.

| What | Command (from the repo root) | Output |
|---|---|---|
| Rust, memory and SQLite stores | `cargo bench -p pemba-benches` | `target/criterion/`, `target/bench-latency/` |
| Python, memory and SQLite engines | `cd crates/pemba-py && maturin develop --release && pytest bench/test_scenarios.py --benchmark-json=../../target/bench-py.json --benchmark-save-data` | `target/bench-py.json` |
| SpiceDB (needs Docker) | `python benches/spicedb/run.py` | `target/bench-spicedb.json` |
| Results table | `python benches/results.py` | `benches/RESULTS.md`, `benches/RESULTS.json` |

The Python and SpiceDB benches generate the datasets into `target/bench-data/` on first use
(`cargo run -p pemba-benches --bin bench-data -- target/bench-data`).

A quick check that every bench builds and gets the right answers, without timing anything:
`cargo test -p pemba-benches --benches`.

Never publish a number without the script and machine that produced it: `RESULTS.md`
records both.
