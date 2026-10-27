//! T5.7's property: GC with compaction never changes an answer (`docs/10-postgres.md`
//! §1.6.5). For random op sequences, every read at every revision from the new horizon to
//! head is the same before and after `gc`, on the store (map join) and through a snapshot
//! (head form at head); afterwards no row below the horizon keeps its batch, and the map
//! holds nothing below the horizon but batch 0 and the batches of schema versions.
//! 16 cases, or `PROPTEST_CASES`. Needs `PEMBA_PG_URL`; reports "not run" without it
//! unless `PEMBA_PG_REQUIRED` is set.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{TestSchema, admin, pg_url};
use pemba_core::model::{RelationshipFilter, Revision};
use pemba_core::store::{ReadStore, WritableStore};
use pemba_store_postgres::{PostgresConfig, PostgresStore};
use pemba_testkit::generate::{self, StoreOp};
use pemba_testkit::store_equivalence;
use proptest::prelude::*;
use proptest::test_runner::TestRunner;

fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// Every read at `revs`, on the store and through a fresh snapshot.
fn answers(s: &PostgresStore, revs: &[Revision]) -> Vec<Vec<String>> {
    revs.iter()
        .flat_map(|&r| {
            let snapshot = s.snapshot().unwrap();
            // `reads` takes a `WritableStore`; through the snapshot, read every tuple of
            // the generator's types and the counts.
            let through_snapshot = ["doc", "folder"]
                .iter()
                .map(|t| {
                    let f = RelationshipFilter::new(t.parse().unwrap());
                    format!("{:?}", snapshot.read_relationships(r, &f, u32::MAX, None))
                })
                .chain([format!("{:?}", snapshot.count_by_relation(r))])
                .collect();
            [store_equivalence::reads(s, r), through_snapshot]
        })
        .collect()
}

fn count(url: &str, schema: &TestSchema, sql: &str) -> i64 {
    admin(url)
        .query_one(&sql.replace("{s}", &schema.q()), &[])
        .unwrap()
        .get(0)
}

#[test]
fn gc_and_compaction_never_change_an_answer() {
    let Some(url) = pg_url() else { return };
    let mut runner = TestRunner::new(config());
    let strategy = (generate::store_ops(), any::<u8>());
    runner
        .run(&strategy, |(ops, fraction)| {
            let schema = TestSchema::new(&url);
            let store = PostgresStore::open(
                &url,
                PostgresConfig::default()
                    .with_schema(&schema.name)
                    .with_pool_size(2)
                    .with_auto_migrate(true),
            )
            .unwrap();
            for op in &ops {
                match op {
                    StoreOp::Write(req) => drop(store.write(req, None)),
                    StoreOp::Schema(n) => {
                        let source = format!("schema {n}");
                        store
                            .write_schema(&source, &format!("{n:064x}"), &|_| Ok(()))
                            .unwrap();
                    }
                    StoreOp::Gc(f) => {
                        let head = store.head_revision().unwrap();
                        store.gc(StoreOp::gc_revision(*f, head)).unwrap();
                    }
                }
            }
            let head = store.head_revision().unwrap();
            let keep_from = StoreOp::gc_revision(fraction, head).max(store.gc_horizon().unwrap());
            let readable: Vec<Revision> = (keep_from.0..=head.0).map(Revision).collect();
            let schema_before = store.current_schema().unwrap();
            let before = answers(&store, &readable);

            store.gc(keep_from).unwrap();
            prop_assert_eq!(answers(&store, &readable), before);
            prop_assert_eq!(store.current_schema().unwrap(), schema_before);
            prop_assert_eq!(store.head_revision().unwrap(), head);

            let h = i64::try_from(keep_from.0).unwrap();
            prop_assert_eq!(
                count(
                    &url,
                    &schema,
                    &format!(
                        "SELECT count(*) FROM {{s}}.pemba_relationships r \
                         JOIN {{s}}.pemba_revisions m ON m.batch_id = r.created_batch \
                         WHERE r.created_batch <> 0 AND m.rev <= {h}"
                    )
                ),
                0,
                "a row created at or below the horizon kept its batch"
            );
            prop_assert_eq!(
                count(
                    &url,
                    &schema,
                    &format!(
                        "SELECT count(*) FROM {{s}}.pemba_revisions m WHERE m.rev < {h} \
                         AND m.batch_id <> 0 AND NOT EXISTS \
                         (SELECT 1 FROM {{s}}.pemba_schema s WHERE s.created_batch = m.batch_id)"
                    )
                ),
                0,
                "a map row below the horizon survived"
            );
            prop_assert_eq!(
                count(&url, &schema, "SELECT count(*) FROM {s}.pemba_pending"),
                0
            );
            drop(store);
            Ok(())
        })
        .unwrap();
}
