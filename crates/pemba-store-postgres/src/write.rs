//! [`WritableStore`] for [`PostgresStore`]: native writes (`docs/10-postgres.md` §1.6.1).
//!
//! Each call runs the shared algorithm of [`pemba_core::write`] in its own READ COMMITTED
//! transaction on a pooled connection. The rows carry a batch id; at `COMMIT` the trigger
//! maps the batch to the next revision, and the same connection, now in autocommit, looks
//! that revision up. Errors are classified by [`store_err`](crate::store_err): SQLSTATE
//! 40001 and 40P01 are the retryable [`StoreError::Conflict`].

use std::sync::atomic::Ordering;

use pemba_core::Error;
use pemba_core::model::{RelationshipFilter, Revision, WriteRequest};
use pemba_core::store::{
    DeleteResult, SchemaCheck, SqlDialect, StoreError, StoredSchema, WritableStore, WriteTxn,
};
use pemba_core::write::{self as shared, Applied};
use postgres::{Client, IsolationLevel};

use crate::txn::PostgresTxn;
use crate::{PostgresStore, store_err};

fn db(e: &postgres::Error) -> Error {
    Error::Store(store_err(e))
}

fn corrupt(message: String) -> Error {
    Error::Store(StoreError::Corrupt { message })
}

/// Runs `f` in a READ COMMITTED transaction on `client` and commits it. On error the
/// transaction is rolled back when dropped.
fn transact<T>(
    client: &mut Client,
    f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut tx = client
        .build_transaction()
        .isolation_level(IsolationLevel::ReadCommitted)
        .start()
        .map_err(|e| db(&e))?;
    let out = f(&mut PostgresTxn(&mut tx))?;
    tx.commit().map_err(|e| db(&e))?;
    Ok(out)
}

impl PostgresStore {
    /// Runs one write that queues a batch, commits it, and returns the revision the
    /// commit gave it.
    ///
    /// If the connection fails after `COMMIT` was sent, the write may have committed
    /// although this returns `Err`: retrying a `Touch` or `Delete` is harmless, a
    /// `Create` then fails with `AlreadyExists`.
    fn native<T>(
        &self,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<(T, Applied), Error>,
    ) -> Result<(T, Revision), Error> {
        let mut lease = self.pool.lease()?;
        let client = lease.client()?;
        let (out, applied) = transact(client, f)?;
        let Applied::Pending { batch, .. } = applied else {
            return Err(corrupt(format!(
                "this store assigns revisions at commit, but the write returned {applied:?}"
            )));
        };
        let rev = self
            .revision_of(client, batch)?
            .ok_or_else(|| corrupt(format!("batch {batch} committed without a revision")))?;
        Ok((out, rev))
    }

    /// The revision the commit trigger gave `batch`, or `None` if it has none: its
    /// transaction has not committed, or rolled back. A revision found is committed, so
    /// the cached head moves up to it: reads on the store right after see the write.
    ///
    /// If the trigger fired before `COMMIT` (`SET CONSTRAINTS ... IMMEDIATE`,
    /// `docs/10-postgres.md` §1.6.7), warns through `tracing`, once per store: apps that
    /// never read the server's `WARNING` still find out.
    pub(crate) fn revision_of(
        &self,
        client: &mut Client,
        batch: u64,
    ) -> Result<Option<Revision>, Error> {
        let batch_id = i64::try_from(batch)
            .map_err(|_| corrupt(format!("batch id {batch} is out of range")))?;
        let row = client
            .query_opt(
                &format!(
                    "SELECT rev, early FROM {} WHERE batch_id = $1",
                    self.dialect.table("pemba_revisions")
                ),
                &[&batch_id],
            )
            .map_err(|e| db(&e))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let rev: i64 = row.try_get(0).map_err(|e| db(&e))?;
        let early: bool = row.try_get(1).map_err(|e| db(&e))?;
        let rev =
            Revision(u64::try_from(rev).map_err(|_| corrupt(format!("negative revision {rev}")))?);
        self.cache.observe_head(rev);
        if early && !self.early_warned.swap(true, Ordering::AcqRel) {
            tracing::warn!(
                revision = rev.0,
                batch,
                "pemba: revision assigned before COMMIT, by SET CONSTRAINTS ... IMMEDIATE in \
                 the writing transaction; every other pemba commit on this store waited for \
                 that transaction to end. Name your own constraints instead of SET \
                 CONSTRAINTS ALL IMMEDIATE (docs/10-postgres.md §1.6.7). Warned once per store."
            );
        }
        Ok(Some(rev))
    }

    /// Runs `f` in its own committed transaction, for the GC steps, which commit no
    /// revision.
    fn unversioned<T>(
        &self,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut lease = self.pool.lease()?;
        transact(lease.client()?, f)
    }
}

impl WritableStore for PostgresStore {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        let ((), rev) = self.native(|txn| {
            shared::write(txn, &self.dialect, req, schema_version).map(|a| ((), a))
        })?;
        Ok(rev)
    }

    /// Serialised against other schema writes, and against writes checking the version,
    /// by the schema lock ([`SqlDialect::schema_lock`]).
    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        let (written, rev) = self.native(|txn| {
            let w = shared::write_schema(txn, &self.dialect, source, hash, check)?;
            let applied = w.applied;
            Ok((w, applied))
        })?;
        Ok(written.at(source, hash, rev))
    }

    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error> {
        let (deleted, rev) = self.native(|txn| {
            let d = shared::delete_matching(txn, &self.dialect, filter, limit)?;
            Ok((d, d.applied))
        })?;
        Ok(deleted.at(rev))
    }

    /// Its own transaction, committed before any row is removed (`docs/05-storage.md` §8).
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        let horizon =
            self.unversioned(|txn| shared::raise_gc_horizon(txn, &self.dialect, keep_from))?;
        self.cache.observe(Revision(0), horizon);
        Ok(horizon)
    }

    /// Removes tuples whose delete batch maps to a revision at or below the stored
    /// horizon. Pruning `pemba_pending` and the batch map is T5.7.
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        self.unversioned(|txn| shared::gc_batch(txn, &self.dialect, max_rows))
    }
}
