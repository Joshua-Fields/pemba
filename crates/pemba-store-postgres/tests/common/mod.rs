//! Shared set-up for the Postgres integration tests (`docs/10-postgres.md` §4).
//!
//! Tests read `PEMBA_PG_URL` (a `postgres://` URL to a server the tests may create schemas
//! and databases on) and report "not run" without it, unless `PEMBA_PG_REQUIRED` is set
//! (CI sets it). Each test makes its own schema, `pemba_t_<random>`, so tests run in
//! parallel against one server; the schema is dropped when the test passes and left for
//! inspection when it fails.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pemba_store_postgres::postgres::{Client, NoTls};

/// The server URL, or `None` (after saying so) when the tests should not run.
pub(crate) fn pg_url() -> Option<String> {
    let url = std::env::var("PEMBA_PG_URL").ok().filter(|u| !u.is_empty());
    if url.is_none() {
        assert!(
            std::env::var_os("PEMBA_PG_REQUIRED").is_none(),
            "PEMBA_PG_REQUIRED is set but PEMBA_PG_URL is not"
        );
        eprintln!("not run: set PEMBA_PG_URL (docs/10-postgres.md §4)");
    }
    url
}

pub(crate) fn admin(url: &str) -> Client {
    Client::connect(url, NoTls).unwrap()
}

/// A name no other test (or earlier run) uses.
pub(crate) fn unique(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{:x}{nanos:08x}{n:x}", std::process::id())
}

pub(crate) fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `url` with its query string extended by `key=value` (value percent-encoded).
pub(crate) fn with_param(url: &str, key: &str, value: &str) -> String {
    let encoded: String = value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{key}={encoded}")
}

/// `url` pointing at database `db` instead.
pub(crate) fn with_database(url: &str, db: &str) -> String {
    let (base, query) = url
        .split_once('?')
        .map_or((url, None), |(b, q)| (b, Some(q)));
    let scheme_end = base.find("://").map_or(0, |i| i + 3);
    let path_start = base[scheme_end..]
        .find('/')
        .map_or(base.len(), |i| scheme_end + i);
    let mut out = format!("{}/{db}", &base[..path_start]);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    out
}

/// A fresh schema, dropped when the test passes.
pub(crate) struct TestSchema {
    pub(crate) url: String,
    pub(crate) name: String,
}

impl TestSchema {
    pub(crate) fn new(url: &str) -> Self {
        Self::named(url, &unique("pemba_t_"))
    }

    pub(crate) fn named(url: &str, name: &str) -> Self {
        admin(url)
            .batch_execute(&format!("CREATE SCHEMA {}", quote(name)))
            .unwrap();
        Self {
            url: url.to_owned(),
            name: name.to_owned(),
        }
    }

    pub(crate) fn q(&self) -> String {
        quote(&self.name)
    }
}

impl Drop for TestSchema {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = admin(&self.url).batch_execute(&format!("DROP SCHEMA {} CASCADE", self.q()));
        }
    }
}
