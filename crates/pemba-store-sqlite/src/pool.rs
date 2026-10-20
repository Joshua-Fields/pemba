//! Fixed-size pool of read-only connections (`docs/05-storage.md` §4.2).
//!
//! Hand-rolled (a `Mutex<Vec<_>>` and a `Condvar`) rather than `r2d2`: the pool never
//! grows, never times out and never health-checks, so a pool crate would add threads and
//! dependencies for nothing.

use std::sync::{Condvar, Mutex};

use pemba_core::store::StoreError;
use rusqlite::Connection;

use crate::poisoned;

#[derive(Debug)]
pub(crate) struct ReaderPool {
    state: Mutex<State>,
    returned: Condvar,
}

#[derive(Debug)]
struct State {
    idle: Vec<Connection>,
    /// Connections in the pool, idle or leased; drops when a broken one is discarded.
    open: usize,
}

impl ReaderPool {
    pub(crate) fn new(conns: Vec<Connection>) -> Self {
        Self {
            state: Mutex::new(State {
                open: conns.len(),
                idle: conns,
            }),
            returned: Condvar::new(),
        }
    }

    /// Runs `f` on an idle connection, waiting for one if all are leased.
    pub(crate) fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        f(self.lease()?.conn()?)
    }

    /// Leases an idle connection until the lease is dropped, waiting for one if all are
    /// leased.
    pub(crate) fn lease(&self) -> Result<Lease<'_>, StoreError> {
        Ok(Lease {
            pool: self,
            conn: Some(self.take()?),
        })
    }

    fn take(&self) -> Result<Connection, StoreError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        loop {
            if let Some(conn) = state.idle.pop() {
                return Ok(conn);
            }
            if state.open == 0 {
                // Every connection was discarded; waiting would block forever.
                return Err(StoreError::Backend {
                    message: "no usable reader connections left".to_owned(),
                });
            }
            state = self.returned.wait(state).map_err(poisoned)?;
        }
    }

    fn give_back(&self, conn: Connection) {
        // A connection left inside a transaction (by an early return or a panic in `f`)
        // would pin an old snapshot for its next user, so end it or discard the connection.
        let usable = conn.is_autocommit() || conn.execute_batch("ROLLBACK").is_ok();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if usable {
            state.idle.push(conn);
        } else {
            state.open = state.open.saturating_sub(1);
        }
        // Wake one waiter either way: a discard may leave nothing to wait for.
        self.returned.notify_one();
    }
}

/// Returns its connection to the pool when dropped, including on unwind; a transaction
/// left open on it is rolled back then.
#[derive(Debug)]
pub(crate) struct Lease<'a> {
    pool: &'a ReaderPool,
    conn: Option<Connection>,
}

impl Lease<'_> {
    pub(crate) fn conn(&self) -> Result<&Connection, StoreError> {
        // Only `drop` takes the connection out.
        self.conn.as_ref().ok_or(StoreError::Poisoned)
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.give_back(conn);
        }
    }
}
