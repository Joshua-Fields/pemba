# Spike: Postgres revision ordering (OQ-002)

Throwaway evidence for `docs/10-postgres.md` §1. Not production code, not a workspace
member, never merged. Branch `spike-pg-revisions`.

## Run

```bash
docker run -d --name pemba-spike-pg -e POSTGRES_PASSWORD=spike -e POSTGRES_DB=spike \
    -p 55432:5432 postgres:17 -c max_connections=200
python -m venv .venv && .venv/bin/pip install "psycopg[binary]"   # .venv\Scripts on Windows
python spike.py --extras                                   # all schemes, RC, long txns
python spike.py --isolation SERIALIZABLE --schemes counter,watermark,committid
python spike.py --writers 16 --hold-ms 0 --long-every 0    # throughput, no holds
```
`SPIKE_DSN` overrides the connection string.

## Schemes

| name | revision assigned | ordering mechanism |
|---|---|---|
| `naive` | `nextval` at write | none (negative control) |
| `counter` | `nextval` at write | store-wide advisory xact lock held from write to commit (option a) |
| `watermark` | `nextval` at write | per-revision advisory xact lock; head = below oldest in-flight (option b) |
| `committime` | at COMMIT | deferred constraint trigger takes a store-wide lock, `nextval`, rewrites rows by predicate |
| `committid` | at COMMIT | as `committime`, but rewrites the writer's recorded ctids, predicate fallback (round-2 recommendation; hypothesis A alone) |
| `batchmap` | at COMMIT | hypothesis A+B: rows carry batch ids, no rewrite; the trigger only inserts `batch_id -> rev` (round-3 recommendation) |

## What the harness checks

8 writers + 4 readers (defaults). Writers hold half their transactions open 0-30 ms after
`write()` returns; with `--long-every 4 --long-s 3` one more writer holds a transaction
open for 3 s every 4 s (a long foreign transaction). Readers repeatedly resolve `Latest`
and fingerprint the rows visible at it inside one REPEATABLE READ snapshot.

- **violations**: observations whose fingerprint at head `N` differs from the final
  state's fingerprint at `N` (a write <= N became visible after N was observed).
- **head_regressions**: a request that started after another finished saw a lower head.
- **write/rev vs commit order inversions**: pairs where one transaction finished
  committing before the other started committing, yet was written later / got the
  larger revision. Proves the interleaving was exercised.
- **visibility_lag_ms**: commit returned -> first request started afterwards sees it.
- **writer_blocked_ms**: write + commit time minus the deliberate hold.
- `--extras`: a deterministic forced inversion (A writes, B writes and commits while A is
  open, reader observes, A commits) and savepoint/rollback/same-transaction checks.

## Review checks (second round, `docs/10-postgres.md` §1.5.1)

```bash
python review_checks.py      # SET CONSTRAINTS ALL IMMEDIATE, skipped revisions, 1,000-tuple batches
python spike.py --schemes committid --immediate long --duration 20   # IMMEDIATE on the long txn
python spike.py --schemes committid --immediate all --long-every 0   # IMMEDIATE everywhere
python spike.py --schemes committid,watermark --isolation SERIALIZABLE --retry 10 --backoff
python ssi_probe.py          # diagnostic: which trigger step causes sticky 40001s
```
`--retry N` retries a transaction on 40001 up to N times (`--backoff`: 10 ms * 2^n with
full jitter, cap 1 s) and reports transactions needing a retry and giving up.
`ssi_probe.py` variants other than `full` are deliberately wrong and exist only to isolate
the cause.

Results: `results-*.json`, `runs*.log`, `review_checks.log`, `ssi_probe.log`; summary in
`docs/10-postgres.md` §1.5 and §1.5.1.
Environment: Postgres 17.11 in Docker Desktop on Windows 11, Python 3.12, psycopg 3.3.6.
Absolute numbers are machine-specific; compare schemes against each other.

## Round 3 (SERIALIZABLE give-ups, `docs/10-postgres.md` §1.5.2)

- `spike.py` now resolves a commit-time revision on a separate autocommit connection, as
  the design does; `--resolve-on-writer` restores the rounds 1-2 behaviour (a new
  SERIALIZABLE transaction on the writer's connection), which inflated 40001 rates.
- `batchmap`: rows store `created_batch` / `deleted_batch`; reads at head use
  `deleted_batch IS NULL` (no join), reads at an older revision join the batch map.
- `bench_reads.py`: read-path cost, revision columns vs batch map, emulating the store
  reads of each `docs/07-testing.md` §7 bench shape (the Rust benches cannot run on
  Postgres yet).
- `detect_immediate.py`: `current_query()` inside the trigger tells COMMIT from
  SET CONSTRAINTS ... IMMEDIATE.
- `summarize.py`: one line per scheme for results files.
- Results: `r3-*.json`, `r3-*.log`, `runs7-9.log`.
