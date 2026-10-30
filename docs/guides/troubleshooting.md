# Troubleshooting, bounds and choosing an engine

An allowed check returns True/`Ok(true)`; a valid denial returns False/`Ok(false)`.
An invalid request, storage problem, undefined cycle or exhausted bound raises an
exception/returns `Err`. Never catch every exception and silently treat it as a normal
denial or grant. Keep a distinct operational error path and decide your application's
response explicitly. Invalid Python string arguments may raise `ValueError`; negative
or unrepresentable unsigned limits can raise `OverflowError` before engine validation.

| Symptom | What to check |
|---|---|
| Python import/build fails | Correct venv/interpreter, CPython >=3.11, matching wheel platform; source build needs Rust 1.99.0, C tools and Python headers |
| Current Rust example fails with an alpha dependency | Current docs use 0.1.0 constructors and `check_str`; use matching 0.1.0 dependencies or the alpha tag's own docs |
| SQLite `StorageError` on opening | Interpreter SQLite >=3.26.0 and `sqlite3.threadsafety > 0`; do not install a second SQLite build to bypass this |
| `ForeignConnectionMismatch` | Same file (not an in-memory SQLite connection), or same Postgres cluster/database; configured Postgres schema is fixed by the engine |
| Foreign-write `ValueError` / `TypeError` | Transaction already open, supported synchronous driver, correct connection type; raw async connections are deferred |
| `RevisionPending` / future revision | Commit first; engine reads cannot see uncommitted data; AtLeast does not wait indefinitely |
| `WriteRolledBack` | Discard the Postgres pending token; SQLite rollback cannot be detected from its known number |
| `REVISION_EXPIRED` | GC passed your revision; restart pagination from a fresh snapshot if appropriate |
| `NotMigrated` / `StorageVersionTooNew` | Run the matching migration as deploy role; do not downgrade metadata or mutate internal tables |
| TLS handshake failure | Platform CA trust and DSN sslmode; foreign driver's TLS is configured separately |
| Other commits stall / `PembaWarning` | Long transaction, early constraints or prepared 2PC transaction holding the revision lock |
| Transaction conflict | Roll back and retry the complete app transaction with bounded backoff; also handle driver COMMIT errors |
| Schema change rejected | Remove orphaning live relationships before changing the schema |
| Lookup omits/duplicates between pages | Use the first result's revision for every page; consider concurrent GC |

For unexpected permissions, use `pemba check --explain`, Python `check_with_trace`, or
Rust `Engine::check_with_trace`. CLI exit codes are 0 allowed, 1 denied, 2 error;
`--json` exposes the trace for tools. Use a fixture or the
[memory-only playground](https://joshua-fields.github.io/pemba/) to share a model,
without credentials or sensitive identifiers. The CLI does not offer a general
Postgres migration/repair command.

## Bounds

These defaults bound engine work. Rust configures them through `EngineConfig` setters;
Python exposes `max_depth` on engine construction, not every Rust limit.

| Limit | Default |
|---|---|
| Evaluation depth | 50 |
| Operations in one write | 1,000 |
| Rows per filtered delete | 10,000 |
| Lookup results per page | 1,000 |
| Lookup candidate nodes | 200,000 |
| Relationships read per check node | 100,000 |
| Store reads per check | 200,000 |
| Recorded trace nodes | 10,000 |

Evaluation limits fail closed (`EvaluationLimitError`); they do not mean denied.
Trace capacity is different: exceeding it truncates the explanation, leaving the
computed answer unchanged. Page limits paginate results; a lookup candidate/work limit
is an error, not silent partial success. Prefer bounded models and measured changes
to increasing every limit. Inspect [known differences](../../crates/pemba-difftest/KNOWN_DIFFERENCES.md)
for cyclic data, wildcard exclusions and unsupported DSL features.

## When a service is a better fit

pemba fits one application, or a few services sharing one database, that wants app rows
and permissions to commit together. Choose [SpiceDB](https://github.com/authzed/spicedb)
or another Zanzibar service when many languages/services need centralised permissions,
horizontal or multi-region scale, caveats/expiration/Watch, or a mature supported
product with service-level support commitments. pemba's 0.1.0 API contract carries
best-effort maintenance, without an SLA, guaranteed patch or long-term support.
No native async API, supported WASM package,
SpiceDB wire compatibility, distributed dispatch or server mode is promised here.
WASM is the browser playground build only. Review semantic differences before
migrating schemas; common syntax alone does not guarantee identical answers.

See [installation](installation.md), [schema](schema.md),
[consistency and retention](consistency.md), [transactions](transactions.md).
