//! Differential tests of pemba against SpiceDB (`docs/07-testing.md` §4).
//!
//! A [`DiffCase`] (a schema, its tuples and the questions to ask) is answered by pemba's
//! engine over the memory store and by a SpiceDB server over its HTTP API, and every
//! answer is compared. Each case gets its own SpiceDB datastore: the harness talks to
//! `spicedb serve-testing`, which isolates datastores per bearer token, and uses a fresh
//! token per case. Mismatches that a documented difference explains
//! (`KNOWN_DIFFERENCES.md`, [`known`]) are counted, not failed; any other mismatch is a
//! failure, printed as fixture YAML ready for `testdata/fixtures/regressions/`.
//!
//! The tests are `#[ignore]` and need `PEMBA_SPICEDB_URL` (the HTTP gateway, e.g.
//! `http://127.0.0.1:8443`) and `PEMBA_SPICEDB_KEY` (a token prefix).

#![forbid(unsafe_code)]

pub mod known;
mod spicedb;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use pemba_core::model::{
    Consistency, ObjectId, ObjectRef, RelName, Relationship, SubjectRef, TypeName, WriteOp,
    WriteRequest,
};
use pemba_core::{
    CheckRequest, Engine, EngineConfig, LookupResourcesRequest, LookupSubjectsRequest,
};
use pemba_schema::ir::CompiledSchema;
use pemba_store_memory::MemoryStore;

pub use crate::spicedb::SpiceDb;

/// An object id no generated or fixture tuple uses: it stands for objects never written,
/// so wildcards are compared too.
pub const FRESH_ID: &str = "fresh";

/// Datastores a schema is written to before the case's own (see [`run`]). SpiceDB
/// 1.56.2 accepted one schema it should refuse in 24 of 60 writes; 12 probes plus the
/// case's own write let such a schema through about once in 150 000 cases, and then it
/// shows as an unexplained mismatch rather than passing silently.
const SCHEMA_PROBES: usize = 12;

/// The most `lookup_resources` / `lookup_subjects` questions asked per case.
const MAX_LOOKUPS: usize = 12;

/// One schema, its data and the questions both engines answer.
#[derive(Clone, Debug)]
pub struct DiffCase {
    /// Where it came from, e.g. `fixture google_docs.yaml` or `generated #17`.
    pub name: String,
    /// Schema source, accepted by both.
    pub schema: String,
    /// Tuples written (touched) before any question.
    pub tuples: Vec<Relationship>,
    /// Objects to ask about besides those in the tuples (e.g. from fixture assertions).
    pub extra_objects: Vec<ObjectRef>,
}

/// A question asked of both engines.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Question {
    /// `check(resource, permission, subject)`.
    Check(ObjectRef, RelName, SubjectRef),
    /// `lookup_resources(type, permission, subject)`.
    LookupResources(TypeName, RelName, SubjectRef),
    /// `lookup_subjects(resource, permission, subject_type)`.
    LookupSubjects(ObjectRef, RelName, TypeName),
}

impl fmt::Display for Question {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Check(r, p, s) => write!(f, "check {r}#{p}@{s}"),
            Self::LookupResources(t, p, s) => write!(f, "lookup_resources {t}#{p}@{s}"),
            Self::LookupSubjects(r, p, t) => write!(f, "lookup_subjects {r}#{p}@{t}"),
        }
    }
}

/// An answer: `Err` holds the error message (both engines may refuse a question).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A check result.
    Allowed(bool),
    /// Resource ids, ascending.
    Resources(Vec<String>),
    /// Subject ids (ascending, empty with a wildcard), wildcard, excluded ids (ascending).
    Subjects(Vec<String>, bool, Vec<String>),
    /// An error, by message.
    Error(String),
}

impl Answer {
    /// Equal, or both errors (the engines word their errors differently).
    pub fn agrees(&self, other: &Self) -> bool {
        match (self, other) {
            // Error messages differ between the engines; failing on both sides agrees.
            (Self::Error(_), Self::Error(_)) => true,
            (a, b) => a == b,
        }
    }
}

impl fmt::Display for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed(b) => write!(f, "{b}"),
            Self::Resources(ids) | Self::Subjects(ids, false, _) => write!(f, "{ids:?}"),
            Self::Subjects(_, true, excluded) => write!(f, "every object except {excluded:?}"),
            Self::Error(e) => write!(f, "error: {e}"),
        }
    }
}

/// A question the engines answered differently.
#[derive(Clone, Debug)]
pub struct Mismatch {
    /// The question.
    pub question: Question,
    /// pemba's answer.
    pub pemba: Answer,
    /// SpiceDB's answer.
    pub spicedb: Answer,
    /// The `KNOWN_DIFFERENCES.md` entry that explains it, if any.
    pub known: Option<&'static str>,
}

/// The outcome of one case.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The known difference that stopped the case before any question (SpiceDB refused a
    /// schema pemba accepts, `KD-2`).
    pub not_compared: Option<&'static str>,
    /// Questions asked.
    pub asked: usize,
    /// Every disagreement, explained or not.
    pub mismatches: Vec<Mismatch>,
}

impl Outcome {
    /// Mismatches no known difference explains.
    pub fn unexplained(&self) -> impl Iterator<Item = &Mismatch> {
        self.mismatches.iter().filter(|m| m.known.is_none())
    }
}

/// Runs `case` on pemba and on SpiceDB under the datastore of `token`.
///
/// # Errors
///
/// A message if the case could not be set up on either side (schema or tuples refused,
/// SpiceDB unreachable). Answers that are errors are compared, not returned.
pub fn run(spicedb: &SpiceDb, token: &str, case: &DiffCase) -> Result<Outcome, String> {
    let compiled = pemba_schema::compile(&case.schema)
        .map_err(|e| format!("pemba refused the schema: {e}"))?
        .schema;
    let engine = pemba_engine(case)?;
    // SpiceDB's check behind KD-2 is nondeterministic: the same schema is sometimes
    // accepted. Probe a few throwaway datastores so a case it would refuse is never
    // compared by luck (and its KD-3 re-check never fails by luck).
    // The real write can still be the one it refuses (every probe accepted by luck), so it
    // is classified the same way rather than failing the case.
    let refused = |e: String| match known::explain_setup(&e) {
        Some(k) => Ok(Outcome {
            not_compared: Some(k),
            ..Outcome::default()
        }),
        None => Err(e),
    };
    for probe in 0..SCHEMA_PROBES {
        let probe_token = format!("{token}-probe{probe}");
        if let Err(e) = spicedb.write_schema(&probe_token, &case.schema) {
            return refused(e);
        }
    }
    if let Err(e) = spicedb.write_schema(token, &case.schema) {
        return refused(e);
    }
    spicedb.write_tuples(token, &case.tuples)?;

    let questions = questions(&compiled, case);
    let checks: Vec<&Question> = questions
        .iter()
        .filter(|q| matches!(q, Question::Check(..)))
        .collect();
    let mut theirs = spicedb.check_bulk(token, &checks)?.into_iter();

    // Both engines again without one type's wildcard tuples, built on first use (KD-3).
    let mut reduced: BTreeMap<TypeName, Option<(Engine<MemoryStore>, String)>> = BTreeMap::new();
    let mut without_wildcards = |ty: &TypeName, q: &Question| -> Option<(Answer, Answer)> {
        let entry = reduced.entry(ty.clone()).or_insert_with(|| {
            let less = DiffCase {
                tuples: case
                    .tuples
                    .iter()
                    .filter(|t| {
                        !matches!(&t.subject, SubjectRef::Wildcard { object_type } if object_type == ty)
                    })
                    .cloned()
                    .collect(),
                ..case.clone()
            };
            let token = format!("{token}-no-{}", ty.as_str().replace('/', "_"));
            let engine = pemba_engine(&less).ok()?;
            spicedb.write_schema(&token, &less.schema).ok()?;
            spicedb.write_tuples(&token, &less.tuples).ok()?;
            Some((engine, token))
        });
        let (engine, token) = entry.as_ref()?;
        let theirs = match q {
            Question::Check(..) => spicedb.check_bulk(token, &[q]).ok()?.pop()?,
            _ => return None,
        };
        Some((ask_pemba(engine, q), theirs))
    };

    let mut outcome = Outcome::default();
    for q in &questions {
        let spicedb_answer = match q {
            Question::Check(..) => theirs
                .next()
                .ok_or_else(|| "SpiceDB returned fewer bulk results".to_owned())?,
            Question::LookupResources(t, p, s) => spicedb.lookup_resources(token, t, p, s),
            Question::LookupSubjects(r, p, t) => spicedb.lookup_subjects(token, r, p, t),
        };
        let pemba_answer = ask_pemba(&engine, q);
        outcome.asked += 1;
        if !pemba_answer.agrees(&spicedb_answer) {
            outcome.mismatches.push(Mismatch {
                known: known::explain(
                    &mut without_wildcards,
                    case,
                    q,
                    &pemba_answer,
                    &spicedb_answer,
                ),
                question: q.clone(),
                pemba: pemba_answer,
                spicedb: spicedb_answer,
            });
        }
    }
    let types: Vec<&str> = compiled.types.iter().map(|t| &*t.name).collect();
    spicedb.clear(token, &types)?;
    for (ty, made) in &reduced {
        if made.is_some() {
            spicedb.clear(
                &format!("{token}-no-{}", ty.as_str().replace('/', "_")),
                &types,
            )?;
        }
    }
    Ok(outcome)
}

/// pemba's engine over the memory store with `case`'s schema and tuples.
fn pemba_engine(case: &DiffCase) -> Result<Engine<MemoryStore>, String> {
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(EngineConfig::default())
        .build()
        .map_err(|e| e.to_string())?;
    engine
        .write_schema(&case.schema)
        .map_err(|e| format!("pemba refused the schema: {e}"))?;
    if !case.tuples.is_empty() {
        engine
            .write(WriteRequest {
                ops: case.tuples.iter().cloned().map(WriteOp::Touch).collect(),
                preconditions: Vec::new(),
            })
            .map_err(|e| format!("pemba refused the tuples: {e}"))?;
    }
    Ok(engine)
}

/// `tuples` without the ones that would close a cycle between objects (`a` → `b` when a
/// tuple on `a` names `b` as its subject), in order: each tuple is kept unless its subject
/// already reaches its resource.
///
/// Generated cases go through this before they are compared. On cyclic data SpiceDB
/// dispatches until its depth limit, exploring paths exponentially (`KD-1`); on generated
/// data that exhausted 2.5 GB in one bulk check. Cyclic data is compared through the
/// fixtures instead.
pub fn without_cycles(tuples: &[Relationship]) -> Vec<Relationship> {
    let mut edges: BTreeMap<ObjectRef, BTreeSet<ObjectRef>> = BTreeMap::new();
    let mut kept = Vec::new();
    for t in tuples {
        let target = match &t.subject {
            SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => Some(o),
            SubjectRef::Wildcard { .. } => None,
        };
        if let Some(target) = target {
            if reaches(&edges, target, &t.resource) {
                continue;
            }
            edges
                .entry(t.resource.clone())
                .or_default()
                .insert(target.clone());
        }
        kept.push(t.clone());
    }
    kept
}

fn reaches(
    edges: &BTreeMap<ObjectRef, BTreeSet<ObjectRef>>,
    from: &ObjectRef,
    to: &ObjectRef,
) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(o) = stack.pop() {
        if o == to {
            return true;
        }
        if seen.insert(o) {
            stack.extend(edges.get(o).into_iter().flatten());
        }
    }
    false
}

/// Every object the case mentions, by type, plus [`FRESH_ID`] for every type.
fn universe(schema: &CompiledSchema, case: &DiffCase) -> BTreeSet<ObjectRef> {
    let mut objects = BTreeSet::new();
    for t in &case.tuples {
        objects.insert(t.resource.clone());
        match &t.subject {
            SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => {
                objects.insert(o.clone());
            }
            SubjectRef::Wildcard { .. } => {}
        }
    }
    objects.extend(case.extra_objects.iter().cloned());
    for ty in &schema.types {
        if let (Ok(t), Ok(id)) = (ty.name.parse(), FRESH_ID.parse()) {
            objects.insert(ObjectRef {
                object_type: t,
                object_id: id,
            });
        }
    }
    objects
}

/// Checks of every member of every object for every object subject and every subject set
/// in the tuples; then up to [`MAX_LOOKUPS`] of each lookup, spread over the universe.
fn questions(schema: &CompiledSchema, case: &DiffCase) -> Vec<Question> {
    let objects = universe(schema, case);
    let mut subjects: Vec<SubjectRef> = objects.iter().cloned().map(SubjectRef::Object).collect();
    let sets: BTreeSet<SubjectRef> = case
        .tuples
        .iter()
        .filter(|t| matches!(t.subject, SubjectRef::Set { .. }))
        .map(|t| t.subject.clone())
        .collect();
    subjects.extend(sets);

    let members = |ty: &TypeName| -> Vec<RelName> {
        schema
            .types
            .iter()
            .find(|t| *t.name == *ty.as_str())
            .map(|t| {
                t.members
                    .iter()
                    .filter_map(|m| m.name().parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut out = Vec::new();
    for resource in &objects {
        for member in members(&resource.object_type) {
            for subject in &subjects {
                out.push(Question::Check(
                    resource.clone(),
                    member.clone(),
                    subject.clone(),
                ));
            }
        }
    }

    // Lookups: a deterministic spread, so every case asks a few of each.
    let mut lookups = Vec::new();
    for subject in subjects
        .iter()
        .filter(|s| matches!(s, SubjectRef::Object(_)))
    {
        for ty in &schema.types {
            let Ok(ty_name) = ty.name.parse::<TypeName>() else {
                continue;
            };
            for member in members(&ty_name) {
                lookups.push(Question::LookupResources(
                    ty_name.clone(),
                    member,
                    subject.clone(),
                ));
            }
        }
    }
    out.extend(spread(lookups, MAX_LOOKUPS));
    let mut lookups = Vec::new();
    for resource in &objects {
        for member in members(&resource.object_type) {
            for ty in &schema.types {
                if let Ok(t) = ty.name.parse::<TypeName>() {
                    lookups.push(Question::LookupSubjects(
                        resource.clone(),
                        member.clone(),
                        t,
                    ));
                }
            }
        }
    }
    out.extend(spread(lookups, MAX_LOOKUPS));
    out
}

/// At most `n` items, evenly spaced through `items`.
fn spread<T>(items: Vec<T>, n: usize) -> Vec<T> {
    let len = items.len();
    if len <= n {
        return items;
    }
    items
        .into_iter()
        .enumerate()
        .filter(|(i, _)| (i * n) % len < n)
        .map(|(_, q)| q)
        .take(n)
        .collect()
}

fn ask_pemba(engine: &Engine<MemoryStore>, q: &Question) -> Answer {
    match q {
        Question::Check(resource, permission, subject) => match engine.check(CheckRequest {
            resource: resource.clone(),
            permission: permission.clone(),
            subject: subject.clone(),
            consistency: Consistency::Latest,
        }) {
            Ok(c) => Answer::Allowed(c.allowed),
            Err(e) => Answer::Error(format!("{}: {e}", e.code())),
        },
        Question::LookupResources(ty, permission, subject) => {
            let mut req =
                LookupResourcesRequest::new(ty.clone(), permission.clone(), subject.clone());
            let mut ids = Vec::new();
            loop {
                match engine.lookup_resources(req.clone()) {
                    Ok(resp) => {
                        ids.extend(resp.resource_ids.iter().map(ToString::to_string));
                        match resp.next_cursor {
                            Some(c) => {
                                req.cursor = Some(c);
                                req.consistency = Consistency::Exact(resp.revision);
                            }
                            None => break,
                        }
                    }
                    Err(e) => return Answer::Error(format!("{}: {e}", e.code())),
                }
            }
            ids.sort();
            Answer::Resources(ids)
        }
        Question::LookupSubjects(resource, permission, ty) => {
            match engine.lookup_subjects(LookupSubjectsRequest::new(
                resource.clone(),
                permission.clone(),
                ty.clone(),
            )) {
                Ok(resp) => Answer::Subjects(
                    resp.subject_ids.iter().map(ObjectId::to_string).collect(),
                    resp.wildcard,
                    resp.excluded_ids.iter().map(ObjectId::to_string).collect(),
                ),
                Err(e) => Answer::Error(format!("{}: {e}", e.code())),
            }
        }
    }
}

/// `case` and its unexplained mismatches as a fixture (`docs/07-testing.md` §3), with each
/// check listed under pemba's answer and SpiceDB's in a comment, ready to paste into
/// `testdata/fixtures/regressions/` once someone decides who is right.
pub fn repro_yaml(case: &DiffCase, mismatches: &[&Mismatch]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# differential test: {}", case.name);
    out.push_str("schema: |-\n");
    for line in case.schema.lines() {
        let _ = writeln!(out, "  {line}");
    }
    out.push_str("relationships: |-\n");
    for t in &case.tuples {
        let _ = writeln!(out, "  {t}");
    }
    let (mut yes, mut no) = (Vec::new(), Vec::new());
    for m in mismatches {
        match (&m.question, &m.pemba) {
            (Question::Check(r, p, s), Answer::Allowed(true)) => yes.push((r, p, s, &m.spicedb)),
            (Question::Check(r, p, s), Answer::Allowed(false)) => no.push((r, p, s, &m.spicedb)),
            (q, w) => {
                let _ = writeln!(out, "# {q}: pemba {w}, SpiceDB {}", m.spicedb);
            }
        }
    }
    if !yes.is_empty() || !no.is_empty() {
        out.push_str("assertions:\n");
        for (key, list) in [("assertTrue", yes), ("assertFalse", no)] {
            if list.is_empty() {
                continue;
            }
            let _ = writeln!(out, "  {key}:");
            for (r, p, s, theirs) in list {
                let _ = writeln!(out, "    - {r}#{p}@{s} # SpiceDB: {theirs}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels(tuples: &[&str]) -> Vec<Relationship> {
        tuples.iter().map(|t| t.parse().unwrap()).collect()
    }

    #[test]
    fn cycles_are_cut_where_they_close() {
        let kept = without_cycles(&rels(&[
            "grp:a#member@grp:b#member",
            "grp:b#member@grp:c",
            "grp:c#member@grp:a#member",
            "grp:c#member@user:*",
            "grp:d#member@grp:d#member",
            "grp:a#member@grp:c",
        ]));
        assert_eq!(
            kept,
            rels(&[
                "grp:a#member@grp:b#member",
                "grp:b#member@grp:c",
                "grp:c#member@user:*",
                "grp:a#member@grp:c",
            ])
        );
    }

    #[test]
    fn spread_keeps_at_most_n_evenly() {
        assert_eq!(spread((0..5).collect(), 12), vec![0, 1, 2, 3, 4]);
        let s = spread((0..100).collect::<Vec<usize>>(), 10);
        assert_eq!(s.len(), 10);
        assert_eq!(s[0], 0);
    }
}
