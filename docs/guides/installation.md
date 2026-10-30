# Installation and quick starts

These guides describe the 0.1.0 API. Installation commands target 0.1.0 after
publication; for an unpublished release candidate, use the checkout recipes below.
Check [release status](https://github.com/Joshua-Fields/pemba/releases) and
[release evidence](../RELEASE-0.1.0.md) before assuming a registry artifact exists.
The [Rust quick start](../../README.md#rust-quick-start) uses the 0.1.0 constructors
and `Engine::check_str`; older alphas need their own versioned examples.
See [CHANGELOG](../../CHANGELOG.md) for migrations and [API documentation](../06-api.md).

## Requirements

| Component | Supported floor / build requirement |
|---|---|
| Python | CPython 3.11 or newer; abi3 extension and shipped `.pyi` / `py.typed` |
| SQLite | 3.26.0 or newer, with thread support; Python uses the interpreter's `sqlite3` |
| Rust source builds | Rust **1.99.0**, pinned in `rust-toolchain.toml`; rustfmt and Clippy for development |
| Postgres | Server 15 or newer, database encoding `UTF8`; CI covers 15 and 18 |
| Binary Python wheels | Linux x86_64/aarch64 (manylinux2014, glibc >=2.17), macOS x86_64/arm64, Windows x86_64 |

Wheel installation needs no Rust compiler and bundles no SQLite. On another platform,
pip may build an sdist: install the pinned Rust toolchain, a platform C compiler/linker
and Python development headers first. On Windows use MSVC Build Tools and its Windows
SDK; on Linux use the distribution's compiler/build tools and Python headers; on macOS
use the Command Line Tools. Rust's SQLite store builds its bundled SQLite; Python's
extension uses CPython's SQLite instead. Postgres support uses rustls, with platform
certificate roots, and needs no OpenSSL or libpq for engine-owned connections. Optional
application drivers have their own installation requirements.

## Registry installation

```sh
python -m venv .venv
# POSIX: . .venv/bin/activate
# PowerShell: .venv\Scripts\Activate.ps1
python -m pip install pemba==0.1.0
cargo add pemba@0.1.0
cargo add pemba-store-postgres@0.1.0
cargo install pemba-cli@0.1.0 --locked
pemba --help
```

Run Python and Rust commands in their respective projects. `pemba-cli` installs the
`pemba` executable into Cargo's bin directory; put that directory on PATH. The CLI
validates/formats schemas, runs fixtures and explains checks against SQLite or fixtures;
it is not a Postgres migration CLI. Explicit versions keep installation tied to the
documented API. For a prior alpha, select that exact version and its own documentation.

## Current-checkout Python quick start

Clone the repository, enter it, install Rust 1.99.0 through rustup, then:

```sh
python -m venv .venv
# Activate as above (Windows: .venv\Scripts\Activate.ps1).
python -m pip install 'maturin>=1.15,<2'
cd crates/pemba-py
maturin develop --release
```

Make a new empty directory outside the checkout, save the entire
[README Python example](../../README.md#python-quick-start) as `quickstart.py`, and run
it with that venv's Python. It creates `app.db`, including an application row and
permission in one transaction. Use a fresh directory on each run: the insert is
intentionally not an upsert. A source checkout verification is not a registry-install
or release-wheel verification; those have separate release gates.

## Current-checkout Rust quick start

In a new directory outside the repository, run `cargo new --bin pemba-quickstart`,
then replace its dependency section with this, substituting your checkout's absolute
path (use forward slashes on Windows):

```toml
[dependencies]
pemba = { path = "/absolute/path/to/pemba/crates/pemba", default-features = false, features = ["memory"] }
```

Paste the complete [README Rust example](../../README.md#rust-quick-start) into
`src/main.rs`, then run `cargo +1.99.0 run`. It asserts the permission and lookup result.
This explicitly selects `memory`; it needs no database and loses data on exit.
The facade defaults enable both `memory` and `sqlite`. For persistence select `sqlite`
and use `SqliteStore::open("app.db", SqliteConfig::default())`. Postgres is a separate
`pemba-store-postgres` dependency; use the same checkout/version as the facade.
`PostgresStore` and `PostgresConfig` come from that crate, not the facade.

To install the current-checkout CLI, run from the repository root:

```sh
cargo +1.99.0 install --path crates/pemba-cli --locked --root /absolute/path/to/cli-install
/absolute/path/to/cli-install/bin/pemba --help
```

On Windows the executable is `pemba.exe`. This isolated install root avoids replacing
an existing CLI. The [transaction guide](transactions.md) covers migrations, deployment
roles and foreign transaction driver versions. New Rust code should construct requests
with `new` and setters; do not copy pre-audit struct literals from older alpha examples.
Python alpha.3's `int(pending)` / index conversions were removed in alpha.4: use
`pending.revision()` after commit, or `at_least=pending`.

## Runnable verification

Run this with the installed Python from an empty directory. It checks the actual
interpreter and extension, not a stub or a second SQLite build:

```python
import sys
import sqlite3
from importlib.metadata import version
from pemba import Engine, MIN_SQLITE_VERSION

assert sys.version_info >= (3, 11)
assert sqlite3.sqlite_version_info >= (3, 26, 0)
assert sqlite3.threadsafety > 0
print("pemba", version("pemba"), "SQLite", sqlite3.sqlite_version, "floor", MIN_SQLITE_VERSION)
engine = Engine.memory()
engine.write_schema("definition user {} definition document { relation viewer: user permission view = viewer }")
rev = engine.write(touch=["document:readme#viewer@user:anne"])
assert engine.check("document:readme#view@user:anne", at_least=rev)
```

Next: [schema](schema.md), [consistency](consistency.md),
[transactions and Postgres](transactions.md), [troubleshooting and limits](troubleshooting.md).
