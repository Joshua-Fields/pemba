//! A rusqlite connection that Python can hold a write lock on, so `run_probe.py` can
//! test what a `close()` by CPython's own SQLite copy does to that lock (OQ-032).

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use rusqlite::Connection;

fn err(e: rusqlite::Error) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// One connection of the bundled SQLite copy, in WAL mode.
#[pyclass(unsendable)]
struct Writer {
    conn: Connection,
}

#[pymethods]
impl Writer {
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let conn = Connection::open(path).map_err(err)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(err)?;
        if mode != "wal" {
            return Err(PyRuntimeError::new_err(format!("journal_mode is {mode}")));
        }
        conn.execute_batch("PRAGMA busy_timeout = 0").map_err(err)?;
        Ok(Self { conn })
    }

    /// Runs SQL that returns no rows.
    fn exec(&self, sql: &str) -> PyResult<()> {
        self.conn.execute_batch(sql).map_err(err)
    }

    /// First column of the first row, as text.
    fn scalar(&self, sql: &str) -> PyResult<String> {
        self.conn
            .query_row(sql, [], |r| {
                Ok(match r.get_ref(0)? {
                    rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                    rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    other => format!("{other:?}"),
                })
            })
            .map_err(err)
    }

    /// Whether this connection is inside a transaction.
    fn in_transaction(&self) -> bool {
        !self.conn.is_autocommit()
    }
}

/// The bundled SQLite's version string.
#[pyfunction]
fn sqlite_version() -> &'static str {
    rusqlite::version()
}

#[pymodule]
fn sqlite_copies_probe(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Writer>()?;
    m.add_function(wrap_pyfunction!(sqlite_version, m)?)?;
    Ok(())
}
