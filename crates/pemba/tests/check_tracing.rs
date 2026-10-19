//! T1.3: tracing spans on `check` and `check_bulk` carry the `EvalStats` fields
//! (`docs/04-evaluation.md` §1).
//!
//! A test binary of its own: `tracing` caches per-callsite interest process-wide, and a test
//! running without a subscriber on another thread can cache "never" for the engine's spans
//! before the scoped [`Recorder`] is installed, which made these assertions flaky.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use pemba::model::{Consistency, Relationship, WriteOp, WriteRequest};
use pemba::{CheckItem, CheckRequest, Engine, MemoryStore};

const SCHEMA: &str = "\
definition user {}

definition group {
    relation member: user | group#member
}

definition document {
    relation owner: user
    relation viewer: user | group#member
    permission view = viewer + owner
}
";

fn engine(rels: &[&str]) -> Engine<MemoryStore> {
    let e = Engine::builder().store(MemoryStore::new()).build().unwrap();
    e.write_schema(SCHEMA).unwrap();
    e.write(WriteRequest {
        ops: rels
            .iter()
            .map(|r| WriteOp::Touch(r.parse::<Relationship>().unwrap()))
            .collect(),
        preconditions: Vec::new(),
    })
    .unwrap();
    e
}

fn request(s: &str, consistency: Consistency) -> CheckRequest {
    let r: Relationship = s.parse().unwrap();
    CheckRequest {
        resource: r.resource,
        permission: r.relation,
        subject: r.subject,
        consistency,
    }
}

fn item(s: &str) -> CheckItem {
    let r: Relationship = s.parse().unwrap();
    CheckItem {
        resource: r.resource,
        permission: r.relation,
        subject: r.subject,
    }
}

type Fields = Vec<(String, String)>;

/// Records every span's name and fields, including values recorded after creation.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(String, Fields)>>>);

struct FieldVisitor<'a>(&'a mut Fields);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().to_owned(), format!("{value:?}")));
    }
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut spans = self.0.lock().unwrap();
        let mut fields = Vec::new();
        attrs.record(&mut FieldVisitor(&mut fields));
        spans.push((attrs.metadata().name().to_owned(), fields));
        tracing::span::Id::from_u64(spans.len() as u64)
    }
    fn record(&self, id: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut spans = self.0.lock().unwrap();
        let i = usize::try_from(id.into_u64() - 1).unwrap();
        values.record(&mut FieldVisitor(&mut spans[i].1));
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn field<'a>(fields: &'a Fields, name: &str) -> &'a str {
    fields.iter().rev().find(|(n, _)| n == name).map_or_else(
        || panic!("span has no field {name}: {fields:?}"),
        |(_, v)| v.as_str(),
    )
}

#[test]
fn check_emits_span_with_stats_fields() {
    let e = engine(&[
        "document:d#viewer@group:eng#member",
        "group:eng#member@user:anne",
    ]);
    let rec = Recorder::default();
    let resp = tracing::subscriber::with_default(rec.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        e.check(request("document:d#view@user:anne", Consistency::Latest))
            .unwrap()
    });
    let spans = rec.0.lock().unwrap();
    let (_, fields) = spans
        .iter()
        .find(|(name, _)| name == "check")
        .expect("a `check` span");
    assert_eq!(field(fields, "resource"), "document:d");
    assert_eq!(field(fields, "permission"), "view");
    assert_eq!(field(fields, "subject"), "user:anne");
    assert_eq!(field(fields, "allowed"), "true");
    assert_eq!(field(fields, "revision"), resp.revision.0.to_string());
    let s = resp.stats;
    assert_eq!(field(fields, "store_reads"), s.store_reads.to_string());
    assert_eq!(field(fields, "tuples_read"), s.tuples_read.to_string());
    assert_eq!(field(fields, "memo_hits"), s.memo_hits.to_string());
    assert_eq!(
        field(fields, "max_depth_reached"),
        s.max_depth_reached.to_string()
    );
    field(fields, "elapsed_us");
}

#[test]
fn check_bulk_emits_span_with_stats_fields() {
    let e = engine(&["document:d#owner@user:anne"]);
    let rec = Recorder::default();
    tracing::subscriber::with_default(rec.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        e.check_bulk(
            vec![
                item("document:d#owner@user:anne"),
                item("document:d#owner@user:anne"),
            ],
            Consistency::Latest,
        )
        .unwrap()
    });
    let spans = rec.0.lock().unwrap();
    let (_, fields) = spans
        .iter()
        .find(|(name, _)| name == "check_bulk")
        .expect("a `check_bulk` span");
    assert_eq!(field(fields, "items"), "2");
    assert_eq!(field(fields, "store_reads"), "1");
    assert_eq!(field(fields, "memo_hits"), "1");
    field(fields, "tuples_read");
    field(fields, "max_depth_reached");
    field(fields, "elapsed_us");
}
