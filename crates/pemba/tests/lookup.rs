//! T2.4: `lookup_resources` (`docs/04-evaluation.md` §5).
//!
//! One test per phase 1 rule (seeds and expansions) and per kind of candidate phase 2 must
//! drop. Where a test claims a candidate was generated and then filtered out, it reads the
//! `candidates` field of the `lookup_resources` span, so every lookup here runs under a
//! span recorder (a test binary of its own for that reason; see `check_tracing.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use pemba::model::{
    Consistency, ObjectId, Relationship, Revision, SubjectRef, WriteOp, WriteRequest,
};
use pemba::store::WritableStore;
use pemba::{
    Engine, EngineConfig, Error, LookupResourcesRequest, LookupResourcesResponse, MemoryStore,
    SqliteConfig, SqliteStore,
};

const SCHEMA: &str = "\
definition user {}

definition group {
    relation member: user | group#member
}

definition folder {
    relation parent: folder
    relation viewer: user | group#member
    permission view = viewer + parent->view
}

definition document {
    relation parent: folder | folder#viewer
    relation owner: user
    relation editor: user
    relation viewer: user | user:* | group#member
    relation banned: user
    relation approver: group
    permission edit = editor + owner
    permission view = viewer + edit + parent->view
    permission both = viewer & editor
    permission view_not_banned = viewer - banned
    permission approver_all = approver.all(member)
}
";

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels
            .iter()
            .map(|r| WriteOp::Touch(r.parse::<Relationship>().unwrap()))
            .collect(),
        preconditions: Vec::new(),
    }
}

fn engine_on<S: WritableStore>(store: S, config: EngineConfig, rels: &[&str]) -> Engine<S> {
    let e = Engine::builder()
        .store(store)
        .config(config)
        .build()
        .unwrap();
    e.write_schema(SCHEMA).unwrap();
    if !rels.is_empty() {
        e.write(touch(rels)).unwrap();
    }
    e
}

fn engine(rels: &[&str]) -> Engine<MemoryStore> {
    engine_on(MemoryStore::new(), EngineConfig::default(), rels)
}

/// `"document#view@user:anne"` → a first-page request at `Latest`.
fn request(query: &str) -> LookupResourcesRequest {
    let (resource_type, rest) = query.split_once('#').unwrap();
    let (permission, subject) = rest.split_once('@').unwrap();
    LookupResourcesRequest::new(
        resource_type.parse().unwrap(),
        permission.parse().unwrap(),
        subject.parse::<SubjectRef>().unwrap(),
    )
}

/// One page, with the span's `candidates` count.
fn page<S: WritableStore>(
    e: &Engine<S>,
    req: LookupResourcesRequest,
) -> (Result<LookupResourcesResponse, Error>, Option<usize>) {
    let rec = Recorder::default();
    let result = tracing::subscriber::with_default(rec.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        e.lookup_resources(req)
    });
    let candidates = rec.field("candidates").map(|v| v.parse().unwrap());
    (result, candidates)
}

/// Every page concatenated, following cursors at the first page's revision.
fn all<S: WritableStore>(e: &Engine<S>, query: &str, limit: Option<u32>) -> Vec<String> {
    let mut req = LookupResourcesRequest {
        limit,
        ..request(query)
    };
    let mut ids = Vec::new();
    loop {
        let resp = e.lookup_resources(req.clone()).unwrap();
        ids.extend(resp.resource_ids.iter().map(ToString::to_string));
        match resp.next_cursor {
            Some(c) => {
                req.cursor = Some(c);
                req.consistency = Consistency::Exact(resp.revision);
            }
            None => return ids,
        }
    }
}

/// The answer, checked to be the same for every page size.
fn lookup<S: WritableStore>(e: &Engine<S>, query: &str) -> Vec<String> {
    let whole = all(e, query, None);
    for limit in [1, 2, 3] {
        assert_eq!(
            all(e, query, Some(limit)),
            whole,
            "{query} with limit {limit}"
        );
    }
    whole
}

/// The answer and how many candidates phase 1 produced, in one page.
fn lookup_counting(e: &Engine<MemoryStore>, query: &str) -> (Vec<String>, usize) {
    let (resp, candidates) = page(e, request(query));
    let resp = resp.unwrap();
    assert_eq!(resp.next_cursor, None);
    (
        resp.resource_ids.iter().map(ToString::to_string).collect(),
        candidates.expect("a `candidates` field"),
    )
}

// ---- phase 1: seeds ----

#[test]
fn object_subject_seeds_from_its_tuples() {
    let e = engine(&[
        "document:a#viewer@user:anne",
        "document:b#viewer@user:bob",
        "document:c#editor@user:anne",
    ]);
    assert_eq!(lookup(&e, "document#viewer@user:anne"), ["a"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["a", "c"]);
    assert_eq!(lookup(&e, "document#view@user:carol"), Vec::<String>::new());
}

#[test]
fn wildcard_tuples_seed_every_object_of_the_type() {
    let e = engine(&[
        "document:public#viewer@user:*",
        "document:a#viewer@user:anne",
    ]);
    // `nobody` appears in no tuple.
    assert_eq!(lookup(&e, "document#view@user:nobody"), ["public"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["a", "public"]);
    // Wildcards match objects only, never subject sets.
    assert_eq!(
        lookup(&e, "document#view@group:eng#member"),
        Vec::<String>::new()
    );
}

#[test]
fn subject_set_seeds_itself_by_self_membership() {
    // No tuple mentions `group:lonely` or `document:z`.
    let e = engine(&["document:a#viewer@user:anne"]);
    assert_eq!(lookup(&e, "group#member@group:lonely#member"), ["lonely"]);
    // ... and expands from there: same-object use (viewer → view).
    assert_eq!(lookup(&e, "document#view@document:z#viewer"), ["z"]);
    assert_eq!(lookup(&e, "document#viewer@document:z#viewer"), ["z"]);
    // Self membership is per member: `z#viewer` is not in `S(z, editor)`.
    assert_eq!(
        lookup(&e, "document#editor@document:z#viewer"),
        Vec::<String>::new()
    );
}

#[test]
fn subject_set_seeds_from_tuples_naming_it_exactly() {
    let e = engine(&[
        "document:b#viewer@group:eng#member",
        "group:eng#member@user:anne",
    ]);
    assert_eq!(lookup(&e, "document#view@group:eng#member"), ["b"]);
    assert_eq!(lookup(&e, "group#member@group:eng#member"), ["eng"]);
}

#[test]
fn subject_set_self_membership_feeds_arrows() {
    // S(document:y, view) ⊇ S(folder:x, view) ∋ folder:x#view.
    let e = engine(&["document:y#parent@folder:x"]);
    assert_eq!(lookup(&e, "document#view@folder:x#view"), ["y"]);
    assert_eq!(lookup(&e, "folder#view@folder:x#view"), ["x"]);
}

// ---- phase 1: expansion rules ----

#[test]
fn rule1_same_object_use() {
    // owner → edit → view: two steps of `used_by` on one object.
    let e = engine(&["document:a#owner@user:anne"]);
    assert_eq!(lookup(&e, "document#edit@user:anne"), ["a"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["a"]);
    assert_eq!(
        lookup(&e, "document#viewer@user:anne"),
        Vec::<String>::new()
    );
}

#[test]
fn rule2_subject_set_use() {
    // Nested groups: anne ∈ eng ⊂ all; documents grant to each.
    let e = engine(&[
        "group:eng#member@user:anne",
        "group:all#member@group:eng#member",
        "document:b#viewer@group:eng#member",
        "document:c#viewer@group:all#member",
        "document:d#viewer@group:other#member",
    ]);
    assert_eq!(lookup(&e, "group#member@user:anne"), ["all", "eng"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["b", "c"]);
}

#[test]
fn rule3_arrow_use_with_any_subject_relation() {
    let e = engine(&[
        "folder:root#viewer@user:anne",
        "folder:sub#parent@folder:root",
        // Plain object on the tupleset.
        "document:d#parent@folder:sub",
        // A subject set on the tupleset: the arrow ignores its relation and walks
        // `folder:root`, so phase 1 must read the tupleset with any subject relation.
        "document:e#parent@folder:root#viewer",
        "document:f#parent@folder:elsewhere",
    ]);
    assert_eq!(lookup(&e, "folder#view@user:anne"), ["root", "sub"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["d", "e"]);
}

// ---- phase 2: over-approximate candidates are filtered out ----

#[test]
fn intersection_candidates_are_filtered() {
    let e = engine(&[
        "document:only_viewer#viewer@user:anne",
        "document:only_editor#editor@user:anne",
        "document:both#viewer@user:anne",
        "document:both#editor@user:anne",
    ]);
    let (ids, candidates) = lookup_counting(&e, "document#both@user:anne");
    assert_eq!(ids, ["both"]);
    assert_eq!(candidates, 3);
}

#[test]
fn exclusion_candidates_are_filtered() {
    let e = engine(&[
        "document:ok#viewer@user:anne",
        "document:banned#viewer@user:anne",
        "document:banned#banned@user:anne",
        "document:banned_only#banned@user:anne",
    ]);
    let (ids, candidates) = lookup_counting(&e, "document#view_not_banned@user:anne");
    assert_eq!(ids, ["ok"]);
    // The subtracted side is a union branch in phase 1 too.
    assert_eq!(candidates, 3);
}

#[test]
fn all_arrow_candidates_are_filtered() {
    let e = engine(&[
        "group:g1#member@user:anne",
        "group:g2#member@user:bob",
        "group:g3#member@user:anne",
        "document:partial#approver@group:g1",
        "document:partial#approver@group:g2",
        "document:full#approver@group:g1",
        "document:full#approver@group:g3",
    ]);
    let (ids, candidates) = lookup_counting(&e, "document#approver_all@user:anne");
    assert_eq!(ids, ["full"]);
    assert_eq!(candidates, 2);
}

// ---- cycles ----

#[test]
fn nested_groups_that_contain_each_other() {
    let e = engine(&[
        "group:a#member@group:b#member",
        "group:b#member@group:c#member",
        "group:c#member@group:a#member",
        "group:b#member@user:anne",
        "group:x#member@group:y#member",
        "group:y#member@group:x#member",
        "document:d#viewer@group:a#member",
        "document:e#viewer@group:x#member",
    ]);
    assert_eq!(lookup(&e, "group#member@user:anne"), ["a", "b", "c"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["d"]);
    assert_eq!(lookup(&e, "group#member@user:bob"), Vec::<String>::new());
    assert_eq!(lookup(&e, "group#member@group:x#member"), ["x", "y"]);
    assert_eq!(lookup(&e, "document#view@group:y#member"), ["e"]);
}

#[test]
fn cyclic_folders() {
    let e = engine(&[
        "folder:a#parent@folder:b",
        "folder:b#parent@folder:a",
        "folder:a#viewer@user:anne",
        "document:d#parent@folder:b",
    ]);
    assert_eq!(lookup(&e, "folder#view@user:anne"), ["a", "b"]);
    assert_eq!(lookup(&e, "document#view@user:anne"), ["d"]);
}

// ---- pagination and limits ----

#[test]
fn pages_follow_the_cursor_in_byte_order() {
    let e = engine(&[
        "document:b#viewer@user:anne",
        "document:a#viewer@user:anne",
        "document:B#viewer@user:anne",
        "document:c#banned@user:anne",
    ]);
    let mut req = LookupResourcesRequest {
        limit: Some(2),
        ..request("document#view_not_banned@user:anne")
    };
    let first = e.lookup_resources(req.clone()).unwrap();
    assert_eq!(first.resource_ids, ids(&["B", "a"]));
    assert_eq!(first.next_cursor, Some("a".parse().unwrap()));
    req.cursor = first.next_cursor;
    req.consistency = Consistency::Exact(first.revision);
    req.limit = Some(1);
    // `c` remains a candidate after `b`, so the page has a cursor; the next is empty.
    let second = e.lookup_resources(req.clone()).unwrap();
    assert_eq!(second.resource_ids, ids(&["b"]));
    assert_eq!(second.next_cursor, Some("b".parse().unwrap()));
    req.cursor = second.next_cursor;
    let third = e.lookup_resources(req).unwrap();
    assert_eq!(third.resource_ids, ids(&[]));
    assert_eq!(third.next_cursor, None);
    // A cursor need not be a result.
    let from_middle = e
        .lookup_resources(LookupResourcesRequest {
            cursor: Some("a0".parse().unwrap()),
            ..request("document#view@user:anne")
        })
        .unwrap();
    assert_eq!(from_middle.resource_ids, ids(&["b"]));
}

fn ids(v: &[&str]) -> Vec<ObjectId> {
    v.iter().map(|s| s.parse().unwrap()).collect()
}

#[test]
fn limit_must_be_between_one_and_max_lookup_results() {
    let e = engine_on(
        MemoryStore::new(),
        EngineConfig {
            max_lookup_results: 2,
            ..EngineConfig::default()
        },
        &[
            "document:a#viewer@user:anne",
            "document:b#viewer@user:anne",
            "document:c#viewer@user:anne",
        ],
    );
    for limit in [0, 3] {
        let err = e
            .lookup_resources(LookupResourcesRequest {
                limit: Some(limit),
                ..request("document#view@user:anne")
            })
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    }
    // `None` is the maximum.
    let resp = e
        .lookup_resources(request("document#view@user:anne"))
        .unwrap();
    assert_eq!(resp.resource_ids, ids(&["a", "b"]));
}

#[test]
fn lookup_too_large_past_max_lookup_nodes() {
    let rels: Vec<String> = (0..10)
        .map(|i| format!("document:d{i}#viewer@user:anne"))
        .collect();
    let rels: Vec<&str> = rels.iter().map(String::as_str).collect();
    let config = |max_lookup_nodes| EngineConfig {
        max_lookup_nodes,
        ..EngineConfig::default()
    };
    // Each document reaches viewer, view, both and view_not_banned: 40 nodes.
    let e = engine_on(MemoryStore::new(), config(39), &rels);
    let err = e
        .lookup_resources(request("document#view@user:anne"))
        .unwrap_err();
    assert!(matches!(err, Error::LookupTooLarge { max: 39 }), "{err:?}");
    assert_eq!(err.code(), "LOOKUP_TOO_LARGE");
    let e = engine_on(MemoryStore::new(), config(40), &rels);
    assert_eq!(
        e.lookup_resources(request("document#view@user:anne"))
            .unwrap()
            .resource_ids
            .len(),
        10
    );
}

#[test]
fn a_candidate_check_error_fails_the_page() {
    let e = engine_on(
        MemoryStore::new(),
        EngineConfig {
            max_depth: 2,
            ..EngineConfig::default()
        },
        &[
            "document:a#viewer@user:anne",
            "group:g#member@user:bob",
            "document:b#viewer@group:g#member",
        ],
    );
    // `b`'s check goes view → viewer → group member: depth 3.
    let err = e
        .lookup_resources(request("document#view@user:bob"))
        .unwrap_err();
    assert!(matches!(err, Error::MaxDepthExceeded { .. }), "{err:?}");
}

#[test]
fn invalid_requests_are_refused() {
    let e = engine(&["document:a#viewer@user:anne"]);
    for q in [
        "nope#view@user:anne",
        "document#nope@user:anne",
        "document#view@nope:anne",
        "document#view@group:eng#nope",
        "document#view@user:*",
    ] {
        let err = e.lookup_resources(request(q)).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{q}: {err:?}");
    }
    let empty = Engine::builder().store(MemoryStore::new()).build().unwrap();
    let err = empty
        .lookup_resources(request("document#view@user:anne"))
        .unwrap_err();
    assert!(matches!(err, Error::NoSchema), "{err:?}");
}

#[test]
fn consistency_is_checked() {
    let e = engine(&["document:a#viewer@user:anne"]);
    let err = e
        .lookup_resources(LookupResourcesRequest {
            consistency: Consistency::AtLeast(Revision(99)),
            ..request("document#view@user:anne")
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { .. }),
        "{err:?}"
    );
}

// ---- paging consistency, on both stores ----

/// Page 1 at `Latest`, a write that adds a matching resource between page 1's cursor and
/// the rest, page 2 at `Exact(page 1's revision)`: page 2 must not see the new resource.
fn later_pages_read_the_first_pages_revision<S: WritableStore>(store: S) {
    let e = engine_on(
        store,
        EngineConfig::default(),
        &["document:a#viewer@user:anne", "document:c#viewer@user:anne"],
    );
    let first = e
        .lookup_resources(LookupResourcesRequest {
            limit: Some(1),
            ..request("document#view@user:anne")
        })
        .unwrap();
    assert_eq!(first.resource_ids, ids(&["a"]));
    let r1 = first.revision;
    let r2 = e.write(touch(&["document:b#viewer@user:anne"])).unwrap();
    assert!(r2 > r1);
    let second = e
        .lookup_resources(LookupResourcesRequest {
            limit: Some(1),
            cursor: first.next_cursor.clone(),
            consistency: Consistency::Exact(r1),
            ..request("document#view@user:anne")
        })
        .unwrap();
    assert_eq!(second.revision, r1);
    assert_eq!(second.resource_ids, ids(&["c"]));
    assert_eq!(second.next_cursor, None);
    // Control: at `Latest` the same cursor does see it.
    let latest = e
        .lookup_resources(LookupResourcesRequest {
            limit: Some(1),
            cursor: first.next_cursor,
            ..request("document#view@user:anne")
        })
        .unwrap();
    assert_eq!(latest.revision, r2);
    assert_eq!(latest.resource_ids, ids(&["b"]));
}

#[test]
fn later_pages_read_the_first_pages_revision_memory() {
    later_pages_read_the_first_pages_revision(MemoryStore::new());
}

#[test]
fn later_pages_read_the_first_pages_revision_sqlite() {
    let dir = TempDir::new();
    later_pages_read_the_first_pages_revision(
        SqliteStore::open(dir.0.join("lookup.db"), SqliteConfig::default()).unwrap(),
    );
}

#[test]
fn hand_written_answers_agree_on_sqlite() {
    let rels = [
        "group:a#member@group:b#member",
        "group:b#member@group:a#member",
        "group:b#member@user:anne",
        "folder:root#viewer@group:a#member",
        "folder:sub#parent@folder:root",
        "document:d#parent@folder:sub",
        "document:e#parent@folder:root#viewer",
        "document:p#viewer@user:*",
        "document:x#viewer@user:anne",
        "document:x#banned@user:anne",
    ];
    let dir = TempDir::new();
    let sqlite = engine_on(
        SqliteStore::open(dir.0.join("lookup.db"), SqliteConfig::default()).unwrap(),
        EngineConfig::default(),
        &rels,
    );
    let memory = engine(&rels);
    for q in [
        "group#member@user:anne",
        "folder#view@user:anne",
        "document#view@user:anne",
        "document#view_not_banned@user:anne",
        "document#view@group:b#member",
        "document#view@folder:sub#view",
    ] {
        assert_eq!(lookup(&sqlite, q), lookup(&memory, q), "{q}");
    }
    assert_eq!(
        lookup(&sqlite, "document#view@user:anne"),
        ["d", "e", "p", "x"]
    );
}

// ---- support ----

/// A fresh directory, removed on drop. Declared before engines so they close first.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-lookup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Fields = Vec<(String, String)>;

/// Records every span's name and fields, including values recorded after creation.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(String, Fields)>>>);

impl Recorder {
    fn field(&self, name: &str) -> Option<String> {
        let spans = self.0.lock().unwrap();
        let (_, fields) = spans.iter().find(|(n, _)| n == "lookup_resources")?;
        fields
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }
}

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
