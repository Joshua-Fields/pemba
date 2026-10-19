//! Runs the store conformance suite (`docs/05-storage.md` §9) against `SqliteStore`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU32, Ordering};

use warden_store_sqlite::{SqliteConfig, SqliteStore};

#[test]
fn sqlite_store_conforms() {
    let dir = std::env::temp_dir().join(format!("warden-conformance-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let next = AtomicU32::new(0);
    // Each case drops its store before the next starts, so removing the directory at the
    // end also works on Windows.
    let result = std::panic::catch_unwind(|| {
        warden_testkit::store_conformance::run(|| {
            let n = next.fetch_add(1, Ordering::Relaxed);
            SqliteStore::open(dir.join(format!("case{n}.db")), SqliteConfig::default()).unwrap()
        });
    });
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
