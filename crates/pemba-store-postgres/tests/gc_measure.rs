//! T5.7's measurements (`docs/10-postgres.md` §1.6.5): rebasing batch ids against keeping
//! the map. Builds 100,000 one-tuple write calls directly in SQL (the worst case for map
//! size), then reports the map's size per write call, the time GC takes to rebase them,
//! and the p50 of an engine read paging 500 tuples at `Exact(head - 1)` (the map join)
//! before and after compaction, with head reads (no join) for reference.
//!
//! `#[ignore]`: run by hand with `--ignored --nocapture` against `PEMBA_PG_URL`; the
//! numbers are recorded in `10` §1.6.5.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::{Duration, Instant};

use common::{TestSchema, admin, pg_url};
use pemba_core::model::{Consistency, RelationshipFilter, Revision};
use pemba_core::{Engine, ReadRelationshipsRequest};
use pemba_store_postgres::{PostgresConfig, PostgresStore};

const WRITES: i64 = 100_000;
const PAGE: u32 = 100;
const PAGES: usize = 5;
const ROUNDS: usize = 60;

/// p50 of reading `PAGES` pages of `PAGE` tuples at `c`.
fn paging_p50(engine: &Engine<PostgresStore>, c: Consistency) -> Duration {
    let mut times: Vec<Duration> = (0..ROUNDS)
        .map(|_| {
            let start = Instant::now();
            let mut cursor = None;
            let mut consistency = c;
            for _ in 0..PAGES {
                let resp = engine
                    .read_relationships({
                        let mut api_request = ReadRelationshipsRequest::new(
                            RelationshipFilter::new("doc".parse().unwrap()),
                        );
                        api_request.consistency = consistency;
                        api_request.limit = Some(PAGE);
                        api_request.cursor = cursor.take();
                        api_request
                    })
                    .unwrap();
                assert_eq!(resp.relationships.len(), PAGE as usize);
                consistency = Consistency::Exact(resp.revision);
                cursor = resp.next_cursor;
            }
            start.elapsed()
        })
        .collect();
    times.sort();
    times[times.len() / 2]
}

/// `VACUUM` cannot run inside the implicit transaction of a multi-statement query.
fn vacuum(url: &str, s: &str) {
    let mut c = admin(url);
    for table in ["pemba_relationships", "pemba_revisions"] {
        c.batch_execute(&format!("VACUUM ANALYZE {s}.{table}"))
            .unwrap();
    }
}

fn size(url: &str, relation: &str) -> i64 {
    admin(url)
        .query_one(&format!("SELECT pg_total_relation_size('{relation}')"), &[])
        .unwrap()
        .get(0)
}

#[test]
#[ignore = "a measurement, run by hand; see docs/10-postgres.md §1.6.5"]
fn rebase_against_keeping_the_map() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let s = schema.q();
    let store = PostgresStore::open(
        &url,
        PostgresConfig::default()
            .with_schema(&schema.name)
            .with_auto_migrate(true),
    )
    .unwrap();
    // Write call g created doc:<g> in batch g, mapped to revision g.
    admin(&url)
        .batch_execute(&format!(
            "INSERT INTO {s}.pemba_revisions (batch_id, rev, kind, committed_at) \
               SELECT g, g, 'write', now() FROM generate_series(1, {WRITES}) g; \
             INSERT INTO {s}.pemba_relationships (resource_type, resource_id, relation, \
               subject_type, subject_id, created_batch) \
               SELECT 'doc', lpad(g::text, 7, '0'), 'viewer', 'user', 'u', g \
               FROM generate_series(1, {WRITES}) g; \
             SELECT setval('{s}.pemba_rev_seq', {WRITES}); \
             SELECT setval('{s}.pemba_batch_seq', {WRITES})"
        ))
        .unwrap();
    vacuum(&url, &s);
    let map = format!("{s}.pemba_revisions");
    let map_bytes = size(&url, &map);
    let engine = Engine::builder().store(store).build().unwrap();
    engine
        .write_schema("definition user {}\ndefinition doc {\n    relation viewer: user\n}")
        .unwrap();
    let head = engine.head_revision().unwrap();
    let old = Consistency::Exact(Revision(head.0 - 1));

    let below_before = paging_p50(&engine, old);
    let head_before = paging_p50(&engine, Consistency::Latest);

    let start = Instant::now();
    engine.gc(Revision(head.0 - 1_000)).unwrap();
    let gc_time = start.elapsed();
    vacuum(&url, &s);
    let below_after = paging_p50(&engine, old);
    let head_after = paging_p50(&engine, Consistency::Latest);
    // Rows, not bytes: VACUUM keeps freed space for reuse rather than returning it.
    let map_after: i64 = admin(&url)
        .query_one(&format!("SELECT count(*) FROM {map}"), &[])
        .unwrap()
        .get(0);

    #[allow(clippy::cast_precision_loss)]
    let per_write = map_bytes as f64 / WRITES as f64;
    eprintln!(
        "gc_measure, {WRITES} one-tuple write calls:\n\
         \x20 map: {map_bytes} bytes ({per_write:.0} bytes per write call, indexes included); \
         after GC {map_after} map rows are left\n\
         \x20 GC rebasing {} rows and pruning the map: {gc_time:?}\n\
         \x20 paging {} tuples at Exact(head - 1), p50: before compaction {below_before:?}, \
         after {below_after:?}\n\
         \x20 paging at Latest (head form, no join), p50: before {head_before:?}, after \
         {head_after:?}",
        WRITES - 1_000,
        PAGE as usize * PAGES,
    );
}
