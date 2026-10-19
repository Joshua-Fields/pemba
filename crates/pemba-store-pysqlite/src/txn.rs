//! [`WriteTxn`] over a Python `sqlite3.Connection`.

use pyo3::prelude::*;
use pyo3::types::{PyInt, PyString, PyTuple};
use pemba_core::store::{SqlValue, StoreError, WriteTxn};

/// `SQLITE_CONSTRAINT_UNIQUE`, an extended result code.
const CONSTRAINT_UNIQUE: i64 = 2067;
/// `SQLITE_BUSY_SNAPSHOT`, an extended result code.
const BUSY_SNAPSHOT: i64 = 517;
/// `SQLITE_BUSY` and `SQLITE_LOCKED`, primary result codes.
const BUSY: i64 = 5;
const LOCKED: i64 = 6;

/// A Python `sqlite3.Connection` as a [`WriteTxn`]: statements run with
/// `Connection.execute`, so they use `sqlite3`'s statement cache. Never begins, commits or
/// rolls back on its own; whoever owns the transaction does.
#[derive(Debug)]
pub struct PyTxn<'a> {
    conn: &'a Py<PyAny>,
    /// The application's connection, not one the store opened: driver errors other than
    /// unique violations and conflicts are [`StoreError::Foreign`] (`docs/05-storage.md`
    /// §10).
    foreign: bool,
}

impl<'a> PyTxn<'a> {
    /// Wraps a connection the store owns. It should be in a transaction the caller
    /// controls (opened with `isolation_level=None` and an explicit `BEGIN`), or the
    /// statements autocommit one by one.
    pub fn new(conn: &'a Py<PyAny>) -> Self {
        Self {
            conn,
            foreign: false,
        }
    }

    /// Wraps an application's connection for a foreign-transaction write
    /// (`docs/05-storage.md` §10). As [`new`](Self::new), except that its other driver
    /// errors are reported as [`StoreError::Foreign`].
    pub fn foreign(conn: &'a Py<PyAny>) -> Self {
        Self {
            conn,
            foreign: true,
        }
    }

    fn err(&self, py: Python<'_>, e: &PyErr) -> StoreError {
        match store_err(py, e) {
            StoreError::Backend { message } if self.foreign => StoreError::Foreign { message },
            other => other,
        }
    }
}

fn params<'py>(py: Python<'py>, values: &[SqlValue]) -> PyResult<Bound<'py, PyTuple>> {
    let items: Vec<Bound<'py, PyAny>> = values
        .iter()
        .map(|v| match v {
            SqlValue::Null => py.None().into_bound(py),
            SqlValue::Integer(n) => PyInt::new(py, *n).into_any(),
            SqlValue::Text(s) => PyString::new(py, s).into_any(),
        })
        .collect();
    PyTuple::new(py, items)
}

/// Classifies a Python exception like `pemba-store-sqlite` classifies a driver error:
/// `sqlite3.Error.sqlite_errorcode` (Python ≥ 3.11) carries the extended result code.
pub(crate) fn store_err(py: Python<'_>, e: &PyErr) -> StoreError {
    let message = e.to_string();
    let code: Option<i64> = e
        .value(py)
        .getattr("sqlite_errorcode")
        .ok()
        .and_then(|c| c.extract().ok());
    match code {
        Some(CONSTRAINT_UNIQUE) => StoreError::UniqueViolation { message },
        Some(BUSY_SNAPSHOT) => StoreError::Conflict {
            message: format!(
                "SQLITE_BUSY_SNAPSHOT: the transaction read before another connection \
                 committed, so it cannot take the write lock ({message})"
            ),
        },
        Some(c) if matches!(c & 0xff, BUSY | LOCKED) => StoreError::Conflict { message },
        _ => StoreError::Backend { message },
    }
}

fn value(i: usize, v: &Bound<'_, PyAny>) -> Result<SqlValue, StoreError> {
    if v.is_none() {
        Ok(SqlValue::Null)
    } else if v.is_instance_of::<PyInt>() {
        v.extract::<i64>()
            .map(SqlValue::Integer)
            .map_err(|e| StoreError::Backend {
                message: format!("column {i}: {e}"),
            })
    } else if let Ok(s) = v.cast::<PyString>() {
        s.to_str()
            .map(|s| SqlValue::Text(s.to_owned()))
            .map_err(|e| StoreError::Backend {
                message: format!("column {i} is not UTF-8: {e}"),
            })
    } else {
        Err(StoreError::Backend {
            message: format!("column {i} is a REAL or BLOB, outside the portable SQL subset"),
        })
    }
}

impl WriteTxn for PyTxn<'_> {
    fn execute(&mut self, sql: &str, values: &[SqlValue]) -> Result<u64, StoreError> {
        Python::attach(|py| {
            let run = || -> PyResult<i64> {
                let cursor = self
                    .conn
                    .bind(py)
                    .call_method1("execute", (sql, params(py, values)?))?;
                cursor.getattr("rowcount")?.extract()
            };
            // `rowcount` is -1 for statements that change no rows.
            run()
                .map(|n| u64::try_from(n).unwrap_or(0))
                .map_err(|e| self.err(py, &e))
        })
    }

    fn query(&mut self, sql: &str, values: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        Python::attach(|py| {
            let rows = self
                .conn
                .bind(py)
                .call_method1(
                    "execute",
                    (sql, params(py, values).map_err(|e| self.err(py, &e))?),
                )
                .and_then(|cursor| cursor.call_method0("fetchall"))
                .map_err(|e| self.err(py, &e))?;
            let mut out = Vec::new();
            for row in rows.try_iter().map_err(|e| self.err(py, &e))? {
                let row = row.map_err(|e| self.err(py, &e))?;
                let mut values = Vec::new();
                for (i, v) in row.try_iter().map_err(|e| self.err(py, &e))?.enumerate() {
                    values.push(value(i, &v.map_err(|e| self.err(py, &e))?)?);
                }
                out.push(values);
            }
            Ok(out)
        })
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        match self
            .query("SELECT last_insert_rowid()", &[])?
            .first()
            .map(Vec::as_slice)
        {
            Some([SqlValue::Integer(n)]) => Ok(*n),
            other => Err(StoreError::Backend {
                message: format!("last_insert_rowid() returned {other:?}"),
            }),
        }
    }
}
