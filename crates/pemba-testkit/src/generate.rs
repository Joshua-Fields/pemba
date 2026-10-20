//! `proptest` strategies for schemas, relationships and queries (`docs/07-testing.md` §2.2).
//!
//! Choices are drawn as raw numbers and resolved against what exists, so every value is
//! valid by construction and shrinks well: schemas always compile, tuples always match
//! the schema.
//!
//! Generated schemas have:
//! - 1–4 types (`ty0`..), each with 1–4 relations (`rel0`..) and 0–3 permissions
//!   (`perm0`..); relations allow objects, wildcards and subject sets (of any member);
//! - permission expressions of depth ≤ 3 over union, intersection, exclusion and arrows
//!   (`->` and `.all`). A permission computes relations and *earlier* permissions only (no
//!   `E_DIRECT_CYCLE`); recursion comes from arrows and subject sets, as in real schemas;
//! - stratifiable recursion only (OQ-020): no cycle of the member dependency graph runs
//!   through an exclusion's subtracted side, so no data cycle can cross a negation and the
//!   engine never answers `NonMonotonicCycle`. Two variants ([`SubSides`]): the default
//!   keeps recursion off `sub` sides entirely (nothing a `sub` side reaches lies on a
//!   cycle); [`SubSides::Recursive`] allows monotone recursion beneath them (T1.4b).

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

use pemba_core::model::Relationship;
use proptest::prelude::*;

/// Object ids used in generated tuples: a small alphabet, to force sharing and cycles.
pub const IDS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

/// How much recursion generated schemas allow beneath an exclusion's subtracted side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubSides {
    /// Nothing a `sub` side reaches lies on a cycle.
    Acyclic,
    /// A `sub` side may reach cycles, as long as none leads back to the permission that
    /// subtracts it (the schema stays stratifiable).
    Recursive,
}

/// A generated schema, before rendering to source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaModel {
    /// Types `ty0`, `ty1`, … in order.
    pub types: Vec<TypeModel>,
}

/// One generated type. Its members are its relations, then its permissions; a member index
/// counts across both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeModel {
    /// `relN`: allowed subjects of each relation (non-empty, no duplicates).
    pub relations: Vec<Vec<Allowed>>,
    /// `permN`: each permission's expression.
    pub permissions: Vec<ExprModel>,
}

/// One allowed subject form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allowed {
    /// `tyN`.
    Object(usize),
    /// `tyN:*`.
    Wildcard(usize),
    /// `tyN#member`, by member index.
    Set(usize, usize),
}

/// A permission expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprModel {
    /// A member of the same type, by member index.
    Member(usize),
    /// `relT->name` or `relT.all(name)`.
    Arrow {
        /// Relation index of the tupleset.
        tupleset: usize,
        /// Member name looked up on each related type.
        target: String,
        /// `.all` rather than `->`.
        all: bool,
    },
    /// `a + b + …`.
    Union(Vec<ExprModel>),
    /// `a & b & …`.
    Intersection(Vec<ExprModel>),
    /// `a - b`.
    Exclusion(Box<ExprModel>, Box<ExprModel>),
}

impl TypeModel {
    fn member_count(&self) -> usize {
        self.relations.len() + self.permissions.len()
    }

    fn member_name(&self, i: usize) -> String {
        if i < self.relations.len() {
            format!("rel{i}")
        } else {
            format!("perm{}", i - self.relations.len())
        }
    }

    fn member_index(&self, name: &str) -> Option<usize> {
        (0..self.member_count()).find(|&i| self.member_name(i) == name)
    }
}

impl SchemaModel {
    /// The schema source, in canonical member order (relations before permissions).
    pub fn source(&self) -> String {
        let mut out = String::new();
        for (i, t) in self.types.iter().enumerate() {
            let _ = writeln!(out, "definition ty{i} {{");
            for (j, allowed) in t.relations.iter().enumerate() {
                let allowed: Vec<String> = allowed.iter().map(|a| self.allowed(*a)).collect();
                let _ = writeln!(out, "    relation rel{j}: {}", allowed.join(" | "));
            }
            for (j, e) in t.permissions.iter().enumerate() {
                let _ = writeln!(out, "    permission perm{j} = {}", expr_source(t, e));
            }
            out.push_str("}\n\n");
        }
        out
    }

    fn allowed(&self, a: Allowed) -> String {
        match a {
            Allowed::Object(t) => format!("ty{t}"),
            Allowed::Wildcard(t) => format!("ty{t}:*"),
            Allowed::Set(t, m) => format!("ty{t}#{}", self.types[t].member_name(m)),
        }
    }

    /// Whether some member lies on a cycle of the dependency graph (data-dependent
    /// recursion is possible).
    pub fn is_recursive(&self) -> bool {
        let g = Graph::new(self);
        (0..g.nodes.len()).any(|n| g.cyclic(n))
    }

    /// Whether some exclusion's base side reaches a member on a cycle.
    pub fn has_recursive_exclusion_base(&self) -> bool {
        let g = Graph::new(self);
        let mut found = false;
        for (ti, t) in self.types.iter().enumerate() {
            for e in &t.permissions {
                visit(e, &mut |x| {
                    if let ExprModel::Exclusion(base, _) = x {
                        let mut targets = Vec::new();
                        direct_targets(self, ti, base, &mut targets);
                        found |= g.reaches_cycle(&targets);
                    }
                });
            }
        }
        found
    }

    /// Whether some exclusion's subtracted side reaches a member on a cycle.
    pub fn has_recursive_exclusion_sub(&self) -> bool {
        let g = Graph::new(self);
        let mut found = false;
        for (ti, t) in self.types.iter().enumerate() {
            for e in &t.permissions {
                visit(e, &mut |x| {
                    if let ExprModel::Exclusion(_, sub) = x {
                        let mut targets = Vec::new();
                        direct_targets(self, ti, sub, &mut targets);
                        found |= g.reaches_cycle(&targets);
                    }
                });
            }
        }
        found
    }

    /// Replaces every unsafe subtracted side with a relation of the same type that allows
    /// no subject sets (a node with no dependencies), or, if the type has none, drops the
    /// subtraction. With [`SubSides::Acyclic`] a side is unsafe if it reaches a cycle; with
    /// [`SubSides::Recursive`], if it reaches the permission it is in (a negative edge on a
    /// cycle). Only removes edges, so it never creates a cycle.
    fn fix_sub_sides(&mut self, mode: SubSides) {
        loop {
            let g = Graph::new(self);
            let mut fixed = false;
            for ti in 0..self.types.len() {
                let plain = self.types[ti]
                    .relations
                    .iter()
                    .position(|a| !a.iter().any(|x| matches!(x, Allowed::Set(..))));
                let snapshot = self.clone();
                let relations = self.types[ti].relations.len();
                for (pi, e) in self.types[ti].permissions.iter_mut().enumerate() {
                    let unsafe_targets = |targets: &[(usize, usize)]| match mode {
                        SubSides::Acyclic => g.reaches_cycle(targets),
                        SubSides::Recursive => g.reaches(targets, (ti, relations + pi)),
                    };
                    fixed |= fix_subs(&snapshot, ti, e, &unsafe_targets, plain);
                }
            }
            if !fixed {
                return;
            }
        }
    }
}

/// The source text of a permission expression on `t`.
fn expr_source(t: &TypeModel, e: &ExprModel) -> String {
    // Compound operands are always parenthesised, so the mixing rule always holds.
    let operand = |x: &ExprModel| match x {
        ExprModel::Member(_) | ExprModel::Arrow { .. } => expr_source(t, x),
        _ => format!("({})", expr_source(t, x)),
    };
    match e {
        ExprModel::Member(i) => t.member_name(*i),
        ExprModel::Arrow {
            tupleset,
            target,
            all,
        } => {
            if *all {
                format!("rel{tupleset}.all({target})")
            } else {
                format!("rel{tupleset}->{target}")
            }
        }
        ExprModel::Union(xs) => xs.iter().map(operand).collect::<Vec<_>>().join(" + "),
        ExprModel::Intersection(xs) => xs.iter().map(operand).collect::<Vec<_>>().join(" & "),
        ExprModel::Exclusion(a, b) => format!("{} - {}", operand(a), operand(b)),
    }
}

/// A `(type, member)` node of the dependency graph, by indices.
type Node = (usize, usize);

/// Fixes the first unsafe subtracted side found in `e` (outermost first); returns whether
/// it changed anything.
fn fix_subs(
    schema: &SchemaModel,
    ty: usize,
    e: &mut ExprModel,
    is_unsafe: &dyn Fn(&[Node]) -> bool,
    plain: Option<usize>,
) -> bool {
    match e {
        ExprModel::Member(_) | ExprModel::Arrow { .. } => false,
        ExprModel::Union(xs) | ExprModel::Intersection(xs) => xs
            .iter_mut()
            .any(|x| fix_subs(schema, ty, x, is_unsafe, plain)),
        ExprModel::Exclusion(base, sub) => {
            let mut targets = Vec::new();
            direct_targets(schema, ty, sub, &mut targets);
            // A nested exclusion inside `sub` needs no separate fix: its targets are a
            // subset of these.
            if is_unsafe(&targets) {
                match plain {
                    Some(r) => **sub = ExprModel::Member(r),
                    None => *e = (**base).clone(),
                }
                true
            } else {
                fix_subs(schema, ty, base, is_unsafe, plain)
            }
        }
    }
}

fn visit(e: &ExprModel, f: &mut impl FnMut(&ExprModel)) {
    f(e);
    match e {
        ExprModel::Member(_) | ExprModel::Arrow { .. } => {}
        ExprModel::Union(xs) | ExprModel::Intersection(xs) => {
            for x in xs {
                visit(x, f);
            }
        }
        ExprModel::Exclusion(a, b) => {
            visit(a, f);
            visit(b, f);
        }
    }
}

/// The `(type, member)` nodes `e` (on type `ty`) depends on directly.
fn direct_targets(schema: &SchemaModel, ty: usize, e: &ExprModel, out: &mut Vec<(usize, usize)>) {
    visit(e, &mut |x| match x {
        ExprModel::Member(m) => out.push((ty, *m)),
        ExprModel::Arrow {
            tupleset, target, ..
        } => {
            out.push((ty, *tupleset));
            for a in &schema.types[ty].relations[*tupleset] {
                let x = match *a {
                    Allowed::Object(x) | Allowed::Set(x, _) => x,
                    Allowed::Wildcard(_) => continue,
                };
                if let Some(m) = schema.types[x].member_index(target) {
                    out.push((x, m));
                }
            }
        }
        _ => {}
    });
}

/// The generator's own view of the member dependency graph (the oracle computes its own
/// from the compiled IR; a test checks they agree on stratifiability).
struct Graph {
    nodes: Vec<(usize, usize)>,
    /// `reach[i]`: nodes reachable from node `i` by one or more edges.
    reach: Vec<BTreeSet<usize>>,
}

impl Graph {
    fn new(schema: &SchemaModel) -> Self {
        let nodes: Vec<(usize, usize)> = schema
            .types
            .iter()
            .enumerate()
            .flat_map(|(t, def)| (0..def.member_count()).map(move |m| (t, m)))
            .collect();
        let index = |n: (usize, usize)| nodes.iter().position(|&x| x == n);
        let succ: Vec<Vec<usize>> = nodes
            .iter()
            .map(|&(t, m)| {
                let def = &schema.types[t];
                let mut targets = Vec::new();
                if m < def.relations.len() {
                    for a in &def.relations[m] {
                        if let Allowed::Set(x, y) = *a {
                            targets.push((x, y));
                        }
                    }
                } else {
                    direct_targets(
                        schema,
                        t,
                        &def.permissions[m - def.relations.len()],
                        &mut targets,
                    );
                }
                targets.into_iter().filter_map(index).collect()
            })
            .collect();
        let reach = (0..nodes.len())
            .map(|start| {
                let mut seen = BTreeSet::new();
                let mut todo = succ[start].clone();
                while let Some(n) = todo.pop() {
                    if seen.insert(n) {
                        todo.extend(&succ[n]);
                    }
                }
                seen
            })
            .collect();
        Self { nodes, reach }
    }

    fn cyclic(&self, n: usize) -> bool {
        self.reach[n].contains(&n)
    }

    /// Whether any of `targets` is `node` or reaches it.
    fn reaches(&self, targets: &[(usize, usize)], node: (usize, usize)) -> bool {
        let Some(n) = self.nodes.iter().position(|&x| x == node) else {
            return false;
        };
        targets.iter().any(|t| {
            self.nodes
                .iter()
                .position(|x| x == t)
                .is_some_and(|i| i == n || self.reach[i].contains(&n))
        })
    }

    /// Whether any of `targets`, or anything they reach, lies on a cycle.
    fn reaches_cycle(&self, targets: &[(usize, usize)]) -> bool {
        targets.iter().any(|t| {
            self.nodes
                .iter()
                .position(|n| n == t)
                .is_some_and(|i| self.cyclic(i) || self.reach[i].iter().any(|&j| self.cyclic(j)))
        })
    }
}

// ---- raw choices ----

#[derive(Clone, Debug)]
enum RawExpr {
    Leaf(usize),
    Arrow(usize, usize, usize, bool),
    Union(Vec<RawExpr>),
    Intersection(Vec<RawExpr>),
    Exclusion(Box<RawExpr>, Box<RawExpr>),
}

#[derive(Clone, Debug)]
struct RawType {
    /// Per relation: `(kind, type, member)`, kind 0 object, 1 wildcard, 2 subject set.
    relations: Vec<Vec<(u8, usize, usize)>>,
    permissions: Vec<RawExpr>,
}

fn raw_expr() -> impl Strategy<Value = RawExpr> {
    let leaf = prop_oneof![
        3 => any::<usize>().prop_map(RawExpr::Leaf),
        2 => (any::<usize>(), any::<usize>(), any::<usize>(), prop::bool::weighted(0.3))
            .prop_map(|(r, t, m, all)| RawExpr::Arrow(r, t, m, all)),
    ];
    // Two levels above the leaves: depth ≤ 3.
    leaf.prop_recursive(2, 12, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(RawExpr::Union),
            prop::collection::vec(inner.clone(), 2..4).prop_map(RawExpr::Intersection),
            (inner.clone(), inner).prop_map(|(a, b)| RawExpr::Exclusion(Box::new(a), Box::new(b))),
        ]
    })
}

fn raw_type() -> impl Strategy<Value = RawType> {
    let kind = prop_oneof![5 => Just(0u8), 1 => Just(1u8), 4 => Just(2u8)];
    let relation = prop::collection::vec((kind, any::<usize>(), any::<usize>()), 1..4);
    (
        prop::collection::vec(relation, 1..5),
        prop::collection::vec(raw_expr(), 0..4),
    )
        .prop_map(|(relations, permissions)| RawType {
            relations,
            permissions,
        })
}

/// Resolves raw choices into a valid model.
fn resolve(raw: &[RawType], mode: SubSides) -> SchemaModel {
    let n = raw.len();
    let member_count = |t: usize| raw[t].relations.len() + raw[t].permissions.len();
    let mut types: Vec<TypeModel> = raw
        .iter()
        .map(|t| TypeModel {
            relations: t
                .relations
                .iter()
                .map(|allowed| {
                    let mut out: Vec<Allowed> = Vec::new();
                    for &(kind, ty, m) in allowed {
                        let ty = ty % n;
                        let a = match kind {
                            0 => Allowed::Object(ty),
                            1 => Allowed::Wildcard(ty),
                            _ => Allowed::Set(ty, m % member_count(ty)),
                        };
                        if !out.contains(&a) {
                            out.push(a);
                        }
                    }
                    out
                })
                .collect(),
            permissions: Vec::new(),
        })
        .collect();
    // Member names only, so arrows can resolve targets before permissions are built.
    let shapes = types.clone();
    let names = |t: usize| -> Vec<String> {
        (0..member_count(t))
            .map(|i| {
                if i < shapes[t].relations.len() {
                    format!("rel{i}")
                } else {
                    format!("perm{}", i - shapes[t].relations.len())
                }
            })
            .collect()
    };
    for (ti, t) in raw.iter().enumerate() {
        let relations = &shapes[ti].relations;
        for (pi, e) in t.permissions.iter().enumerate() {
            let e = resolve_expr(e, relations, pi, &names);
            types[ti].permissions.push(e);
        }
    }
    let mut model = SchemaModel { types };
    model.fix_sub_sides(mode);
    model
}

fn resolve_expr(
    e: &RawExpr,
    relations: &[Vec<Allowed>],
    perm: usize,
    names: &dyn Fn(usize) -> Vec<String>,
) -> ExprModel {
    // Relations and earlier permissions: never a direct cycle.
    let leaves = relations.len() + perm;
    let leaf = |i: usize| ExprModel::Member(i % leaves);
    let rec = |x: &RawExpr| resolve_expr(x, relations, perm, names);
    match e {
        RawExpr::Leaf(i) => leaf(*i),
        RawExpr::Arrow(rel, ty, member, all) => {
            // Arrows may only walk relations without wildcards (`E_ARROW_OVER_WILDCARD`).
            let walkable: Vec<usize> = (0..relations.len())
                .filter(|&j| {
                    !relations[j]
                        .iter()
                        .any(|a| matches!(a, Allowed::Wildcard(_)))
                })
                .collect();
            if walkable.is_empty() {
                return leaf(*rel);
            }
            let tupleset = walkable[rel % walkable.len()];
            let types: Vec<usize> = relations[tupleset]
                .iter()
                .filter_map(|a| match *a {
                    Allowed::Object(x) | Allowed::Set(x, _) => Some(x),
                    Allowed::Wildcard(_) => None,
                })
                .collect();
            let candidates = names(types[ty % types.len()]);
            ExprModel::Arrow {
                tupleset,
                target: candidates[member % candidates.len()].clone(),
                all: *all,
            }
        }
        RawExpr::Union(xs) => ExprModel::Union(xs.iter().map(rec).collect()),
        RawExpr::Intersection(xs) => ExprModel::Intersection(xs.iter().map(rec).collect()),
        RawExpr::Exclusion(a, b) => ExprModel::Exclusion(Box::new(rec(a)), Box::new(rec(b))),
    }
}

/// Valid schemas as described in the module docs, with no recursion beneath subtracted
/// sides.
pub fn schema() -> impl Strategy<Value = SchemaModel> {
    schema_with(SubSides::Acyclic)
}

/// Valid schemas as described in the module docs, in the given variant.
pub fn schema_with(mode: SubSides) -> impl Strategy<Value = SchemaModel> {
    prop::collection::vec(raw_type(), 1..5).prop_map(move |raw| resolve(&raw, mode))
}

/// A raw tuple choice: `(type, relation, allowed form, resource id, subject id)`.
type RawTuple = (usize, usize, usize, usize, usize);

/// Resolves raw tuple choices against `schema`, dropping duplicates.
fn tuples(schema: &SchemaModel, raw: &[RawTuple]) -> Vec<Relationship> {
    let mut out: Vec<Relationship> = Vec::new();
    for &(t, r, a, res, sub) in raw {
        let ti = t % schema.types.len();
        let def = &schema.types[ti];
        let ri = r % def.relations.len();
        let allowed = &def.relations[ri];
        let subject = match allowed[a % allowed.len()] {
            Allowed::Object(x) => format!("ty{x}:{}", IDS[sub % IDS.len()]),
            Allowed::Wildcard(x) => format!("ty{x}:*"),
            Allowed::Set(x, m) => format!(
                "ty{x}:{}#{}",
                IDS[sub % IDS.len()],
                schema.types[x].member_name(m)
            ),
        };
        let text = format!("ty{ti}:{}#rel{ri}@{subject}", IDS[res % IDS.len()]);
        if let Ok(rel) = text.parse::<Relationship>()
            && !out.contains(&rel)
        {
            out.push(rel);
        }
    }
    out
}

/// A query to resolve against the oracle's universe (`docs/07-testing.md` §2.2):
/// `resource` and `member` pick an object and one of its type's members; the subject is
/// an object, a subject set, or (`from_set`) an element of the answer set, so both `true`
/// and `false` answers are common.
#[derive(Clone, Copy, Debug)]
pub struct RawQuery {
    /// Picks the resource from the universe.
    pub resource: usize,
    /// Picks the member on the resource's type.
    pub member: usize,
    /// Picks the subject.
    pub subject: usize,
    /// Picks a subject set member for the subject (when not `from_set`).
    pub subject_member: usize,
    /// 0: an object subject; 1: a subject set; 2: an element of `S(resource, member)`.
    pub kind: u8,
}

/// A schema, data valid for it, and queries.
#[derive(Clone)]
pub struct Case {
    /// The schema.
    pub schema: SchemaModel,
    /// Distinct tuples valid for the schema.
    pub tuples: Vec<Relationship>,
    /// Queries to resolve against the oracle's universe.
    pub queries: Vec<RawQuery>,
}

impl fmt::Debug for Case {
    // Readable when proptest prints a shrunk failure: the source and the tuples as text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Case {{\nschema:\n{}tuples:", self.schema.source())?;
        for t in &self.tuples {
            writeln!(f, "    {t}")?;
        }
        writeln!(f, "queries: {:?}\n}}", self.queries)
    }
}

/// A [`Case`] with 0–40 tuples and 1–24 queries, over [`schema`].
pub fn case() -> impl Strategy<Value = Case> {
    case_with(SubSides::Acyclic)
}

/// A [`Case`] with 0–40 tuples and 1–24 queries, over [`schema_with`]`(mode)`.
pub fn case_with(mode: SubSides) -> impl Strategy<Value = Case> {
    let query = (
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
        0u8..3,
    )
        .prop_map(
            |(resource, member, subject, subject_member, kind)| RawQuery {
                resource,
                member,
                subject,
                subject_member,
                kind,
            },
        );
    (
        schema_with(mode),
        prop::collection::vec(
            (
                any::<usize>(),
                any::<usize>(),
                any::<usize>(),
                0..IDS.len(),
                0..IDS.len(),
            ),
            0..=40,
        ),
        prop::collection::vec(query, 1..=24),
    )
        .prop_map(|(schema, raw, queries)| Case {
            tuples: tuples(&schema, &raw),
            schema,
            queries,
        })
}

// ---- store op sequences (property 5) ----

/// Resource types, relations and subjects of [`store_ops`] tuples: no schema, since stores
/// do not validate, but every kind of subject (object, wildcard, subject set) and ids that
/// sort differently byte-wise than by eye (`B` before `a`).
const STORE_RESOURCE_TYPES: [&str; 2] = ["doc", "folder"];
const STORE_RESOURCE_IDS: [&str; 2] = ["a", "B"];
const STORE_RELATIONS: [&str; 2] = ["viewer", "parent"];
const STORE_SUBJECTS: [&str; 5] = [
    "user:Y",
    "user:*",
    "group:g#member",
    "group:g#admin",
    "folder:a",
];

/// One step of a store op sequence.
#[derive(Clone, Debug)]
pub enum StoreOp {
    /// `WritableStore::write`.
    Write(pemba_core::model::WriteRequest),
    /// `WritableStore::write_schema` with a source and hash made from the number.
    Schema(u8),
    /// `WritableStore::gc` at [`StoreOp::gc_revision`]: never above head, and at head
    /// half the time so that recent deletes are collected.
    Gc(u8),
}

impl StoreOp {
    /// The `keep_from` a [`StoreOp::Gc`] uses with the store at `head`.
    pub fn gc_revision(
        fraction: u8,
        head: pemba_core::model::Revision,
    ) -> pemba_core::model::Revision {
        let f = u64::from(fraction).saturating_mul(2).min(255);
        pemba_core::model::Revision(head.0 * f / 255)
    }
}

/// Every relationship [`store_ops`] can mention.
pub fn store_tuples() -> Vec<Relationship> {
    let mut out = Vec::new();
    for t in STORE_RESOURCE_TYPES {
        for id in STORE_RESOURCE_IDS {
            for r in STORE_RELATIONS {
                for s in STORE_SUBJECTS {
                    if let Ok(rel) = format!("{t}:{id}#{r}@{s}").parse() {
                        out.push(rel);
                    }
                }
            }
        }
    }
    out
}

fn store_filter() -> impl Strategy<Value = pemba_core::model::RelationshipFilter> {
    let pick = |n: usize| prop::option::of(0..n);
    (
        0..STORE_RESOURCE_TYPES.len(),
        pick(STORE_RESOURCE_IDS.len()),
        pick(STORE_RELATIONS.len()),
        pick(STORE_SUBJECTS.len()),
        pick(STORE_SUBJECTS.len()),
        prop::option::of(0..3usize),
    )
        .prop_filter_map("names always parse", |(t, id, rel, st, sid, srel)| {
            let subject = |i: usize| STORE_SUBJECTS[i].split_once(':');
            Some(pemba_core::model::RelationshipFilter {
                resource_type: STORE_RESOURCE_TYPES[t].parse().ok()?,
                resource_id: id.map(|i| STORE_RESOURCE_IDS[i].parse()).transpose().ok()?,
                relation: rel.map(|i| STORE_RELATIONS[i].parse()).transpose().ok()?,
                subject_type: st
                    .and_then(subject)
                    .map(|(t, _)| t.parse())
                    .transpose()
                    .ok()?,
                subject_id: sid
                    .and_then(subject)
                    .map(|(_, id)| id.split('#').next().unwrap_or(id).to_owned()),
                subject_relation: srel.map(|i| match i {
                    0 => None,
                    1 => "member".parse().ok(),
                    _ => "admin".parse().ok(),
                }),
            })
        })
}

fn store_write() -> impl Strategy<Value = pemba_core::model::WriteRequest> {
    use pemba_core::model::{Precondition, WriteOp, WriteRequest};
    let n = store_tuples().len();
    // Weighted towards deletes so that GC finds deleted rows to remove.
    let op = (0u8..5, 0..n);
    let pre = (any::<bool>(), store_filter()).prop_map(|(must_exist, f)| {
        if must_exist {
            Precondition::MustExist(f)
        } else {
            Precondition::MustNotExist(f)
        }
    });
    (
        prop::collection::vec(op, 0..=6),
        prop::option::weighted(0.3, pre).prop_map(|p| p.into_iter().collect::<Vec<_>>()),
    )
        .prop_map(|(ops, preconditions)| {
            let tuples = store_tuples();
            WriteRequest {
                ops: ops
                    .into_iter()
                    .map(|(kind, i)| {
                        let r = tuples[i].clone();
                        match kind {
                            0 => WriteOp::Create(r),
                            1 | 2 => WriteOp::Touch(r),
                            _ => WriteOp::Delete(r),
                        }
                    })
                    .collect(),
                preconditions,
            }
        })
}

/// 1–16 store operations: mostly write batches (creates, touches, deletes, preconditions,
/// sometimes duplicates or conflicts), with the odd schema write and GC.
pub fn store_ops() -> impl Strategy<Value = Vec<StoreOp>> {
    let op = prop_oneof![
        12 => store_write().prop_map(StoreOp::Write),
        1 => any::<u8>().prop_map(StoreOp::Schema),
        3 => any::<u8>().prop_map(StoreOp::Gc),
    ];
    prop::collection::vec(op, 1..=16)
}
