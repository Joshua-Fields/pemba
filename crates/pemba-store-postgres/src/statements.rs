//! Per-connection prepared-statement cache (OQ-056, `docs/10-postgres.md` §2.1).
//!
//! Without it every statement costs two round trips: `Parse`/`Describe`, then
//! `Bind`/`Execute`. With it, a statement this connection has run before costs one. The
//! store's SQL texts depend on a request's shape, never on its values, so a few dozen
//! entries cover every path; the bound only guards against an unforeseen source of texts.
//!
//! Entries live with their connection: a closed or discarded connection takes its cache
//! with it, and a replacement starts empty. A shared generation invalidates every
//! connection's cache at once; it is bumped when the server reports a stale plan
//! (`0A000`, "cached plan must not change result type", after DDL on a table a statement
//! reads) and when this store writes a schema. A dropped `postgres::Statement` closes its
//! server-side statement, so eviction frees the server's memory too.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use postgres::error::SqlState;
use postgres::types::ToSql;
use postgres::{Client, Row, Statement};

/// Statements kept per connection; the least recently used one goes first.
pub(crate) const CAPACITY: usize = 256;

pub(crate) struct StatementCache {
    statements: HashMap<String, Cached>,
    /// Use counter, for least-recently-used eviction.
    tick: u64,
    /// The generation the entries were prepared under.
    seen: u64,
    generation: Arc<AtomicU64>,
}

struct Cached {
    statement: Statement,
    used: u64,
}

impl StatementCache {
    pub(crate) fn new(generation: Arc<AtomicU64>) -> Self {
        let seen = generation.load(Ordering::Acquire);
        Self {
            statements: HashMap::new(),
            tick: 0,
            seen,
            generation,
        }
    }

    /// The cached statement for `sql`, or the one `prepare` makes, which is then cached.
    pub(crate) fn get(
        &mut self,
        sql: &str,
        prepare: impl FnOnce(&str) -> Result<Statement, postgres::Error>,
    ) -> Result<Statement, postgres::Error> {
        let now = self.generation.load(Ordering::Acquire);
        if now != self.seen {
            self.statements.clear();
            self.seen = now;
        }
        self.tick += 1;
        if let Some(c) = self.statements.get_mut(sql) {
            c.used = self.tick;
            return Ok(c.statement.clone());
        }
        let statement = prepare(sql)?;
        if self.statements.len() >= CAPACITY {
            let oldest = self
                .statements
                .iter()
                .min_by_key(|(_, c)| c.used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.statements.remove(&k);
            }
        }
        self.statements.insert(
            sql.to_owned(),
            Cached {
                statement: statement.clone(),
                used: self.tick,
            },
        );
        Ok(statement)
    }

    /// Looks at an error from running a cached statement: a stale plan invalidates every
    /// connection's cache, so the next attempt anywhere prepares afresh. The failed call
    /// still fails; it is not retried, since it may have been inside a transaction the
    /// error aborted.
    pub(crate) fn saw(&mut self, e: &postgres::Error) {
        if e.code() == Some(&SqlState::FEATURE_NOT_SUPPORTED) {
            self.invalidate_all();
        }
    }

    /// Empties this cache and, through the shared generation, every other connection's.
    pub(crate) fn invalidate_all(&mut self) {
        self.seen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.statements.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.statements.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// Whether `sql` is cached, as of this connection's last use.
    #[cfg(test)]
    pub(crate) fn contains(&self, sql: &str) -> bool {
        self.statements.contains_key(sql)
    }
}

/// A pooled connection and its statement cache.
pub(crate) struct Conn {
    pub(crate) client: Client,
    pub(crate) statements: StatementCache,
}

impl Conn {
    pub(crate) fn new(client: Client, generation: Arc<AtomicU64>) -> Self {
        Self {
            client,
            statements: StatementCache::new(generation),
        }
    }

    /// [`Client::query`] through the cache.
    pub(crate) fn query(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, postgres::Error> {
        let client = &mut self.client;
        let statement = self.statements.get(sql, |s| client.prepare(s))?;
        client
            .query(&statement, params)
            .inspect_err(|e| self.statements.saw(e))
    }

    /// [`Client::query_one`] through the cache.
    pub(crate) fn query_one(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, postgres::Error> {
        let client = &mut self.client;
        let statement = self.statements.get(sql, |s| client.prepare(s))?;
        client
            .query_one(&statement, params)
            .inspect_err(|e| self.statements.saw(e))
    }

    /// [`Client::query_opt`] through the cache.
    pub(crate) fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, postgres::Error> {
        let client = &mut self.client;
        let statement = self.statements.get(sql, |s| client.prepare(s))?;
        client
            .query_opt(&statement, params)
            .inspect_err(|e| self.statements.saw(e))
    }
}
