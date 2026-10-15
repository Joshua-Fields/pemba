//! The reference evaluator, or oracle (`docs/07-testing.md` §2.1).
//!
//! Deliberately naive: it materialises `S(o, m)` (`docs/04-evaluation.md` §2) as an explicit
//! set of subjects for every object in a finite universe and every member, then answers
//! `check(o, m, s)` as `s ∈ S(o, m)`. It shares only the schema IR with the engine, never
//! evaluation code, so the two can be compared (property 1, §2.3).
//!
//! Sets are computed by **stratified** evaluation (OQ-020). Members `(type, member)` form
//! a dependency graph; edges reached through the subtracted side of an exclusion are
//! negative. Each strongly connected component is evaluated to a least fixed point
//! starting from ∅, after every component it depends on is final. A component containing a
//! negative edge has no such fixed point, and the oracle refuses the whole schema with
//! [`ReferenceError::NotStratifiable`] rather than answer. Plain Kleene iteration over all
//! members at once is wrong as soon as a recursive member sits on the base side of an
//! exclusion: it can stop at a non-least fixed point or never stop.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use warden_core::model::{ObjectId, ObjectRef, RelName, Relationship, SubjectRef, TypeName};
use warden_schema::ir::{AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, RelId, TypeId};

/// The extra object id every type's universe gets, standing for every object never
/// mentioned in the tuples (so wildcard semantics are tested on unseen subjects). A valid
/// [`ObjectId`] that generated data (`a`..`f`) never uses.
pub const FRESH_ID: &str = "__fresh__";

/// Why the oracle gives no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReferenceError {
    /// The schema has a cycle through the subtracted side of an exclusion, so stratified
    /// evaluation does not apply. Not a wrong answer and not `false`: "unsupported".
    NotStratifiable {
        /// The members of the offending component, as `type#member`, sorted.
        component: Vec<String>,
    },
    /// A tuple the schema does not allow, or one that uses [`FRESH_ID`].
    InvalidTuple {
        /// The tuple.
        tuple: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A query naming something undefined, a wildcard subject, or an object outside the
    /// universe.
    InvalidQuery {
        /// What is wrong with it.
        message: String,
    },
    /// A bug in the oracle itself.
    Internal {
        /// What went wrong.
        message: String,
    },
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStratifiable { component } => write!(
                f,
                "unsupported: schema is not stratifiable, a cycle through an exclusion's \
                 subtracted side involves {}",
                component.join(", ")
            ),
            Self::InvalidTuple { tuple, reason } => write!(f, "invalid tuple `{tuple}`: {reason}"),
            Self::InvalidQuery { message } => write!(f, "invalid query: {message}"),
            Self::Internal { message } => write!(f, "reference evaluator bug: {message}"),
        }
    }
}

impl std::error::Error for ReferenceError {}

/// A `(type, member)` pair: one node of the dependency graph.
type MemberKey = (TypeId, RelId);

/// One object's member: `S(type:id, member)`.
type Node = (TypeId, ObjectId, RelId);

/// An element of `S(o, m)`. Wildcards are expanded on the way in, so never appear.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Subject {
    Object(TypeId, ObjectId),
    Set(TypeId, ObjectId, RelId),
}

/// A stored subject, resolved against the schema.
#[derive(Clone, Debug)]
enum Stored {
    Object(TypeId, ObjectId),
    Set(TypeId, ObjectId, RelId),
    Wildcard(TypeId),
}

/// The oracle for one schema and one set of tuples, with every `S(o, m)` computed.
#[derive(Debug)]
pub struct Reference {
    schema: CompiledSchema,
    /// `U[t]`: every id of type `t` in the tuples, plus [`FRESH_ID`].
    universe: BTreeMap<TypeId, BTreeSet<ObjectId>>,
    /// `S(o, m)` for every object in the universe and every member of its type.
    sets: BTreeMap<Node, BTreeSet<Subject>>,
}

impl Reference {
    /// Stratifies `schema` and computes every `S(o, m)` over `tuples`.
    ///
    /// # Errors
    ///
    /// [`ReferenceError::NotStratifiable`] if the schema has a cycle through negation;
    /// [`ReferenceError::InvalidTuple`] if a tuple is not allowed by the schema or uses
    /// [`FRESH_ID`].
    pub fn new(schema: &CompiledSchema, tuples: &[Relationship]) -> Result<Self, ReferenceError> {
        Self::with_objects(schema, tuples, &[])
    }

    /// As [`new`](Self::new), with `objects` added to the universe although no tuple
    /// mentions them, so a query may name them (hand-written fixtures ask about subjects
    /// that appear in no tuple). Objects of undefined types are ignored; a query naming
    /// one fails as usual.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_objects(
        schema: &CompiledSchema,
        tuples: &[Relationship],
        objects: &[ObjectRef],
    ) -> Result<Self, ReferenceError> {
        let order = stratify(schema)?;
        let fresh = object_id(FRESH_ID)?;

        let mut universe: BTreeMap<TypeId, BTreeSet<ObjectId>> = schema
            .types
            .iter()
            .map(|t| (t.id, BTreeSet::from([fresh.clone()])))
            .collect();
        for o in objects {
            if let Some(ty) = schema.type_id(o.object_type.as_str()) {
                universe.entry(ty).or_default().insert(o.object_id.clone());
            }
        }
        let mut stored: BTreeMap<Node, Vec<Stored>> = BTreeMap::new();
        for tuple in tuples {
            let (node, subject) = resolve_tuple(schema, tuple)?;
            let mut mention = |ty: TypeId, id: &ObjectId| -> Result<(), ReferenceError> {
                if id.as_str() == FRESH_ID {
                    return Err(ReferenceError::InvalidTuple {
                        tuple: tuple.to_string(),
                        reason: format!("`{FRESH_ID}` is reserved for the oracle"),
                    });
                }
                universe.entry(ty).or_default().insert(id.clone());
                Ok(())
            };
            mention(node.0, &node.1)?;
            match &subject {
                Stored::Object(t, id) | Stored::Set(t, id, _) => mention(*t, id)?,
                Stored::Wildcard(_) => {}
            }
            stored.entry(node).or_default().push(subject);
        }

        let mut oracle = Self {
            schema: schema.clone(),
            universe,
            sets: BTreeMap::new(),
        };
        for component in &order {
            oracle.evaluate_component(component, &stored)?;
        }
        Ok(oracle)
    }

    /// `subject ∈ S(resource, member)`.
    ///
    /// # Errors
    ///
    /// [`ReferenceError::InvalidQuery`] if the resource type or member is undefined, the
    /// subject is a wildcard or names an undefined type or member, or the resource or the
    /// subject's object is outside the universe (two unseen ids are not interchangeable,
    /// so the oracle cannot stand in [`FRESH_ID`] for them).
    pub fn check(
        &self,
        resource: &ObjectRef,
        member: &RelName,
        subject: &SubjectRef,
    ) -> Result<bool, ReferenceError> {
        let node = self.query_node(resource, member)?;
        let subject = match subject {
            SubjectRef::Object(o) => {
                let ty = self.query_type(&o.object_type)?;
                self.in_universe(ty, o)?;
                Subject::Object(ty, o.object_id.clone())
            }
            SubjectRef::Set { object, relation } => {
                let (ty, id, rel) = self.query_node(object, relation)?;
                Subject::Set(ty, id, rel)
            }
            SubjectRef::Wildcard { .. } => {
                return Err(ReferenceError::InvalidQuery {
                    message: format!("wildcard subject `{subject}`"),
                });
            }
        };
        Ok(self.set(&node)?.contains(&subject))
    }

    /// Every subject in `S(resource, member)`, in a fixed order.
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check), for the resource and member.
    pub fn subjects(
        &self,
        resource: &ObjectRef,
        member: &RelName,
    ) -> Result<Vec<SubjectRef>, ReferenceError> {
        let node = self.query_node(resource, member)?;
        self.set(&node)?
            .iter()
            .map(|s| match s {
                Subject::Object(t, id) => Ok(SubjectRef::Object(self.object_ref(*t, id)?)),
                Subject::Set(t, id, r) => Ok(SubjectRef::Set {
                    object: self.object_ref(*t, id)?,
                    relation: self.rel_name(*t, *r)?,
                }),
            })
            .collect()
    }

    /// Every object in the universe, including one [`FRESH_ID`] object per type, in a
    /// fixed order.
    pub fn universe(&self) -> Vec<ObjectRef> {
        self.universe
            .iter()
            .flat_map(|(t, ids)| ids.iter().map(move |id| (*t, id)))
            .filter_map(|(t, id)| self.object_ref(t, id).ok())
            .collect()
    }

    /// Evaluates one strongly connected component to its least fixed point. Every
    /// component it depends on is already in `self.sets`; within it every edge is positive,
    /// so each recomputation can only grow the sets, and the loop ends.
    fn evaluate_component(
        &mut self,
        component: &[MemberKey],
        stored: &BTreeMap<Node, Vec<Stored>>,
    ) -> Result<(), ReferenceError> {
        let nodes: Vec<Node> = component
            .iter()
            .flat_map(|&(t, r)| {
                self.universe
                    .get(&t)
                    .into_iter()
                    .flatten()
                    .map(move |id| (t, id.clone(), r))
            })
            .collect();
        for node in &nodes {
            self.sets.insert(node.clone(), BTreeSet::new());
        }
        loop {
            let mut changed = false;
            for node in &nodes {
                let new = self.compute(node, stored)?;
                let old = self.set(node)?;
                if !old.is_subset(&new) {
                    return Err(internal(format!(
                        "{} shrank inside its own stratum",
                        self.node_name(node)
                    )));
                }
                if new.len() != old.len() {
                    self.sets.insert(node.clone(), new);
                    changed = true;
                }
            }
            if !changed {
                return Ok(());
            }
        }
    }

    /// One application of the semantics table (`docs/04-evaluation.md` §2) to `node`,
    /// from the current sets, plus self membership.
    fn compute(
        &self,
        node: &Node,
        stored: &BTreeMap<Node, Vec<Stored>>,
    ) -> Result<BTreeSet<Subject>, ReferenceError> {
        let (ty, id, rel) = node;
        let mut out = match self.member(*ty, *rel)? {
            Member::Relation(_) => {
                let mut out = BTreeSet::new();
                for s in stored.get(node).into_iter().flatten() {
                    match s {
                        Stored::Object(t, i) => {
                            out.insert(Subject::Object(*t, i.clone()));
                        }
                        Stored::Wildcard(t) => {
                            for i in self.universe.get(t).into_iter().flatten() {
                                out.insert(Subject::Object(*t, i.clone()));
                            }
                        }
                        Stored::Set(t, i, r) => {
                            out.extend(self.set(&(*t, i.clone(), *r))?.iter().cloned());
                        }
                    }
                }
                out
            }
            Member::Permission(p) => self.expr(*ty, id, &p.expr, stored)?,
        };
        out.insert(Subject::Set(*ty, id.clone(), *rel));
        Ok(out)
    }

    fn expr(
        &self,
        ty: TypeId,
        id: &ObjectId,
        expr: &Expr,
        stored: &BTreeMap<Node, Vec<Stored>>,
    ) -> Result<BTreeSet<Subject>, ReferenceError> {
        Ok(match expr {
            Expr::Computed(r) => self.set(&(ty, id.clone(), *r))?.clone(),
            Expr::Union(xs) => {
                let mut out = BTreeSet::new();
                for x in xs {
                    out.extend(self.expr(ty, id, x, stored)?);
                }
                out
            }
            Expr::Intersection(xs) => {
                let mut out: Option<BTreeSet<Subject>> = None;
                for x in xs {
                    let s = self.expr(ty, id, x, stored)?;
                    out = Some(match out {
                        None => s,
                        Some(acc) => acc.intersection(&s).cloned().collect(),
                    });
                }
                out.unwrap_or_default()
            }
            Expr::Exclusion { base, sub } => {
                let base = self.expr(ty, id, base, stored)?;
                let sub = self.expr(ty, id, sub, stored)?;
                base.difference(&sub).cloned().collect()
            }
            Expr::Arrow {
                tupleset,
                mode,
                targets,
                ..
            } => {
                // Distinct related objects: subject relation ignored, wildcards skipped.
                let related: BTreeSet<(TypeId, ObjectId)> = stored
                    .get(&(ty, id.clone(), *tupleset))
                    .into_iter()
                    .flatten()
                    .filter_map(|s| match s {
                        Stored::Object(t, i) | Stored::Set(t, i, _) => Some((*t, i.clone())),
                        Stored::Wildcard(_) => None,
                    })
                    .collect();
                let mut per_object = Vec::new();
                for (t, i) in related {
                    per_object.push(match targets.get(&t) {
                        Some(p) => self.set(&(t, i, *p))?.clone(),
                        None => BTreeSet::new(),
                    });
                }
                match mode {
                    ArrowMode::Any => per_object.into_iter().flatten().collect(),
                    // ∩ over zero objects is ∅, not "everything".
                    ArrowMode::All => {
                        let mut it = per_object.into_iter();
                        match it.next() {
                            None => BTreeSet::new(),
                            Some(first) => {
                                it.fold(first, |acc, s| acc.intersection(&s).cloned().collect())
                            }
                        }
                    }
                }
            }
        })
    }

    fn set(&self, node: &Node) -> Result<&BTreeSet<Subject>, ReferenceError> {
        self.sets.get(node).ok_or_else(|| {
            internal(format!(
                "{} read before its stratum was evaluated",
                self.node_name(node)
            ))
        })
    }

    fn member(&self, ty: TypeId, rel: RelId) -> Result<&Member, ReferenceError> {
        self.schema
            .type_def(ty)
            .and_then(|t| t.member(rel))
            .ok_or_else(|| internal(format!("member {ty:?}/{rel:?} not in schema")))
    }

    fn query_type(&self, name: &TypeName) -> Result<TypeId, ReferenceError> {
        self.schema
            .type_id(name.as_str())
            .ok_or_else(|| invalid_query(format!("type `{name}` is not defined")))
    }

    fn query_node(&self, object: &ObjectRef, member: &RelName) -> Result<Node, ReferenceError> {
        let ty = self.query_type(&object.object_type)?;
        let rel = self
            .schema
            .type_def(ty)
            .and_then(|t| t.member_id(member.as_str()))
            .ok_or_else(|| {
                invalid_query(format!("`{}` has no member `{member}`", object.object_type))
            })?;
        self.in_universe(ty, object)?;
        Ok((ty, object.object_id.clone(), rel))
    }

    fn in_universe(&self, ty: TypeId, object: &ObjectRef) -> Result<(), ReferenceError> {
        if self
            .universe
            .get(&ty)
            .is_some_and(|ids| ids.contains(&object.object_id))
        {
            Ok(())
        } else {
            Err(invalid_query(format!(
                "`{object}` is outside the universe; use `{}:{FRESH_ID}` for an unseen object",
                object.object_type
            )))
        }
    }

    fn object_ref(&self, ty: TypeId, id: &ObjectId) -> Result<ObjectRef, ReferenceError> {
        let def = self
            .schema
            .type_def(ty)
            .ok_or_else(|| internal(format!("type {ty:?} not in schema")))?;
        Ok(ObjectRef {
            object_type: def
                .name
                .parse()
                .map_err(|e| internal(format!("type name: {e}")))?,
            object_id: id.clone(),
        })
    }

    fn rel_name(&self, ty: TypeId, rel: RelId) -> Result<RelName, ReferenceError> {
        self.member(ty, rel)?
            .name()
            .parse()
            .map_err(|e| internal(format!("member name: {e}")))
    }

    fn node_name(&self, (ty, id, rel): &Node) -> String {
        format!(
            "{}:{id}#{}",
            type_name(&self.schema, *ty),
            member_name(&self.schema, (*ty, *rel))
        )
    }
}

/// The schema's strata in evaluation order: each inner list is one strongly connected
/// component of the member dependency graph, as `type#member` names, and depends only on
/// itself and earlier ones.
///
/// # Errors
///
/// [`ReferenceError::NotStratifiable`] if a component contains a negative edge.
pub fn strata(schema: &CompiledSchema) -> Result<Vec<Vec<String>>, ReferenceError> {
    Ok(stratify(schema)?
        .iter()
        .map(|c| c.iter().map(|&k| member_name(schema, k)).collect())
        .collect())
}

/// One dependency: `from` reads `to`; `negative` if reached through a subtracted side.
struct Edge {
    from: MemberKey,
    to: MemberKey,
    negative: bool,
}

/// The member dependency graph (OQ-020):
/// - a permission depends on every member its expression computes;
/// - an arrow `t->p` depends on the tupleset `t` and on `(X, p)` for every type `X`
///   allowed on `t` that has `p`;
/// - a relation allowing the subject set `x#y` depends on `(x, y)`;
/// - every edge reached through the `sub` side of an exclusion, at any depth, is negative.
fn edges(schema: &CompiledSchema) -> Vec<Edge> {
    fn expr_edges(ty: TypeId, from: MemberKey, e: &Expr, negative: bool, out: &mut Vec<Edge>) {
        let mut add = |to| out.push(Edge { from, to, negative });
        match e {
            Expr::Computed(r) => add((ty, *r)),
            Expr::Arrow {
                tupleset, targets, ..
            } => {
                add((ty, *tupleset));
                for (&x, &p) in targets {
                    add((x, p));
                }
            }
            Expr::Union(xs) | Expr::Intersection(xs) => {
                for x in xs {
                    expr_edges(ty, from, x, negative, out);
                }
            }
            Expr::Exclusion { base, sub } => {
                expr_edges(ty, from, base, negative, out);
                expr_edges(ty, from, sub, true, out);
            }
        }
    }

    let mut out = Vec::new();
    for key in all_members(schema) {
        match schema.type_def(key.0).and_then(|t| t.member(key.1)) {
            Some(Member::Relation(r)) => {
                for allowed in &r.allowed {
                    if let AllowedSubject::Set(x, y) = *allowed {
                        out.push(Edge {
                            from: key,
                            to: (x, y),
                            negative: false,
                        });
                    }
                }
            }
            Some(Member::Permission(p)) => expr_edges(key.0, key, &p.expr, false, &mut out),
            None => {}
        }
    }
    out
}

/// Strongly connected components in dependency order, or the first one (in member order)
/// that contains a negative edge.
///
/// Written for obviousness: reachability by a search from every member, components as
/// sets of mutually reachable members, then repeatedly emitting a component whose
/// dependencies are all emitted.
fn stratify(schema: &CompiledSchema) -> Result<Vec<Vec<MemberKey>>, ReferenceError> {
    let members = all_members(schema);
    let edges = edges(schema);
    let mut succ: BTreeMap<MemberKey, BTreeSet<MemberKey>> =
        members.iter().map(|&m| (m, BTreeSet::new())).collect();
    for e in &edges {
        succ.entry(e.from).or_default().insert(e.to);
    }

    // reach[u] = every member reachable from u by one or more edges.
    let reach: BTreeMap<MemberKey, BTreeSet<MemberKey>> = members
        .iter()
        .map(|&start| {
            let mut seen = BTreeSet::new();
            let mut todo: Vec<MemberKey> =
                succ.get(&start).into_iter().flatten().copied().collect();
            while let Some(m) = todo.pop() {
                if seen.insert(m) {
                    todo.extend(succ.get(&m).into_iter().flatten().copied());
                }
            }
            (start, seen)
        })
        .collect();
    let reaches = |a: MemberKey, b: MemberKey| reach.get(&a).is_some_and(|s| s.contains(&b));
    let component_of = |m: MemberKey| -> Vec<MemberKey> {
        members
            .iter()
            .copied()
            .filter(|&n| n == m || (reaches(m, n) && reaches(n, m)))
            .collect()
    };

    // A negative edge u → v lies in a component iff v gets back to u.
    for e in edges.iter().filter(|e| e.negative) {
        if e.from == e.to || reaches(e.to, e.from) {
            let mut component: Vec<String> = component_of(e.from)
                .into_iter()
                .map(|k| member_name(schema, k))
                .collect();
            component.sort();
            return Err(ReferenceError::NotStratifiable { component });
        }
    }

    let mut components: Vec<Vec<MemberKey>> = Vec::new();
    for &m in &members {
        let c = component_of(m);
        if !components.contains(&c) {
            components.push(c);
        }
    }
    let mut done: BTreeSet<MemberKey> = BTreeSet::new();
    let mut order = Vec::new();
    while order.len() < components.len() {
        let ready = components.iter().find(|c| {
            !done.contains(&c[0])
                && c.iter().all(|m| {
                    succ.get(m)
                        .into_iter()
                        .flatten()
                        .all(|d| done.contains(d) || c.contains(d))
                })
        });
        let Some(c) = ready else {
            return Err(internal("component graph is not acyclic".to_owned()));
        };
        done.extend(c.iter().copied());
        order.push(c.clone());
    }
    Ok(order)
}

fn all_members(schema: &CompiledSchema) -> Vec<MemberKey> {
    schema
        .types
        .iter()
        .flat_map(|t| {
            t.members
                .iter()
                .filter_map(|m| t.member_id(m.name()))
                .map(move |r| (t.id, r))
        })
        .collect()
}

/// Resolves a tuple against the schema, checking it is one the schema allows.
fn resolve_tuple(
    schema: &CompiledSchema,
    tuple: &Relationship,
) -> Result<(Node, Stored), ReferenceError> {
    let bad = |reason: &str| ReferenceError::InvalidTuple {
        tuple: tuple.to_string(),
        reason: reason.to_owned(),
    };
    let ty = schema
        .type_id(tuple.resource.object_type.as_str())
        .ok_or_else(|| bad("resource type is not defined"))?;
    let def = schema
        .type_def(ty)
        .ok_or_else(|| bad("resource type is not defined"))?;
    let rel = def
        .member_id(tuple.relation.as_str())
        .ok_or_else(|| bad("relation is not defined"))?;
    let Some(Member::Relation(r)) = def.member(rel) else {
        return Err(bad("not a relation"));
    };
    let subject_type = |name: &TypeName| {
        schema
            .type_id(name.as_str())
            .ok_or_else(|| bad("subject type is not defined"))
    };
    let (allowed, stored) = match &tuple.subject {
        SubjectRef::Object(o) => {
            let t = subject_type(&o.object_type)?;
            (
                AllowedSubject::Object(t),
                Stored::Object(t, o.object_id.clone()),
            )
        }
        SubjectRef::Wildcard { object_type } => {
            let t = subject_type(object_type)?;
            (AllowedSubject::Wildcard(t), Stored::Wildcard(t))
        }
        SubjectRef::Set { object, relation } => {
            let t = subject_type(&object.object_type)?;
            let y = schema
                .type_def(t)
                .and_then(|d| d.member_id(relation.as_str()))
                .ok_or_else(|| bad("subject relation is not defined"))?;
            (
                AllowedSubject::Set(t, y),
                Stored::Set(t, object.object_id.clone(), y),
            )
        }
    };
    if !r.allowed.contains(&allowed) {
        return Err(bad("subject form is not allowed on the relation"));
    }
    Ok(((ty, tuple.resource.object_id.clone(), rel), stored))
}

fn type_name(schema: &CompiledSchema, ty: TypeId) -> String {
    schema
        .type_def(ty)
        .map_or_else(|| format!("{ty:?}"), |t| t.name.to_string())
}

fn member_name(schema: &CompiledSchema, (ty, rel): MemberKey) -> String {
    let member = schema
        .type_def(ty)
        .and_then(|t| t.member(rel))
        .map_or_else(|| format!("{rel:?}"), |m| m.name().to_owned());
    format!("{}#{member}", type_name(schema, ty))
}

fn object_id(s: &str) -> Result<ObjectId, ReferenceError> {
    s.parse()
        .map_err(|e| internal(format!("`{s}` is not an object id: {e}")))
}

fn invalid_query(message: String) -> ReferenceError {
    ReferenceError::InvalidQuery { message }
}

fn internal(message: String) -> ReferenceError {
    ReferenceError::Internal { message }
}
