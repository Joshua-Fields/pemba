//! Property 5 on Postgres (`docs/10-postgres.md` §4 item 2): memory ≡ SQLite ≡ Postgres
//! for identical op sequences, every outcome and every read at every revision.
//!
//! Sequential ops never fail at commit, so revision numbers must match exactly; a run
//! that left a gap in the Postgres numbering fails here as a test bug, not as a false
//! difference. 32 cases, or `PROPTEST_CASES`, because every read is a round trip.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use common::{TestSchema, admin, pg_url};
use pemba_store_memory::MemoryStore;
use pemba_store_postgres::{PostgresConfig, PostgresStore};
use pemba_store_sqlite::{SqliteConfig, SqliteStore};
use pemba_testkit::{generate, store_equivalence};
use proptest::prelude::*;
use proptest::test_runner::TestRunner;

/// A SQLite store in a fresh directory, removed on drop (store first).
struct TempSqlite {
    store: Option<SqliteStore>,
    dir: PathBuf,
}

impl TempSqlite {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-pg-prop5-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = SqliteConfig::default();
        config.reader_pool = 1;
        let store = SqliteStore::open(dir.join("p5.db"), config).unwrap();
        Self {
            store: Some(store),
            dir,
        }
    }
}

impl Drop for TempSqlite {
    fn drop(&mut self) {
        drop(self.store.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

#[test]
fn memory_sqlite_and_postgres_are_equivalent() {
    let Some(url) = pg_url() else { return };
    let mut runner = TestRunner::new(config());
    runner
        .run(&generate::store_ops(), |ops| {
            let schema = TestSchema::new(&url);
            let pg = PostgresStore::open(
                &url,
                PostgresConfig::default()
                    .with_schema(&schema.name)
                    .with_pool_size(2)
                    .with_auto_migrate(true),
            )
            .unwrap();
            let memory = MemoryStore::new();
            let sqlite = TempSqlite::new();
            store_equivalence::equivalent(&[&memory, sqlite.store.as_ref().unwrap(), &pg], &ops)?;

            // Every revision drawn committed: the sequence's last value is head. (GC prunes
            // the map, so counting its rows would not do.)
            let row = admin(&url)
                .query_one(
                    &format!(
                        "SELECT s.last_value, s.is_called, \
                         (SELECT max(rev) FROM {q}.pemba_revisions) FROM {q}.pemba_rev_seq s",
                        q = schema.q()
                    ),
                    &[],
                )
                .unwrap();
            let (last, called, head): (i64, bool, i64) = (row.get(0), row.get(1), row.get(2));
            let drawn = if called { last } else { 0 };
            prop_assert_eq!(drawn, head, "a sequential run left a gap in the revisions");
            Ok(())
        })
        .unwrap();
}
