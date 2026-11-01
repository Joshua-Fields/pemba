# Roadmap

pemba is an embedded, Zanzibar-style authorisation engine for Rust and Python. SpiceDB and
OpenFGA run as services with their own data, so every permission change is a second write
to a second system. pemba is a library that keeps relationships in your own SQLite or
Postgres database, so the row you insert and the permission that grants access to it commit
together. If you have one application and one database, that is the reason to pick it.
[Compare it with SpiceDB](README.md#pemba-vs-spicedb-when-to-use-which) or
[try it in the browser](https://joshua-fields.github.io/pemba/).

## Current release: 0.2.0

| Release | What it added |
|---|---|
| 0.2.0 | Async Python: `write_async` for psycopg, asyncpg and SQLAlchemy async transactions, and `pemba.aio` read helpers |
| 0.1.0 | First stable API: audited Rust constructors, `check_str`, user guides and the documentation site |
| 0.1.0-alpha.4 | Browser playground running the real engine |
| 0.1.0-alpha.3 | Postgres store, including writes inside your own Postgres transaction |
| 0.1.0-alpha.2 | Explain traces (`pemba check --explain`) and engine configuration |
| 0.1.0-alpha.1 | The engine, schema language, `check` and lookups, memory and SQLite stores, Python bindings and writes inside your own SQLite transaction |

Migration notes are in the [changelog](CHANGELOG.md), and packages and release notes are on
the [releases page](https://github.com/Joshua-Fields/pemba/releases).

## What's next

This is the intended order, with no promised dates.

**0.3.0: faster deep hierarchies on Postgres, and a FastAPI helper**

- **Fewer Postgres round trips.** Done on `main`, not yet released. A check on a document 30
  folders deep goes from 99 round trips to 33 and from 13.3 ms to 6.7 ms (p50), and a lookup
  over 10,000 documents from 506 to 8 and from 72 ms to 16 ms
  ([measurements](benches/RESULTS.md#postgres-rust-linux)). Shallow checks gain less, and
  some are slower at p99.
- **FastAPI helper.** Done on `main`, not yet released. `pip install pemba[fastapi]` adds a
  permission check for your routes: a configurable 403 on deny, and errors that stay errors
  rather than turning into a deny ([guide](docs/guides/fastapi.md)).

**0.4.0: authorised queries**

Filter your application's queries by permission inside the database, instead of filtering
rows in application code. SQLite comes first. Results must agree with `lookup_resources`,
and pagination and ordering must stay correct at a fixed revision.

**After that**

Priorities will follow [issues](https://github.com/Joshua-Fields/pemba/issues) and feedback
from real use.

**Not planned**

Caveats/CEL, expiring relationships, Watch/change feeds, a server mode, distributed
dispatch and SpiceDB wire-API compatibility. If you need these,
[SpiceDB is the better fit](README.md#pemba-vs-spicedb-when-to-use-which). SpiceDB
import/export tooling is not scheduled.

## Known limits

- **SpiceDB compatibility.** The schema language is a strict subset, but some answers
  differ, for example on cyclic data. See
  [known differences](crates/pemba-difftest/KNOWN_DIFFERENCES.md).
- **Bounds.** Evaluation has configurable limits. Hitting one is an error, never a deny or
  an allow. See [bounds and troubleshooting](docs/guides/troubleshooting.md).
- **Transactions.** You own commit and rollback, and engine reads never see uncommitted
  data. See [transactions and Postgres operations](docs/guides/transactions.md).
- **History.** Reads at an old revision apply today's schema and stop working once GC passes
  that revision, so they are not an audit log. See
  [consistency and history](docs/guides/consistency.md).
- **Postgres schema renames.** Do not rename a schema that holds an active store: there is
  no repair command yet. See [Postgres setup](docs/guides/transactions.md#postgres-setup-and-migrations).
- **Playground.** It uses an in-memory store, so it cannot show a real commit or rollback.
  There is no supported WASM package.
- **Support.** Maintenance is best effort, with no response SLA.

Report bugs and request features through
[issues](https://github.com/Joshua-Fields/pemba/issues). Report suspected vulnerabilities
privately, as described in [SECURITY.md](SECURITY.md).
