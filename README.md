# warden

> **Codename.** The final project name is not chosen yet (`docs/01-decisions.md` ADR-012).

warden is an **embedded, Zanzibar-style authorisation engine** written in Rust, with Python
bindings. It runs inside your application process and stores relationship tuples in a
database your application already has (SQLite first, Postgres later), so relationship
writes can join your application's own database transaction.

**Status:** pre-alpha. Nothing works yet; see `docs/08-milestones.md` for the plan.

## Python and SQLite

`Engine.sqlite()` stores relationships through **CPython's own `sqlite3` module**. The
engine shares your interpreter's SQLite library (3.26.0 or newer) and bundles none. In the
same process, open the database with `sqlite3` or anything built on it (SQLAlchemy,
Django, `aiosqlite`). Do **not** open it with anything that brings its own SQLite, such as
`apsw` or `pysqlite3-binary`. On Linux and macOS, two SQLite libraries in one process
release each other's file locks, and a committed transaction can be silently lost. See
[`docs/06-api.md`](docs/06-api.md) §2.5 and
[How To Corrupt An SQLite Database File](https://www.sqlite.org/howtocorrupt.html) §2.2–2.3.

## Documentation

The technical specification lives in [`docs/`](docs/). Start with
[`docs/00-overview.md`](docs/00-overview.md).

## Acknowledgements

warden's design draws on:

- [Zanzibar: Google's Consistent, Global Authorization System](https://research.google/pubs/zanzibar-googles-consistent-global-authorization-system/)
  (Pang et al., USENIX ATC 2019), for the relationship-based authorisation model.
- [SpiceDB](https://github.com/authzed/spicedb) by AuthZed, whose schema language warden
  accepts a strict subset of. SpiceDB is used as a design reference only: no SpiceDB code is
  copied or translated into this project.

## Licence

Licensed under the [Apache License, Version 2.0](LICENSE).
