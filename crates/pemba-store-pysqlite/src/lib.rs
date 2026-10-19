//! SQLite relationship store for warden that runs every statement through CPython's own
//! `sqlite3` module (ADR-006 as amended by OQ-032).
//!
//! # Why
//!
//! On POSIX, SQLite keeps a process-wide list of open database files to work around
//! `close()` dropping every `fcntl` lock the process holds on a file. A second SQLite
//! library in the process has a second list, so a `close()` by either one silently drops
//! the other's locks. With a bundled SQLite in the extension and the app's `sqlite3` on the
//! same file, a committed transaction was lost in the T3.2 probe
//! (<https://www.sqlite.org/howtocorrupt.html> §2.2–2.3). This store links no SQLite: its
//! connections are `sqlite3.Connection` objects, so the engine and the app share one copy.
//!
//! # Shape
//!
//! Same as `warden-store-sqlite` (`docs/05-storage.md` §4.2): one writer connection, a
//! pool of read-only connections, one read transaction per request
//! ([`ReadStore::snapshot`]) and the shared write algorithm in [`warden_core::write`] over
//! [`PyTxn`]. Statement text, migrations and row decoding come from
//! [`warden_sqlite_common`].
//!
//! [`ReadStore::snapshot`]: warden_core::store::ReadStore::snapshot
//!
//! # The GIL
//!
//! Every statement takes the GIL ([`Python::attach`]) for its duration; between
//! statements the store holds none. The store itself never waits for a lock while holding the GIL:
//! the reader pool never blocks (it opens a connection when none is idle) and the writer
//! mutex is taken before any statement runs. A caller that holds the GIL across a whole
//! request (as `warden-py` does, for speed) must therefore serialise its requests on the
//! store: a thread blocked on the writer mutex while holding the GIL would stop the
//! holder, whose `sqlite3` calls release and retake the GIL, from ever finishing.
//!
//! [`Python::attach`]: pyo3::Python::attach

mod store;
mod txn;

pub use store::{PySqliteConfig, PySqliteStore, Synchronous};
pub use txn::PyTxn;
