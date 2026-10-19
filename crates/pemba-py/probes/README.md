# OQ-032 probes (T3.2 phase 0)

Throwaway investigation code, not workspace members, never shipped. See
`docs/OPEN_QUESTIONS.md` OQ-032.

- `sqlite_copies/`: holds a write lock with a bundled SQLite copy (built like `pemba-py`)
  and shows that a `close()` by CPython's `sqlite3` drops it. `maturin develop && python
  run_probe.py`. Exit status 1 = lock lost. CI: `.github/workflows/oq032-probe.yml`.
- `pystore/` (removed in T3.2a): the option A prototype, superseded by
  `crates/pemba-store-pysqlite`. Its benchmark lives on as `crates/pemba-py/bench/`.
  The probe itself is now the regression test `tests/test_one_sqlite.py`.
