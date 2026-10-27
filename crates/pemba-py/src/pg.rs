//! Joining a Python app's Postgres transaction (`docs/10-postgres.md` §2.3): the shared
//! write algorithm runs in Rust, and each statement is forwarded to the app's own
//! connection inside its open transaction (ADR-006).
//!
//! Connections are recognised by duck typing; nothing here imports psycopg, `psycopg2`,
//! `SQLAlchemy` or Django. Every route ends at a raw psycopg 3 or `psycopg2` connection, whose
//! cursors take `%s` placeholders. The shared algorithm emits `$n` (the Rust driver's
//! style), so each statement is rewritten on the way: `$n` → `%s` with the parameters put
//! in placeholder order, and a literal `%` doubled, so that even a schema named with `%`
//! cannot be read as a placeholder.

use pemba::store::{SqlValue, StoreError, WriteTxn};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyInt, PyString, PyTuple};

/// libpq's `PQtransactionStatus` values, which psycopg 3 and psycopg2 both report.
const IDLE: i64 = 0;
const INTRANS: i64 = 2;
const INERROR: i64 = 3;

/// The app's connection, resolved to the raw driver connection pemba talks to.
pub(crate) struct Foreign {
    /// A psycopg 3 or psycopg2 connection.
    pub(crate) raw: Py<PyAny>,
    /// Whether pemba must open and commit the transaction itself (`allow_autocommit=True`
    /// on a connection with none open).
    pub(crate) own_transaction: bool,
}

fn type_name(v: &Bound<'_, PyAny>) -> String {
    v.get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_default()
}

fn module_name(v: &Bound<'_, PyAny>) -> String {
    v.get_type()
        .module()
        .map(|m| m.to_string())
        .unwrap_or_default()
}

/// Whether `conn` is an async connection or session: its `execute` is a coroutine
/// function, or its type says so.
fn is_async(conn: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = conn.py();
    let coroutine = match conn.getattr("execute") {
        Ok(execute) => py
            .import("inspect")?
            .call_method1("iscoroutinefunction", (execute,))?
            .is_truthy()?,
        Err(_) => false,
    };
    let module = module_name(conn);
    // SQLAlchemy's dialect adapters (`AsyncAdapt_*`) are sync facades over an async
    // driver, usable inside `AsyncSession.run_sync`; its async API lives elsewhere.
    let named_async =
        type_name(conn).starts_with("Async") && !module.starts_with("sqlalchemy.dialects");
    Ok(coroutine || named_async || module.starts_with("asyncpg"))
}

/// Async connections and sessions cannot be driven from a synchronous write (§2.4).
fn reject_async(conn: &Bound<'_, PyAny>) -> PyResult<()> {
    if is_async(conn)? {
        let (module, name) = (module_name(conn), type_name(conn));
        return Err(PyTypeError::new_err(format!(
            "conn is an async connection ({module}.{name}); write(conn=) is synchronous. Until \
             write_async arrives, use a sync connection in a worker thread, or \
             AsyncSession.run_sync (docs/10-postgres.md §2.4)"
        )));
    }
    Ok(())
}

/// The connection to send statements to, from the pooled `DBAPI` connection of a
/// `SQLAlchemy` session or connection: the raw driver connection, or, when that is async
/// (`AsyncSession.run_sync`), the library's own sync adapter around it, whose `cursor()`
/// runs inside the session's greenlet.
fn driver<'py>(pooled: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let raw = pooled.getattr("driver_connection")?;
    if is_async(&raw)? && pooled.hasattr("cursor")? {
        let adapter = pooled
            .getattr("dbapi_connection")
            .unwrap_or_else(|_| pooled.clone());
        if !is_async(&adapter)? {
            return Ok(adapter);
        }
    }
    Ok(raw)
}

/// `conn.info.transaction_status` of a raw psycopg connection, if it is one.
fn libpq_status(conn: &Bound<'_, PyAny>) -> Option<i64> {
    conn.getattr("info")
        .and_then(|info| info.getattr("transaction_status"))
        .and_then(|s| {
            s.extract::<i64>()
                .or_else(|_| s.getattr("value")?.extract())
        })
        .ok()
}

/// Resolves `conn` to a raw psycopg connection and checks it is in a usable transaction.
///
/// # Errors
///
/// `TypeError` for an object that is no Postgres connection pemba knows, or an async one;
/// `ValueError` for a failed transaction, one with a statement still running, or none
/// open without `allow_autocommit`.
pub(crate) fn foreign(conn: &Bound<'_, PyAny>, allow_autocommit: bool) -> PyResult<Foreign> {
    reject_async(conn)?;
    // SQLAlchemy and Django own their connection's transaction, so pemba never opens one
    // on them: they must already be in one, `allow_autocommit` or not.
    let require_open = |open: bool, how: &str| -> PyResult<()> {
        if open {
            Ok(())
        } else {
            Err(PyValueError::new_err(format!(
                "conn has no open transaction, so this write would not be atomic with the \
                 application's data; {how}"
            )))
        }
    };
    let (raw, via_wrapper) =
        if conn.hasattr("get_autocommit")? && conn.hasattr("ensure_connection")? {
            // Django's `connection`: inside `atomic()` autocommit is off.
            require_open(
                !conn.call_method0("get_autocommit")?.extract::<bool>()?,
                "write inside transaction.atomic()",
            )?;
            conn.call_method0("ensure_connection")?;
            (conn.getattr("connection")?, true)
        } else if conn.hasattr("in_transaction")? && conn.hasattr("get_bind")? {
            // A SQLAlchemy `Session`, checked before `connection()`, which would begin one.
            require_open(
                conn.call_method0("in_transaction")?.extract::<bool>()?,
                "write inside session.begin() (or after another statement in the session)",
            )?;
            (
                driver(&conn.call_method0("connection")?.getattr("connection")?)?,
                true,
            )
        } else if conn.hasattr("in_transaction")? && conn.hasattr("connection")? {
            // A SQLAlchemy `Connection`.
            require_open(
                conn.call_method0("in_transaction")?.extract::<bool>()?,
                "write inside connection.begin()",
            )?;
            (driver(&conn.getattr("connection")?)?, true)
        } else {
            (conn.clone(), false)
        };
    reject_async(&raw)?;
    if via_wrapper && !module_name(&raw).starts_with("psycopg") {
        // SQLAlchemy's sync adapter over an async driver, inside `AsyncSession.run_sync`
        // (§2.4): the session vouched for its transaction; a failed one fails the first
        // statement.
        return Ok(Foreign {
            raw: raw.unbind(),
            own_transaction: false,
        });
    }
    let module = module_name(&raw);
    if !(module.starts_with("psycopg") && raw.hasattr("cursor")?) {
        return Err(PyTypeError::new_err(format!(
            "conn must be a psycopg 3 or psycopg2 connection, a SQLAlchemy Session or \
             Connection on one, or django.db.connection, not {module}.{}",
            type_name(conn)
        )));
    }
    let status = libpq_status(&raw).ok_or_else(|| {
        PyTypeError::new_err("conn has no info.transaction_status (psycopg2 >= 2.8 is required)")
    })?;
    if status == INERROR {
        return Err(PyValueError::new_err(
            "conn is in a failed transaction; roll it back before writing",
        ));
    }
    if status != IDLE && status != INTRANS {
        return Err(PyValueError::new_err(
            "conn has a statement in progress; finish it before writing",
        ));
    }
    // A connection outside autocommit starts a transaction with its first statement, which
    // the app later commits; that is atomic too.
    let open = via_wrapper
        || status == INTRANS
        || !raw
            .getattr("autocommit")
            .and_then(|a| a.extract::<bool>())
            .unwrap_or(true);
    if !open && !allow_autocommit {
        return Err(PyValueError::new_err(
            "conn has no open transaction, so this write would commit on its own and not \
             atomically with the application's data; open a transaction first, or pass \
             allow_autocommit=True",
        ));
    }
    Ok(Foreign {
        raw: raw.unbind(),
        own_transaction: !open,
    })
}

/// `sql` with `$n` placeholders as `%s`, and `values` in placeholder order; `%` doubled.
fn format_style(sql: &str, values: &[SqlValue]) -> Result<(String, Vec<SqlValue>), StoreError> {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut params = Vec::with_capacity(values.len());
    let mut chars = sql.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        match c {
            '%' => out.push_str("%%"),
            '$' if chars.peek().is_some_and(|(_, d)| d.is_ascii_digit()) => {
                let mut n = 0usize;
                while let Some((_, d)) = chars.peek().copied().filter(|(_, d)| d.is_ascii_digit()) {
                    n = n * 10 + (d as usize - '0' as usize);
                    chars.next();
                }
                let v = n
                    .checked_sub(1)
                    .and_then(|i| values.get(i))
                    .ok_or_else(|| StoreError::Backend {
                        message: format!(
                            "statement names ${n}, but {} parameters were given",
                            values.len()
                        ),
                    })?;
                params.push(v.clone());
                out.push_str("%s");
            }
            c => out.push(c),
        }
    }
    Ok((out, params))
}

/// Classifies a driver exception by its SQLSTATE: psycopg 3's `sqlstate`, psycopg2's
/// `pgcode`.
fn store_err(py: Python<'_>, e: &PyErr) -> StoreError {
    let message = e.to_string();
    let value = e.value(py);
    let code: Option<String> = ["sqlstate", "pgcode"]
        .iter()
        .find_map(|a| value.getattr(*a).ok()?.extract().ok());
    match code.as_deref() {
        Some("23505") => StoreError::UniqueViolation { message },
        Some("40001" | "40P01" | "55P03") => StoreError::Conflict { message },
        // 25P02 (already aborted) and the rest: the app's connection failed.
        _ => StoreError::Foreign { message },
    }
}

/// A raw psycopg 3 or psycopg2 connection as a [`WriteTxn`]: one cursor, reused.
pub(crate) struct PgPyTxn {
    cursor: Py<PyAny>,
}

impl PgPyTxn {
    /// Opens a cursor on `raw`.
    pub(crate) fn new(py: Python<'_>, raw: &Py<PyAny>) -> PyResult<Self> {
        Ok(Self {
            cursor: raw.bind(py).call_method0("cursor")?.unbind(),
        })
    }

    /// Runs `sql` with no parameters (`BEGIN`, `COMMIT`, `ROLLBACK`).
    pub(crate) fn simple(&self, py: Python<'_>, sql: &str) -> PyResult<()> {
        self.cursor.bind(py).call_method1("execute", (sql,))?;
        Ok(())
    }

    fn run<'py>(
        &self,
        py: Python<'py>,
        sql: &str,
        values: &[SqlValue],
    ) -> Result<Bound<'py, PyAny>, StoreError> {
        let (sql, values) = format_style(sql, values)?;
        let params: Vec<Bound<'py, PyAny>> = values
            .iter()
            .map(|v| match v {
                SqlValue::Null => py.None().into_bound(py),
                SqlValue::Integer(n) => PyInt::new(py, *n).into_any(),
                SqlValue::Text(s) => PyString::new(py, s).into_any(),
            })
            .collect();
        let params = PyTuple::new(py, params).map_err(|e| store_err(py, &e))?;
        let cursor = self.cursor.bind(py);
        cursor
            .call_method1("execute", (sql, params))
            .map_err(|e| store_err(py, &e))?;
        Ok(cursor.clone())
    }
}

fn value(i: usize, v: &Bound<'_, PyAny>) -> Result<SqlValue, StoreError> {
    if v.is_none() {
        Ok(SqlValue::Null)
    } else if let Ok(b) = v.cast::<PyBool>() {
        Ok(SqlValue::Integer(i64::from(b.is_true())))
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
            message: format!(
                "column {i} has Python type {}, outside the portable SQL subset",
                type_name(v)
            ),
        })
    }
}

impl WriteTxn for PgPyTxn {
    fn execute(&mut self, sql: &str, values: &[SqlValue]) -> Result<u64, StoreError> {
        Python::attach(|py| {
            let cursor = self.run(py, sql, values)?;
            // -1 when the driver cannot tell.
            let n: i64 = cursor
                .getattr("rowcount")
                .and_then(|n| n.extract())
                .map_err(|e| store_err(py, &e))?;
            Ok(u64::try_from(n).unwrap_or(0))
        })
    }

    fn query(&mut self, sql: &str, values: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        Python::attach(|py| {
            let cursor = self.run(py, sql, values)?;
            let rows = cursor
                .call_method0("fetchall")
                .map_err(|e| store_err(py, &e))?;
            let mut out = Vec::new();
            for row in rows.try_iter().map_err(|e| store_err(py, &e))? {
                let row = row.map_err(|e| store_err(py, &e))?;
                let mut values = Vec::new();
                for (i, v) in row.try_iter().map_err(|e| store_err(py, &e))?.enumerate() {
                    values.push(value(i, &v.map_err(|e| store_err(py, &e))?)?);
                }
                out.push(values);
            }
            Ok(out)
        })
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        // Only `RevisionStrategy::AllocateNow` asks; Postgres assigns at commit.
        Err(StoreError::Backend {
            message: "last_insert_id is not available on Postgres; use RETURNING".to_owned(),
        })
    }
}
