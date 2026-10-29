//! App data and permissions commit or roll back together. See docs/guides/transactional-tutorial.md.
#![forbid(unsafe_code)]

use pemba_core::model::{Consistency, WriteOp, WriteRequest};
use pemba_core::{Engine, Error};
use pemba_store_sqlite::rusqlite::{Connection, TransactionBehavior};
use pemba_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn};
use std::path::Path;

/// Execute the tutorial against a fresh file-backed application database.
///
/// # Errors
/// Returns database, schema, write or evaluation errors; none become a denial.
pub(crate) fn run(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder()
        .store(SqliteStore::open(path, SqliteConfig::default())?)
        .build()?;
    engine.write_schema(
        "definition user {} definition document { relation owner: user permission view = owner }",
    )?;
    let mut app = Connection::open(path)?;
    app.execute_batch("CREATE TABLE documents (id TEXT PRIMARY KEY)")?;

    let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("INSERT INTO documents (id) VALUES (?1)", ["committed"])?;
    let pending = engine.write_in(
        &mut SqliteTxn::new(&tx),
        WriteRequest::new(vec![WriteOp::Touch(
            "document:committed#owner@user:anne".parse()?,
        )]),
    )?;
    let known = engine.resolve(&pending)?;
    assert_eq!(pending.resolved(), Some(known));
    // Knowing the number does not make uncommitted data readable.
    assert!(matches!(
        engine.check_str(
            "document:committed",
            "view",
            "user:anne",
            Consistency::AtLeast(known)
        ),
        Err(Error::RevisionNotYetAvailable { .. })
    ));
    assert!(!engine.check_str(
        "document:committed",
        "view",
        "user:anne",
        Consistency::Latest
    )?);
    tx.commit()?;

    // Portable code resolves only after a successful caller-owned commit.
    let rev = engine.resolve(&pending)?;
    assert!(engine.check_str(
        "document:committed",
        "view",
        "user:anne",
        Consistency::AtLeast(rev)
    )?);
    assert!(!engine.check_str(
        "document:committed",
        "view",
        "user:bob",
        Consistency::AtLeast(rev)
    )?);

    assert_eq!(
        app.query_row("SELECT id FROM documents", [], |row| row
            .get::<_, String>(0))?,
        "committed"
    );

    let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("INSERT INTO documents (id) VALUES (?1)", ["rolled_back"])?;
    let lost = engine.write_in(
        &mut SqliteTxn::new(&tx),
        WriteRequest::new(vec![WriteOp::Touch(
            "document:rolled_back#owner@user:anne".parse()?,
        )]),
    )?;
    // Force an app failure AFTER writing the permission: a duplicate primary key.
    assert!(
        tx.execute("INSERT INTO documents (id) VALUES (?1)", ["rolled_back"])
            .is_err()
    );
    tx.rollback()?;
    // SQLite cannot detect rollback from a token; its number is void and may be reused.
    drop(lost);
    assert_eq!(
        app.query_row("SELECT count(*) FROM documents", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    assert!(!engine.check_str(
        "document:rolled_back",
        "view",
        "user:anne",
        Consistency::Latest
    )?);
    Ok(())
}

#[allow(dead_code)] // The integration test imports this same file and calls run().
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: transactional_sqlite <fresh-app.db>")?;
    run(Path::new(&path))
}
