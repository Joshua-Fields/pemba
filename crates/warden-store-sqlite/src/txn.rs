//! [`WriteTxn`] over a `rusqlite` transaction, and the SQLite [`SqlDialect`].
//!
//! The same adapter serves the store's own connection (inside `BEGIN IMMEDIATE`) and a
//! caller's transaction passed to `Engine::write_in`, so both run the one write algorithm
//! in `warden_core::write` (`docs/05-storage.md` §5, ADR-006).

use rusqlite::types::ValueRef;
use rusqlite::{Connection, Transaction};
use warden_core::store::{SqlDialect, SqlValue, StoreError, WriteTxn};

use crate::store_err;

/// A caller-owned SQLite transaction, as a [`WriteTxn`] for `Engine::write_in`.
///
/// ```no_run
/// # fn demo(engine: &warden_core::Engine<warden_store_sqlite::SqliteStore>,
/// #         req: warden_core::model::WriteRequest) -> Result<(), Box<dyn std::error::Error>> {
/// use rusqlite::{Connection, TransactionBehavior};
/// use warden_store_sqlite::SqliteTxn;
///
/// let mut conn = Connection::open("app.db")?;
/// let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
/// tx.execute("INSERT INTO documents (id) VALUES ('readme')", [])?;
/// let rev = engine.write_in(&mut SqliteTxn::new(&tx), req)?;
/// tx.commit()?; // only now is `rev` visible; a rollback voids it
/// # let _ = rev; Ok(()) }
/// ```
///
/// Statements are prepared with `prepare_cached`, so they are reused across writes on the
/// same connection.
#[derive(Debug)]
pub struct SqliteTxn<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteTxn<'a> {
    /// Wraps an open transaction. Taking a [`Transaction`] (not a bare connection) means
    /// the write is always part of a transaction the caller commits or rolls back.
    pub fn new(tx: &'a Transaction<'_>) -> Self {
        Self { conn: tx }
    }
}

fn param(v: &SqlValue) -> rusqlite::types::Value {
    match v {
        SqlValue::Null => rusqlite::types::Value::Null,
        SqlValue::Integer(n) => rusqlite::types::Value::Integer(*n),
        SqlValue::Text(s) => rusqlite::types::Value::Text(s.clone()),
    }
}

impl WriteTxn for SqliteTxn<'_> {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError> {
        let mut stmt = self.conn.prepare_cached(sql).map_err(|e| store_err(&e))?;
        let n = stmt
            .execute(rusqlite::params_from_iter(params.iter().map(param)))
            .map_err(|e| store_err(&e))?;
        Ok(n as u64)
    }

    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        let mut stmt = self.conn.prepare_cached(sql).map_err(|e| store_err(&e))?;
        let columns = stmt.column_count();
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params.iter().map(param)))
            .map_err(|e| store_err(&e))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| store_err(&e))? {
            let mut values = Vec::with_capacity(columns);
            for i in 0..columns {
                values.push(match row.get_ref(i).map_err(|e| store_err(&e))? {
                    ValueRef::Null => SqlValue::Null,
                    ValueRef::Integer(n) => SqlValue::Integer(n),
                    ValueRef::Text(t) => SqlValue::Text(
                        std::str::from_utf8(t)
                            .map_err(|e| StoreError::Backend {
                                message: format!("column {i} is not UTF-8: {e}"),
                            })?
                            .to_owned(),
                    ),
                    ValueRef::Real(_) | ValueRef::Blob(_) => {
                        return Err(StoreError::Backend {
                            message: format!(
                                "column {i} is a REAL or BLOB, outside the portable SQL subset"
                            ),
                        });
                    }
                });
            }
            out.push(values);
        }
        Ok(out)
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        Ok(self.conn.last_insert_rowid())
    }
}

/// A bare connection as a [`WriteTxn`], for the store's own setup queries outside any
/// transaction. Never handed to the write algorithm.
pub(crate) struct Autocommit<'a>(pub(crate) &'a Connection);

impl WriteTxn for Autocommit<'_> {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError> {
        SqliteTxn { conn: self.0 }.execute(sql, params)
    }

    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        SqliteTxn { conn: self.0 }.query(sql, params)
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        Ok(self.0.last_insert_rowid())
    }
}

/// SQLite statement text: `?` placeholders, unprefixed table names.
#[derive(Debug)]
pub(crate) struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn placeholder(&self, _n: usize) -> String {
        // Plain `?` (bound in order) rather than `?n`, as `docs/05-storage.md` §2 says: it
        // is what Python's `sqlite3` qmark style expects for the foreign path too.
        "?".to_owned()
    }

    fn table(&self, name: &str) -> String {
        name.to_owned()
    }
}
