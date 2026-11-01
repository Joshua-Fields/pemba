//! Read-ahead on real batches (`docs/T10.1-READ-AHEAD.md`): one statement must answer as the
//! single reads do, including where it leaves a speculative read unanswered.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{TestSchema, admin, pg_url};
use pemba_core::model::{Consistency, Relationship, WriteOp, WriteRequest};
use pemba_core::store::{BatchAnswer, BatchRead, ReadStore};
use pemba_core::{CheckRequest, Engine, EngineConfig, Error};
use pemba_store_postgres::{PostgresConfig, PostgresStore};

const SCHEMA: &str = "
definition user {}

definition group {
    relation member: user | group#member
}

definition doc {
    relation viewer: user
    relation shared: user | group#member
    relation parent: doc
    permission view = viewer + shared + parent->view
}
";

fn engine(url: &str, schema: &TestSchema, tuples: &[String]) -> Engine<PostgresStore> {
    let store = PostgresStore::open(
        url,
        PostgresConfig::default()
            .with_schema(&schema.name)
            .with_pool_size(2)
            .with_auto_migrate(true),
    )
    .unwrap();
    let engine = Engine::builder()
        .store(store)
        .config(EngineConfig::default())
        .build()
        .unwrap();
    engine.write_schema(SCHEMA).unwrap();
    write(&engine, tuples);
    engine
}

fn write(engine: &Engine<PostgresStore>, tuples: &[String]) -> pemba_core::model::Revision {
    let ops = tuples
        .iter()
        .map(|t| WriteOp::Touch(t.parse::<Relationship>().unwrap()))
        .collect();
    let mut request = WriteRequest::new(ops);
    request.preconditions = Vec::new();
    engine.write(request).unwrap()
}

fn check(resource: &str, subject: &str) -> CheckRequest {
    CheckRequest::new(
        resource.parse().unwrap(),
        "view".parse().unwrap(),
        subject.parse().unwrap(),
    )
}

#[test]
fn a_speculative_read_over_its_row_cap_is_read_in_full_when_needed() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    // Over the 64 rows a hint may load, with the only grant in the last row by store order.
    let mut tuples: Vec<String> = (0..90)
        .map(|i| format!("doc:d#shared@group:g{i:03}#member"))
        .collect();
    tuples.push("group:g089#member@user:anne".to_owned());
    let engine = engine(&url, &schema, &tuples);
    assert!(engine.check(check("doc:d", "user:anne")).unwrap().allowed);
    assert!(!engine.check(check("doc:d", "user:bob")).unwrap().allowed);

    // The store itself: uncapped when asked for, unanswered when only hinted.
    let snapshot = engine.store().snapshot().unwrap();
    let rev = snapshot.head_revision().unwrap();
    let read = BatchRead::SubjectSets {
        resource: "doc:d".parse().unwrap(),
        relation: "shared".parse().unwrap(),
    };
    let asked = snapshot
        .read_batch(rev, std::slice::from_ref(&read))
        .unwrap();
    let Some(BatchAnswer::SubjectSets(sets)) = &asked[0] else {
        panic!("{asked:?}");
    };
    assert_eq!(sets.len(), 90);
    let hinted = snapshot
        .read_batch(
            rev,
            &[
                BatchRead::Exists("doc:x#viewer@user:a".parse().unwrap()),
                read,
            ],
        )
        .unwrap();
    assert_eq!(hinted[0], Some(BatchAnswer::Exists(false)));
    assert_eq!(hinted[1], None);
}

#[test]
fn a_corrupt_row_matters_only_if_evaluation_reads_it() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = engine(&url, &schema, &["doc:d#viewer@user:anne".to_owned()]);
    // A subject set whose stored type no type name can be, on a relation `view` reads after
    // `viewer`, so every check here hints it.
    admin(&url)
        .batch_execute(&format!(
            "INSERT INTO \"{0}\".pemba_relationships \
             (resource_type, resource_id, relation, subject_type, subject_id, subject_relation, \
              created_batch) VALUES ('doc', 'd', 'shared', 'NOT A TYPE', 'x', 'member', 0)",
            schema.name
        ))
        .unwrap();
    // The direct grant decides first: as before, the bad row is never needed.
    assert!(engine.check(check("doc:d", "user:anne")).unwrap().allowed);
    // Bob must read it: the error surfaces, not a deny.
    let got = engine.check(check("doc:d", "user:bob"));
    assert!(matches!(got, Err(Error::Store(_))), "{got:?}");
}

#[test]
fn below_head_a_batch_reads_at_the_revision_asked_for() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = engine(
        &url,
        &schema,
        &[
            "doc:child#parent@doc:top".to_owned(),
            "doc:top#viewer@user:anne".to_owned(),
        ],
    );
    let before = engine.head_revision().unwrap();
    let mut remove = WriteRequest::new(vec![WriteOp::Delete(
        "doc:top#viewer@user:anne".parse().unwrap(),
    )]);
    remove.preconditions = Vec::new();
    engine.write(remove).unwrap();
    assert!(
        !engine
            .check(check("doc:child", "user:anne"))
            .unwrap()
            .allowed
    );
    let mut at = check("doc:child", "user:anne");
    at.consistency = Consistency::Exact(before);
    assert!(engine.check(at).unwrap().allowed);
}

#[test]
fn a_failed_statement_is_an_error_and_the_connection_recovers() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = engine(&url, &schema, &["doc:d#viewer@user:anne".to_owned()]);
    assert!(engine.check(check("doc:d", "user:anne")).unwrap().allowed);
    // Every batch statement now fails on the server, as a mid-request failure would.
    let mut admin = admin(&url);
    admin
        .batch_execute(&format!(
            "ALTER TABLE \"{0}\".pemba_relationships RENAME TO moved",
            schema.name
        ))
        .unwrap();
    let got = engine.check(check("doc:d", "user:anne"));
    assert!(matches!(got, Err(Error::Store(_))), "{got:?}");
    admin
        .batch_execute(&format!(
            "ALTER TABLE \"{0}\".moved RENAME TO pemba_relationships",
            schema.name
        ))
        .unwrap();
    assert!(engine.check(check("doc:d", "user:anne")).unwrap().allowed);
}
