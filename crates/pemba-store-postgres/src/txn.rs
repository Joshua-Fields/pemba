//! [`WriteTxn`] over a `postgres` transaction, so the shared write algorithm of
//! [`pemba_core::write`] can run on this store's connections.
//!
//! Each statement is prepared first and its parameters converted to the Rust type of the
//! parameter type the server inferred: a portable [`SqlValue::Integer`] binds as `int8`,
//! `int4` or `int2` (range checked), [`SqlValue::Text`] as any text type, and
//! [`SqlValue::Null`] as a null of whatever type is expected. Results come back the same
//! way; a column of a type outside the portable subset fails closed.

use pemba_core::store::{SqlValue, StoreError, WriteTxn};
use postgres::GenericClient;
use postgres::types::{ToSql, Type};

use crate::store_err;

/// A [`WriteTxn`] on anything that runs statements: a `postgres::Transaction` in practice.
pub(crate) struct PostgresTxn<'a, C: GenericClient>(pub(crate) &'a mut C);

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

impl<C: GenericClient> PostgresTxn<'_, C> {
    fn prepare_bound(
        &mut self,
        sql: &str,
        params: &[SqlValue],
    ) -> Result<(postgres::Statement, Vec<Bound>), StoreError> {
        let stmt = self.0.prepare(sql).map_err(|e| store_err(&e))?;
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

impl<C: GenericClient> WriteTxn for PostgresTxn<'_, C> {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError> {
        let (stmt, bound) = self.prepare_bound(sql, params)?;
        let refs: Vec<_> = bound.iter().map(Bound::as_sql).collect();
        self.0.execute(&stmt, &refs).map_err(|e| store_err(&e))
    }

    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        let (stmt, bound) = self.prepare_bound(sql, params)?;
        let refs: Vec<_> = bound.iter().map(Bound::as_sql).collect();
        self.0
            .query(&stmt, &refs)
            .map_err(|e| store_err(&e))?
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
