//! Migration runner (`docs/05-storage.md` §4.1): the shared runner of
//! `warden_sqlite_common::migrate`, on a `rusqlite` connection.

use rusqlite::{Connection, TransactionBehavior};
use warden_core::Error;
use warden_sqlite_common::migrate;

use crate::db;
use crate::txn::{Autocommit, SqliteTxn};

/// Succeeds only if the database is exactly at the library's storage version.
pub(crate) fn check(conn: &Connection) -> Result<(), Error> {
    migrate::check(&mut Autocommit(conn))
}

/// Brings the database to the library's storage version in one immediate transaction.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), Error> {
    // IMMEDIATE takes the write lock before reading the version, so two processes migrating
    // at once serialise and the second finds nothing left to do.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    migrate::migrate(&mut SqliteTxn::new(&tx))?;
    tx.commit().map_err(db)
}
