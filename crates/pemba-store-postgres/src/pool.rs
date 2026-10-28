//! Fixed-size connection pool (`docs/10-postgres.md` §2.1).
//!
//! Hand-rolled like the SQLite reader pool (a `Mutex` and a `Condvar`) rather than
//! `r2d2_postgres`: about the same amount of code, one dependency fewer. Unlike SQLite's
//! it grows on demand up to its size, because a Postgres connection costs a server
//! process, and it replaces connections that broke rather than shrinking for good.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use pemba_core::store::StoreError;
use postgres::{Client, Config};

use crate::statements::Conn;
use crate::tls::Connector;

/// Runs on every new connection before the pool hands it out: pemba's own SQL resolves
/// functions and operators in `pg_catalog` only, whatever the role's or DSN's
/// `search_path` says (`docs/10-postgres.md` §3.3). Tables are qualified anyway.
const SESSION_SETUP: &str = "SET search_path = pg_catalog, pg_temp";

pub(crate) struct Pool {
    config: Config,
    connector: Connector,
    size: usize,
    state: Mutex<State>,
    returned: Condvar,
    /// Shared by every connection's statement cache, to invalidate them all at once.
    generation: Arc<AtomicU64>,
}

struct State {
    idle: Vec<Conn>,
    /// Connections idle, leased or being opened.
    open: usize,
}

/// Opens one connection set up as the pool's are.
pub(crate) fn connect(config: &Config, connector: &Connector) -> Result<Client, StoreError> {
    let mut client = connector
        .connect(config)
        .map_err(|e| crate::store_err(&e))?;
    setup(&mut client)?;
    Ok(client)
}

/// Opens one connection without [`SESSION_SETUP`], for reading the connection's own
/// `current_schema()`; pass it to [`setup`] before any other use.
pub(crate) fn connect_raw(config: &Config, connector: &Connector) -> Result<Client, StoreError> {
    connector.connect(config).map_err(|e| crate::store_err(&e))
}

pub(crate) fn setup(client: &mut Client) -> Result<(), StoreError> {
    client
        .batch_execute(SESSION_SETUP)
        .map_err(|e| crate::store_err(&e))
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `postgres::Client` has no `Debug`, and the config is not worth printing.
        f.debug_struct("Pool")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Pool {
    /// A pool of at most `size` connections, starting with `first` (already set up).
    pub(crate) fn new(config: Config, connector: Connector, size: usize, first: Client) -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        Self {
            config,
            connector,
            size,
            state: Mutex::new(State {
                idle: vec![Conn::new(first, Arc::clone(&generation))],
                open: 1,
            }),
            returned: Condvar::new(),
            generation,
        }
    }

    /// Leases a connection until the lease is dropped: an idle one, a new one while the
    /// pool is below its size, or else the next one returned.
    pub(crate) fn lease(&self) -> Result<Lease<'_>, StoreError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        loop {
            if let Some(conn) = state.idle.pop() {
                return Ok(Lease {
                    pool: self,
                    conn: Some(conn),
                });
            }
            if state.open < self.size {
                state.open += 1;
                drop(state);
                // Connect outside the lock: it is a network round trip or several.
                return match connect(&self.config, &self.connector) {
                    Ok(client) => Ok(Lease {
                        pool: self,
                        conn: Some(Conn::new(client, Arc::clone(&self.generation))),
                    }),
                    Err(e) => {
                        self.forget_one();
                        Err(e)
                    }
                };
            }
            state = self.returned.wait(state).map_err(poisoned)?;
        }
    }

    /// Empties every connection's statement cache, each the next time it is used.
    pub(crate) fn invalidate_statements(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn give_back(&self, conn: Conn) {
        // A `postgres::Transaction` rolls back when dropped, and it borrows the client; a
        // snapshot ends its raw transaction itself or discards the client. So a client
        // coming back is never inside one of ours. A closed one is discarded and its slot
        // freed for a fresh connection.
        if conn.client.is_closed() {
            self.forget_one();
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.idle.push(conn);
        self.returned.notify_one();
    }

    fn forget_one(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.open = state.open.saturating_sub(1);
        }
        self.returned.notify_one();
    }
}

/// Returns its connection to the pool when dropped, including on unwind.
pub(crate) struct Lease<'a> {
    pool: &'a Pool,
    conn: Option<Conn>,
}

impl Lease<'_> {
    pub(crate) fn conn(&mut self) -> Result<&mut Conn, StoreError> {
        // Only `drop` and `discard` take the connection out.
        self.conn.as_mut().ok_or(StoreError::Poisoned)
    }

    /// The connection without its statement cache, for statements that skip it.
    pub(crate) fn client(&mut self) -> Result<&mut Client, StoreError> {
        Ok(&mut self.conn()?.client)
    }

    /// Closes the connection instead of returning it, for one whose state cannot be
    /// trusted (e.g. a transaction that would not roll back); its slot is freed.
    pub(crate) fn discard(mut self) {
        if self.conn.take().is_some() {
            self.pool.forget_one();
        }
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.give_back(conn);
        }
    }
}

fn poisoned<T>(_: PoisonError<T>) -> StoreError {
    StoreError::Poisoned
}
