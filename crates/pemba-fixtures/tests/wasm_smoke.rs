//! wasm32 smoke test (T6.1, `docs/11-playground.md` §2): the engine must *run* on
//! `wasm32-unknown-unknown`, not just build. Under Node, a call that traps there (as
//! `std::time::Instant::now` did) fails this test.
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test -p pemba-fixtures --target wasm32-unknown-unknown --test wasm_smoke
//! ```
//!
//! Lives here because `pemba-fixtures` reaches core, schema and the memory store with no
//! dev-dependency that cannot build for wasm32. Native targets compile it to nothing.

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use pemba_core::model::{ObjectId, ObjectRef, Relationship, SubjectRef, WriteOp, WriteRequest};
use pemba_core::{
    CheckRequest, Engine, EngineConfig, LookupResourcesRequest, LookupSubjectsRequest,
};
use pemba_fixtures::{StoreKind, run_str};
use pemba_store_memory::MemoryStore;
use wasm_bindgen_test::wasm_bindgen_test;

const SCHEMA: &str = "
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

const TUPLES: &[&str] = &[
    "group:eng#member@user:anne",
    "group:all#member@group:eng#member",
    "document:readme#viewer@group:all#member",
    "document:spec#owner@user:bob",
];

fn engine() -> Engine<MemoryStore> {
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(EngineConfig::default())
        .build()
        .unwrap();
    engine.write_schema(SCHEMA).unwrap();
    let ops = TUPLES
        .iter()
        .map(|t| WriteOp::Touch(t.parse::<Relationship>().unwrap()))
        .collect();
    engine
        .write(WriteRequest {
            ops,
            preconditions: Vec::new(),
        })
        .unwrap();
    engine
}

fn check_request(query: &str) -> CheckRequest {
    let q: Relationship = query.parse().unwrap();
    CheckRequest {
        resource: q.resource,
        permission: q.relation,
        subject: q.subject,
        consistency: pemba_core::model::Consistency::Latest,
    }
}

#[wasm_bindgen_test]
fn check() {
    let engine = engine();
    assert!(
        engine
            .check(check_request("document:readme#view@user:anne"))
            .unwrap()
            .allowed
    );
    assert!(
        !engine
            .check(check_request("document:spec#view@user:anne"))
            .unwrap()
            .allowed
    );
}

#[wasm_bindgen_test]
fn check_with_trace() {
    let engine = engine();
    let (response, trace) = engine
        .check_with_trace(check_request("document:readme#view@user:anne"))
        .unwrap();
    assert!(response.allowed);
    assert!(trace.to_json().contains("\"allowed\":true"));
    assert!(trace.render().starts_with("allowed:"));
}

#[wasm_bindgen_test]
fn lookups() {
    let engine = engine();
    let anne: SubjectRef = "user:anne".parse().unwrap();
    let resources = engine
        .lookup_resources(LookupResourcesRequest::new(
            "document".parse().unwrap(),
            "view".parse().unwrap(),
            anne,
        ))
        .unwrap();
    let ids: Vec<&str> = resources
        .resource_ids
        .iter()
        .map(ObjectId::as_str)
        .collect();
    assert_eq!(ids, ["readme"]);

    let readme: ObjectRef = "document:readme".parse().unwrap();
    let subjects = engine
        .lookup_subjects(LookupSubjectsRequest::new(
            readme,
            "view".parse().unwrap(),
            "user".parse().unwrap(),
        ))
        .unwrap();
    let ids: Vec<&str> = subjects.subject_ids.iter().map(ObjectId::as_str).collect();
    assert_eq!(ids, ["anne"]);
}

/// The fixture runner (YAML through `serde-saphyr`), as `pemba test` runs it.
#[wasm_bindgen_test]
fn fixture() {
    let yaml = "
schema: |-
  definition user {}
  definition document {
      relation viewer: user
      permission view = viewer
  }
relationships: |-
  document:readme#viewer@user:anne
assertions:
  assertTrue:
    - document:readme#view@user:anne
  assertFalse:
    - document:readme#view@user:bob
expectedLookups:
  resources:
    - query: \"document#view@user:anne\"
      expect: [\"readme\"]
";
    let report = run_str(yaml, Path::new("."), StoreKind::Memory).unwrap();
    assert!(report.passed(), "{:?}", report.failures);
    assert_eq!(report.checked, 3);
}
