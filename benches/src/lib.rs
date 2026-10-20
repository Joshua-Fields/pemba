//! Benchmark datasets (`docs/07-testing.md` §7).
//!
//! One definition of every scenario, shared by the Criterion benches in `benches/engine.rs`,
//! the pytest-benchmark scenarios and the SpiceDB comparison: the latter two read the files
//! `bench-data` writes, so all three measure the same schema, tuples and queries.

#![forbid(unsafe_code)]

use std::fmt::Write as _;

/// The schema every scenario loads. A strict subset of SpiceDB's language, so the SpiceDB
/// comparison loads it unchanged.
pub const SCHEMA: &str = "\
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
    relation parent: folder
    relation owner: user
    relation editor: user
    relation viewer: user | group#member
    relation banned: user
    permission edit = editor + owner
    permission view = viewer + edit + parent->view
    permission view_unbanned = (viewer + edit) - banned
}
";

/// Unrelated documents loaded beside every `check` scenario so no store is trivially empty.
pub const NOISE_DOCUMENTS: usize = 1000;

/// Documents in `lookup_resources_10k`.
pub const LOOKUP_DOCUMENTS: usize = 10_000;

/// One in this many `lookup_resources_10k` documents is visible to `user:anne` (5%).
pub const LOOKUP_VISIBLE_EVERY: usize = 20;

/// What a scenario measures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// One `check`, whose answer must be `expect`.
    Check {
        /// `type:id`.
        resource: String,
        /// Relation or permission name.
        permission: String,
        /// `type:id` or `type:id#relation`.
        subject: String,
        /// The right answer, verified before timing.
        expect: bool,
    },
    /// Every page of one `lookup_resources`, `expect` ids in total.
    LookupResources {
        /// Resource type name.
        resource_type: String,
        /// Relation or permission name.
        permission: String,
        /// `type:id`.
        subject: String,
        /// Page size; `None` is the engine's maximum.
        page_size: Option<u32>,
        /// The number of ids across all pages, verified before timing.
        expect: usize,
    },
    /// One write of `size` new relationships (see [`write_batch`]).
    WriteBatch {
        /// Relationships per write.
        size: usize,
    },
}

/// A named dataset and the operation timed against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    /// The name in `docs/07-testing.md` §7, e.g. `groups_depth_5`.
    pub name: String,
    /// Relationships to load first, in pemba's tuple syntax.
    pub tuples: Vec<String>,
    /// The operation timed.
    pub op: Op,
}

fn noise() -> Vec<String> {
    (0..NOISE_DOCUMENTS)
        .map(|i| format!("document:n{i}#viewer@user:n{i}"))
        .collect()
}

fn check(name: String, mut tuples: Vec<String>, permission: &str) -> Scenario {
    tuples.extend(noise());
    Scenario {
        name,
        tuples,
        op: Op::Check {
            resource: "document:target".to_owned(),
            permission: permission.to_owned(),
            subject: "user:anne".to_owned(),
            expect: true,
        },
    }
}

/// `user:anne` in `group:g0`, each `g{i}`'s members in `g{i+1}`, the document shared to the
/// last group: `depth` groups between user and document.
fn groups(depth: usize) -> Scenario {
    let mut t = vec!["group:g0#member@user:anne".to_owned()];
    for i in 1..depth {
        t.push(format!("group:g{i}#member@group:g{}#member", i - 1));
    }
    t.push(format!(
        "document:target#viewer@group:g{}#member",
        depth - 1
    ));
    check(format!("groups_depth_{depth}"), t, "view")
}

/// `user:anne` views `folder:f0`, each `f{i}` is the parent of `f{i+1}`, the document is in
/// the last folder: `depth` folders between user and document.
fn folders(depth: usize) -> Scenario {
    let mut t = vec!["folder:f0#viewer@user:anne".to_owned()];
    for i in 1..depth {
        t.push(format!("folder:f{i}#parent@folder:f{}", i - 1));
    }
    t.push(format!("document:target#parent@folder:f{}", depth - 1));
    check(format!("folders_depth_{depth}"), t, "view")
}

/// Every scenario of `docs/07-testing.md` §7, in table order.
///
/// `lookup_resources_10k_pages_100` is an addition: the same lookup in pages of 100, so
/// the cost of re-running phase 1 on every page (OQ-028 (d)) shows next to the one-page
/// figure.
pub fn scenarios() -> Vec<Scenario> {
    let mut out = vec![check(
        "direct_hit".to_owned(),
        vec!["document:target#viewer@user:anne".to_owned()],
        "view",
    )];
    out.extend([1, 5, 10].map(groups));
    out.extend([1, 10, 30].map(folders));

    let mut wide: Vec<String> = (0..10_000)
        .map(|i| format!("group:g{i}#member@user:anne"))
        .collect();
    wide.push("document:target#viewer@group:g5000#member".to_owned());
    out.push(check("wide_groups_10k".to_owned(), wide, "view"));

    out.push(check(
        "exclusion".to_owned(),
        vec![
            "document:target#viewer@user:anne".to_owned(),
            "document:target#editor@user:bob".to_owned(),
            "document:target#banned@user:bob".to_owned(),
        ],
        "view_unbanned",
    ));

    let lookup: Vec<String> = (0..LOOKUP_DOCUMENTS)
        .map(|i| {
            if i % LOOKUP_VISIBLE_EVERY == 0 {
                format!("document:d{i}#viewer@user:anne")
            } else {
                format!("document:d{i}#viewer@user:u{}", i % 100)
            }
        })
        .collect();
    for (name, page_size) in [
        ("lookup_resources_10k", None),
        ("lookup_resources_10k_pages_100", Some(100)),
    ] {
        out.push(Scenario {
            name: name.to_owned(),
            tuples: lookup.clone(),
            op: Op::LookupResources {
                resource_type: "document".to_owned(),
                permission: "view".to_owned(),
                subject: "user:anne".to_owned(),
                page_size,
                expect: LOOKUP_DOCUMENTS / LOOKUP_VISIBLE_EVERY,
            },
        });
    }

    for size in [1, 100, 1000] {
        out.push(Scenario {
            name: format!("write_batch_{size}"),
            tuples: Vec::new(),
            op: Op::WriteBatch { size },
        });
    }
    out
}

/// The `n`th write of a `write_batch_*` scenario: `size` relationships no earlier write
/// made, so every write inserts rather than re-touching existing rows.
pub fn write_batch(n: u64, size: usize) -> Vec<String> {
    (0..size)
        .map(|i| format!("document:w{n}_{i}#viewer@user:w{i}"))
        .collect()
}

/// The manifest `bench-data` writes: one tab-separated line per scenario,
/// `name, kind, a, b, c, expect, page_size` (`-` for an unused field), where `kind` is
/// `check` (a = resource, b = permission, c = subject), `lookup` (a = resource type,
/// b = permission, c = subject) or `write` (a = batch size).
pub fn manifest() -> String {
    let mut out = String::new();
    for s in scenarios() {
        let line = match &s.op {
            Op::Check {
                resource,
                permission,
                subject,
                expect,
            } => format!(
                "{}\tcheck\t{resource}\t{permission}\t{subject}\t{expect}\t-",
                s.name
            ),
            Op::LookupResources {
                resource_type,
                permission,
                subject,
                page_size,
                expect,
            } => format!(
                "{}\tlookup\t{resource_type}\t{permission}\t{subject}\t{expect}\t{}",
                s.name,
                page_size.map_or_else(|| "-".to_owned(), |p| p.to_string())
            ),
            Op::WriteBatch { size } => format!("{}\twrite\t{size}\t-\t-\t-\t-", s.name),
        };
        let _ = writeln!(out, "{line}");
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn every_tuple_parses() {
        for s in scenarios() {
            for t in &s.tuples {
                t.parse::<pemba::model::Relationship>()
                    .unwrap_or_else(|e| panic!("{}: {t}: {e}", s.name));
            }
        }
    }

    #[test]
    fn schema_compiles() {
        let e = pemba::Engine::builder()
            .store(pemba::MemoryStore::new())
            .build()
            .unwrap();
        e.write_schema(SCHEMA).unwrap();
    }

    #[test]
    fn manifest_has_one_line_per_scenario() {
        assert_eq!(manifest().lines().count(), scenarios().len());
    }
}
