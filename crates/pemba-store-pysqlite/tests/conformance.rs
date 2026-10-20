//! Runs the store conformance suite (`docs/05-storage.md` §9) against `PySqliteStore`,
//! in an embedded interpreter.
//!
//! The test binary also links `pemba-testkit`, which links the bundled-SQLite store, so it
//! holds two SQLite copies; they never open the same file, which is what OQ-032 forbids.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU32, Ordering};

use pemba_store_pysqlite::{PySqliteConfig, PySqliteStore};

#[test]
fn pysqlite_store_conforms() {
    let dir = std::env::temp_dir().join(format!("pemba-pyconformance-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let next = AtomicU32::new(0);
    // Each case drops its store (closing its connections) before the next starts.
    let result = std::panic::catch_unwind(|| {
        pemba_testkit::store_conformance::run(|| {
            let n = next.fetch_add(1, Ordering::Relaxed);
            PySqliteStore::open(dir.join(format!("case{n}.db")), PySqliteConfig::default()).unwrap()
        });
    });
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
