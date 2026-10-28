//! [`WriteTxn`] over a `postgres` transaction, so the shared write algorithm of
//! [`pemba_core::write`] can run on this store's connections and, through
//! `Engine::write_in`, inside the app's own transactions.
//!
//! Each statement is prepared first and its parameters converted to the Rust type of the
//! parameter type the server inferred: a portable [`SqlValue::Integer`] binds as `int8`,
//! `int4` or `int2` (range checked), [`SqlValue::Text`] as any text type, and
//! [`SqlValue::Null`] as a null of whatever type is expected. Results come back the same
//! way; a column of a type outside the portable subset fails closed.

use pemba_core::store::{SqlValue, StoreError, WriteTxn};
use postgres::Transaction;
use postgres::types::{ToSql, Type};

use crate::statements::StatementCache;
use crate::store_err;

/// The app's own `postgres` transaction, for
/// [`Engine::write_in`](pemba_core::Engine::write_in) on a [`PostgresStore`]
/// (`docs/10-postgres.md` §1.6.4): the relationships commit or roll back with the app's
/// rows.
///
/// Only a [`Transaction`] (or a savepoint, which is one) is accepted, so a write can never
/// run in autocommit, where it would not be atomic. Any isolation level works; under
/// REPEATABLE READ and SERIALIZABLE, retry the whole transaction on the retryable
/// `TRANSACTION_CONFLICT`, with backoff (`docs/06-api.md` §1).
///
/// ```no_run
/// # use pemba_core::{Engine, Error};
/// # use pemba_core::model::WriteRequest;
/// # use pemba_store_postgres::{PgTxn, PostgresStore};
/// # fn example(engine: &Engine<PostgresStore>, client: &mut postgres::Client, req: WriteRequest)
/// # -> Result<(), Box<dyn std::error::Error>> {
/// let mut tx = client.transaction()?;
/// tx.execute("INSERT INTO documents (id) VALUES ($1)", &[&"readme"])?;
/// let pending = engine.write_in(&mut PgTxn::new(&mut tx), req)?;
/// tx.commit()?; // only now does the write get its revision
/// let rev = engine.resolve(&pending)?;
/// # Ok(())
/// # }
/// ```
///
/// [`PostgresStore`]: crate::PostgresStore
pub struct PgTxn<'a, 't> {
    tx: &'a mut Transaction<'t>,
    /// The connection's statement cache, on the store's own connections only: an app's
    /// connection is not the store's to keep statements on.
    statements: Option<&'a mut StatementCache>,
}

impl<'a, 't> PgTxn<'a, 't> {
    /// Wraps `tx`; nothing is sent until the engine writes.
    pub fn new(tx: &'a mut Transaction<'t>) -> Self {
        Self {
            tx,
            statements: None,
        }
    }

    /// Wraps `tx`, a transaction on a pooled connection whose cache is `statements`.
    pub(crate) fn cached(tx: &'a mut Transaction<'t>, statements: &'a mut StatementCache) -> Self {
        Self {
            tx,
            statements: Some(statements),
        }
    }
}

impl std::fmt::Debug for PgTxn<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgTxn").finish_non_exhaustive()
    }
}

/// One parameter, typed for the statement.
enum Bound {
    I64(Option<i64>),
    I32(Option<i32>),
    I16(Option<i16>),
    Text(Option<String>),
    Bool(Option<bool>),
}

impl Bound {
    fn as_sql(&self) -> &(dyn ToSql + Sync) {
        match self {
            Self::I64(v) => v,
            Self::I32(v) => v,
            Self::I16(v) => v,
            Self::Text(v) => v,
            Self::Bool(v) => v,
        }
    }
}

fn mismatch(i: usize, value: &SqlValue, ty: &Type) -> StoreError {
    StoreError::Backend {
        message: format!(
            "parameter ${} is {value:?}, which cannot bind as {ty}",
            i + 1
        ),
    }
}

fn is_text(ty: &Type) -> bool {
    [Type::TEXT, Type::VARCHAR, Type::BPCHAR, Type::NAME].contains(ty)
}

fn bind(i: usize, value: &SqlValue, ty: &Type) -> Result<Bound, StoreError> {
    let out = match value {
        SqlValue::Null if *ty == Type::INT8 => Bound::I64(None),
        SqlValue::Null if *ty == Type::INT4 => Bound::I32(None),
        SqlValue::Null if *ty == Type::INT2 => Bound::I16(None),
        SqlValue::Null if *ty == Type::BOOL => Bound::Bool(None),
        SqlValue::Null if is_text(ty) => Bound::Text(None),
        SqlValue::Integer(n) if *ty == Type::INT8 => Bound::I64(Some(*n)),
        SqlValue::Integer(n) if *ty == Type::INT4 => {
            Bound::I32(Some(i32::try_from(*n).map_err(|_| mismatch(i, value, ty))?))
        }
        SqlValue::Integer(n) if *ty == Type::INT2 => {
            Bound::I16(Some(i16::try_from(*n).map_err(|_| mismatch(i, value, ty))?))
        }
        SqlValue::Text(s) if is_text(ty) => Bound::Text(Some(s.clone())),
        _ => return Err(mismatch(i, value, ty)),
    };
    Ok(out)
}

fn column(row: &postgres::Row, i: usize) -> Result<SqlValue, StoreError> {
    let ty = row.columns()[i].type_();
    let get_err = |e: postgres::Error| store_err(&e);
    let v = if *ty == Type::INT8 {
        row.try_get::<_, Option<i64>>(i)
            .map_err(get_err)?
            .map(SqlValue::Integer)
    } else if *ty == Type::INT4 {
        row.try_get::<_, Option<i32>>(i)
            .map_err(get_err)?
            .map(|n| SqlValue::Integer(n.into()))
    } else if *ty == Type::INT2 {
        row.try_get::<_, Option<i16>>(i)
            .map_err(get_err)?
            .map(|n| SqlValue::Integer(n.into()))
    } else if *ty == Type::BOOL {
        row.try_get::<_, Option<bool>>(i)
            .map_err(get_err)?
            .map(|b| SqlValue::Integer(b.into()))
    } else if is_text(ty) {
        row.try_get::<_, Option<String>>(i)
            .map_err(get_err)?
            .map(SqlValue::Text)
    } else {
        return Err(StoreError::Backend {
            message: format!("column {i} has type {ty}, outside the portable SQL subset"),
        });
    };
    Ok(v.unwrap_or(SqlValue::Null))
}

impl PgTxn<'_, '_> {
    fn prepare_bound(
        &mut self,
        sql: &str,
        params: &[SqlValue],
    ) -> Result<(postgres::Statement, Vec<Bound>), StoreError> {
        let tx = &mut *self.tx;
        let stmt = match self.statements.as_deref_mut() {
            Some(cache) => cache.get(sql, |s| tx.prepare(s)),
            None => tx.prepare(sql),
        }
        .map_err(|e| store_err(&e))?;
        if stmt.params().len() != params.len() {
            return Err(StoreError::Backend {
                message: format!(
                    "statement takes {} parameters, {} given",
                    stmt.params().len(),
                    params.len()
                ),
            });
        }
        let bound = params
            .iter()
            .zip(stmt.params())
            .enumerate()
            .map(|(i, (v, ty))| bind(i, v, ty))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((stmt, bound))
    }
}

impl PgTxn<'_, '_> {
    /// An error from running a statement, shown to the cache first (a stale plan
    /// invalidates it).
    fn failed(&mut self, e: &postgres::Error) -> StoreError {
        if let Some(cache) = self.statements.as_deref_mut() {
            cache.saw(e);
        }
        store_err(e)
    }
}

impl WriteTxn for PgTxn<'_, '_> {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError> {
        let (stmt, bound) = self.prepare_bound(sql, params)?;
        let refs: Vec<_> = bound.iter().map(Bound::as_sql).collect();
        self.tx.execute(&stmt, &refs).map_err(|e| self.failed(&e))
    }

    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        let (stmt, bound) = self.prepare_bound(sql, params)?;
        let refs: Vec<_> = bound.iter().map(Bound::as_sql).collect();
        self.tx
            .query(&stmt, &refs)
            .map_err(|e| self.failed(&e))?
            .iter()
            .map(|row| (0..row.len()).map(|i| column(row, i)).collect())
            .collect()
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        // Only `RevisionStrategy::AllocateNow` asks for it; this store assigns at commit.
        Err(StoreError::Backend {
            message: "last_insert_id is not available on Postgres; use RETURNING".to_owned(),
        })
    }
}
