# OQ-032 probes (T3.2 phase 0)

Throwaway investigation code, not workspace members, never shipped. See
`docs/OPEN_QUESTIONS.md` OQ-032.

- `sqlite_copies/`: holds a write lock with a bundled SQLite copy (built like `warden-py`)
  and shows that a `close()` by CPython's `sqlite3` drops it. `maturin develop && python
  run_probe.py`. Exit status 1 = lock lost. CI: `.github/workflows/oq032-probe.yml`.
- `pystore/`: option A prototype, a store that runs every statement on Python `sqlite3`
  connections. `maturin develop --release`, then `python conformance.py` (store
  conformance suite) and `python bench.py [rounds]` (needs `warden` built with
  `--release` and `pyyaml`).
