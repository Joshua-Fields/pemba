//! Property 5 (`docs/07-testing.md` §2) for this store: the memory store and
//! `PySqliteStore` give identical results for identical op sequences.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use pemba_store_memory::MemoryStore;
use pemba_store_pysqlite::{PySqliteConfig, PySqliteStore};
use pemba_testkit::generate::{self, StoreOp};
use proptest::prelude::*;

/// A store in a fresh directory, removed on drop (store first).
struct TempStore {
    store: Option<PySqliteStore>,
    dir: PathBuf,
}

impl TempStore {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-pyprop5-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = PySqliteConfig::default();
        config.reader_pool = 1;
        let store = PySqliteStore::open(dir.join("p5.db"), config).unwrap();
        Self {
            store: Some(store),
            dir,
        }
    }
}

impl Drop for TempStore {
    fn drop(&mut self) {
        drop(self.store.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn equivalent(ops: &[StoreOp]) -> Result<(), TestCaseError> {
    let memory = MemoryStore::new();
    let py = TempStore::new();
    pemba_testkit::store_equivalence::equivalent(&memory, py.store.as_ref().unwrap(), ops)
}

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config(64))]

    /// Property 5, quick: 64 cases.
    #[test]
    fn prop5_memory_and_pysqlite_agree(ops in generate::store_ops()) {
        equivalent(&ops)?;
    }
}

proptest! {
    #![proptest_config(config(1000))]

    /// Property 5, ≥ 1 000 cases.
    #[test]
    #[ignore = "slow: 1 000+ cases; run with --ignored"]
    fn prop5_memory_and_pysqlite_agree_full(ops in generate::store_ops()) {
        equivalent(&ops)?;
    }
}
