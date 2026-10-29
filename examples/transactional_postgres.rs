//! App data and permissions in one Postgres transaction. See docs/guides/transactional-tutorial.md.
#![forbid(unsafe_code)]

use pemba_core::model::{Consistency, WriteOp, WriteRequest};
use pemba_core::{Engine, Error};
use pemba_store_postgres::postgres::{Client, NoTls};
use pemba_store_postgres::{PgTxn, PostgresConfig, PostgresStore};

/// Execute the tutorial in an existing, empty, disposable schema on a local test server.
///
/// # Errors
/// Returns configuration, database, schema, write or evaluation errors.
pub(crate) fn run(dsn: &str, schema: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = PostgresConfig::default().with_schema(schema);
    PostgresStore::migrate(dsn, &config)?;
    let engine = Engine::builder()
        .store(PostgresStore::open(dsn, config)?)
        .build()?;
    engine.write_schema(
        "definition user {} definition document { relation owner: user permission view = owner }",
    )?;
    // Local test server only. For remote servers use a TLS-capable app connection.
    let mut app = Client::connect(dsn, NoTls)?;
    let documents = format!("\"{}\".documents", schema.replace('"', "\"\""));
    app.batch_execute(&format!(
        "CREATE TABLE IF NOT EXISTS {documents} (id text PRIMARY KEY)"
    ))?;

    let mut tx = app.transaction()?;
    tx.execute(
        &format!("INSERT INTO {documents} (id) VALUES ($1)"),
        &[&"committed"],
    )?;
    let pending = engine.write_in(
        &mut PgTxn::new(&mut tx),
        WriteRequest::new(vec![WriteOp::Touch(
            "document:committed#owner@user:anne".parse()?,
        )]),
    )?;
    assert_eq!(pending.resolved(), None);
    assert!(matches!(
        engine.resolve(&pending),
        Err(Error::RevisionPending)
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

    assert_eq!(pending.resolved(), Some(rev));
    assert_eq!(
        app.query_one(&format!("SELECT id FROM {documents}"), &[])?
            .get::<_, String>(0),
        "committed"
    );

    let mut tx = app.transaction()?;
    tx.execute(
        &format!("INSERT INTO {documents} (id) VALUES ($1)"),
        &[&"rolled_back"],
    )?;
    let lost = engine.write_in(
        &mut PgTxn::new(&mut tx),
        WriteRequest::new(vec![WriteOp::Touch(
            "document:rolled_back#owner@user:anne".parse()?,
        )]),
    )?;
    // Force an app failure AFTER writing the permission: a duplicate primary key.
    assert!(
        tx.execute(
            &format!("INSERT INTO {documents} (id) VALUES ($1)"),
            &[&"rolled_back"]
        )
        .is_err()
    );
    tx.rollback()?;
    assert!(matches!(engine.resolve(&lost), Err(Error::WriteRolledBack)));
    assert_eq!(
        app.query_one(&format!("SELECT count(*) FROM {documents}"), &[])?
            .get::<_, i64>(0),
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
    let dsn = std::env::var("PEMBA_PG_URL")?;
    let schema = std::env::var("PEMBA_TUTORIAL_SCHEMA")?;
    run(&dsn, &schema)
}
