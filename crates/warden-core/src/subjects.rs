//! `lookup_subjects` (`docs/04-evaluation.md` §6): which objects of one type have a
//! permission on one resource?
//!
//! [`Gatherer::gather`] evaluates `S(o, m)` forwards, restricted to object subjects of the
//! requested type, as a [`SubjectSet`]: a finite set of ids, or every id except a finite
//! set (a wildcard minus exclusions). Cycles are handled exactly as `check` handles them
//! (§3.6), by the shared [`crate::scc`] iteration with subject sets in place of booleans.

use std::collections::{BTreeSet, HashMap};

use warden_schema::ir::{AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, RelId, TypeId};

use crate::engine::read_error;
use crate::error::Error;
use crate::eval::{
    self, EvalStats, Limits, NodeKey, invalid, name, object_ref, rel_name, resolve_set,
};
use crate::model::{Consistency, ObjectId, ObjectRef, RelName, Revision, SubjectRef, TypeName};
use crate::scc::{Finish, Hit, Lattice, Outcome, Scc, min_low};
use crate::store::{ReadStore, StoreError};

/// Which objects of `subject_type` have `permission` on `resource`
/// (`docs/04-evaluation.md` §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupSubjectsRequest {
    /// The object asked about.
    pub resource: ObjectRef,
    /// A permission or relation of the resource's type.
    pub permission: RelName,
    /// The type of the subjects to find. Object subjects only: subject sets are not
    /// returned.
    pub subject_type: TypeName,
    /// Which revision to read at.
    pub consistency: Consistency,
}

impl LookupSubjectsRequest {
    /// A request at the latest revision.
    pub fn new(resource: ObjectRef, permission: RelName, subject_type: TypeName) -> Self {
        Self {
            resource,
            permission,
            subject_type,
            consistency: Consistency::Latest,
        }
    }
}

/// The answer to a [`LookupSubjectsRequest`]: `subject_ids` if `wildcard` is `false`,
/// otherwise every object of the subject type except `excluded_ids`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupSubjectsResponse {
    /// The subjects, ascending byte-wise; empty if `wildcard`.
    pub subject_ids: Vec<ObjectId>,
    /// `true`: every object of the subject type, including ones never written, except
    /// `excluded_ids`.
    pub wildcard: bool,
    /// With `wildcard`, the ids left out, ascending byte-wise; otherwise empty.
    pub excluded_ids: Vec<ObjectId>,
    /// The revision the answer was evaluated at.
    pub revision: Revision,
}

impl LookupSubjectsResponse {
    /// Whether `id` is in the answer.
    pub fn contains(&self, id: &ObjectId) -> bool {
        if self.wildcard {
            self.excluded_ids.binary_search(id).is_err()
        } else {
            self.subject_ids.binary_search(id).is_ok()
        }
    }
}

/// A set of object ids of one type (`docs/04-evaluation.md` §6.1): finite, or cofinite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SubjectSet {
    /// Exactly these ids.
    Ids(BTreeSet<ObjectId>),
    /// Every id except these (a wildcard minus exclusions).
    AllExcept(BTreeSet<ObjectId>),
}

use SubjectSet::{AllExcept, Ids};

impl SubjectSet {
    /// `Ids(∅)`.
    pub(crate) fn empty() -> Self {
        Ids(BTreeSet::new())
    }

    /// `AllExcept(∅)`: every id.
    pub(crate) fn all() -> Self {
        AllExcept(BTreeSet::new())
    }

    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, Ids(a) if a.is_empty())
    }

    /// `self ∪ other`.
    pub(crate) fn union(&self, other: &Self) -> Self {
        match (self, other) {
            (Ids(a), Ids(b)) => Ids(a | b),
            (Ids(a), AllExcept(y)) => AllExcept(y - a),
            (AllExcept(x), Ids(b)) => AllExcept(x - b),
            (AllExcept(x), AllExcept(y)) => AllExcept(x & y),
        }
    }

    /// `self ∩ other`.
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Ids(a), Ids(b)) => Ids(a & b),
            (Ids(a), AllExcept(y)) => Ids(a - y),
            (AllExcept(x), Ids(b)) => Ids(b - x),
            (AllExcept(x), AllExcept(y)) => AllExcept(x | y),
        }
    }

    /// `self \ other`.
    pub(crate) fn minus(&self, other: &Self) -> Self {
        match (self, other) {
            (Ids(a), Ids(b)) => Ids(a - b),
            (Ids(a), AllExcept(y)) => Ids(a & y),
            (AllExcept(x), Ids(b)) => AllExcept(x | b),
            (AllExcept(x), AllExcept(y)) => Ids(y - x),
        }
    }

    /// `self ⊆ other` (§6.2's order). A cofinite set is never inside a finite one: the
    /// universe of ids is unbounded.
    pub(crate) fn is_subset(&self, other: &Self) -> bool {
        match (self, other) {
            (Ids(a), Ids(b)) => a.is_subset(b),
            (Ids(a), AllExcept(x)) => a.is_disjoint(x),
            (AllExcept(_), Ids(_)) => false,
            (AllExcept(x), AllExcept(y)) => y.is_subset(x),
        }
    }

    /// `id ∈ self`.
    #[cfg(test)]
    pub(crate) fn contains(&self, id: &ObjectId) -> bool {
        match self {
            Ids(a) => a.contains(id),
            AllExcept(x) => !x.contains(id),
        }
    }
}

impl Lattice for SubjectSet {
    fn bottom() -> Self {
        Self::empty()
    }
    fn join(&mut self, other: &Self) {
        *self = self.union(other);
    }
    fn le(&self, other: &Self) -> bool {
        self.is_subset(other)
    }
    fn is_top(&self) -> bool {
        matches!(self, AllExcept(x) if x.is_empty())
    }
}

/// Validates a `lookup_subjects` request against the schema (`docs/04` §1 step 1).
///
/// # Errors
///
/// [`Error::InvalidRequest`] if the resource type, permission or subject type is not
/// defined.
pub(crate) fn resolve(
    schema: &CompiledSchema,
    req: &LookupSubjectsRequest,
) -> Result<(NodeKey, TypeId), Error> {
    let root = eval::node(schema, &req.resource, &req.permission, "")?;
    let subject_type = schema.type_id(req.subject_type.as_str()).ok_or_else(|| {
        invalid(format!(
            "subject type `{}` is not defined in the schema",
            req.subject_type
        ))
    })?;
    Ok((root, subject_type))
}

type SetOutcome = Outcome<SubjectSet>;

/// Evaluates `S(o, m)` restricted to objects of one type, at one revision (§6.2).
pub(crate) struct Gatherer<'a> {
    schema: &'a CompiledSchema,
    store: &'a dyn ReadStore,
    rev: Revision,
    limits: Limits,
    subject_type: TypeId,
    subject_type_name: &'a TypeName,
    /// Settled values.
    memo: HashMap<NodeKey, SubjectSet>,
    cycle: Scc<SubjectSet>,
    reads: u64,
    stats: EvalStats,
}

impl<'a> Gatherer<'a> {
    pub(crate) fn new(
        schema: &'a CompiledSchema,
        store: &'a dyn ReadStore,
        rev: Revision,
        limits: Limits,
        subject_type: (TypeId, &'a TypeName),
    ) -> Self {
        Self {
            schema,
            store,
            rev,
            limits,
            subject_type: subject_type.0,
            subject_type_name: subject_type.1,
            memo: HashMap::new(),
            cycle: Scc::new(),
            reads: 0,
            stats: EvalStats::default(),
        }
    }

    /// `S(root)`, restricted to the subject type.
    pub(crate) fn gather(&mut self, root: &NodeKey) -> Result<SubjectSet, Error> {
        let result = self.eval(root, 1).map(|o| o.value);
        self.cycle.reset();
        result
    }

    /// Counters so far; `elapsed` is left to the caller.
    pub(crate) fn stats(&self) -> EvalStats {
        self.stats
    }

    fn eval(&mut self, key: &NodeKey, depth: u32) -> Result<SetOutcome, Error> {
        self.stats.max_depth_reached = self.stats.max_depth_reached.max(depth);
        if depth > self.limits.max_depth {
            let mut path: Vec<String> = self
                .cycle
                .path_from(0)
                .map(|k| name(self.schema, k))
                .collect();
            path.push(name(self.schema, key));
            return Err(Error::MaxDepthExceeded { path });
        }
        if let Some(value) = self.memo.get(key) {
            self.stats.memo_hits += 1;
            return Ok(Outcome::settled(value.clone()));
        }
        match self.cycle.hit(key)? {
            Some(Hit::Found(out)) => return Ok(out),
            Some(Hit::NonMonotonic { from }) => {
                let mut path: Vec<String> = self
                    .cycle
                    .path_from(from)
                    .map(|k| name(self.schema, k))
                    .collect();
                path.push(name(self.schema, key));
                return Err(Error::NonMonotonicCycle { path });
            }
            None => {}
        }

        let schema = self.schema;
        let member = schema
            .type_def(key.ty)
            .and_then(|t| t.member(key.rel))
            .ok_or_else(|| invalid(format!("internal: node {key:?} is not in the schema")))?;
        let p = self.cycle.begin(key);
        let mut passes = 0;
        loop {
            passes += 1;
            self.cycle.push_frame(key);
            let out = match member {
                Member::Relation(r) => self.eval_direct(key, &r.allowed, depth),
                Member::Permission(perm) => {
                    let mut fanout = 0;
                    self.eval_expr(key, &perm.expr, depth, &mut fanout)
                }
            };
            self.cycle.pop_frame();
            match self.cycle.finish(key, p, out?, &mut self.memo) {
                Finish::Done(out) => {
                    if out.low.is_none() && passes > 1 {
                        self.stats.cycle_passes += passes;
                    }
                    return Ok(out);
                }
                Finish::Rerun => {}
            }
        }
    }

    /// A relation's stored subjects: objects of the subject type, its wildcard if the
    /// relation allows it (as `check`), and every stored subject set's members.
    fn eval_direct(
        &mut self,
        key: &NodeKey,
        allowed: &[AllowedSubject],
        depth: u32,
    ) -> Result<SetOutcome, Error> {
        let resource = object_ref(self.schema, key);
        let relation = rel_name(self.schema, key.ty, key.rel);
        let subjects = self.read(|s, rev| s.read_tuples(rev, &resource, &relation))?;
        self.count_tuples(subjects.len());
        self.check_fanout(key, subjects.len())?;
        let wildcard_allowed = allowed.contains(&AllowedSubject::Wildcard(self.subject_type));
        let mut ids = BTreeSet::new();
        let mut sets = Vec::new();
        let mut all = false;
        for subject in subjects {
            match subject {
                SubjectRef::Object(o) if o.object_type == *self.subject_type_name => {
                    ids.insert(o.object_id);
                }
                SubjectRef::Object(_) => {}
                SubjectRef::Wildcard { object_type } => {
                    all |= object_type == *self.subject_type_name && wildcard_allowed;
                }
                SubjectRef::Set {
                    object,
                    relation: r,
                } => sets.push(resolve_set(
                    self.schema,
                    key,
                    &resource,
                    &relation,
                    object,
                    r,
                    allowed,
                )?),
            }
        }
        let mut value = if all { SubjectSet::all() } else { Ids(ids) };
        let mut low = None;
        for child in sets {
            let out = self.eval(&child, depth + 1)?;
            low = min_low(low, out.low);
            value.join(&out.value);
        }
        Ok(Outcome { value, low })
    }

    /// A permission expression on `key`'s object.
    fn eval_expr(
        &mut self,
        key: &NodeKey,
        expr: &Expr,
        depth: u32,
        fanout: &mut usize,
    ) -> Result<SetOutcome, Error> {
        match expr {
            Expr::Computed(rel) => self.eval(
                &NodeKey {
                    ty: key.ty,
                    id: key.id.clone(),
                    rel: *rel,
                },
                depth + 1,
            ),
            Expr::Union(operands) => {
                let mut value = SubjectSet::empty();
                let mut low = None;
                for e in operands {
                    let out = self.eval_expr(key, e, depth, fanout)?;
                    low = min_low(low, out.low);
                    value.join(&out.value);
                }
                Ok(Outcome { value, low })
            }
            Expr::Intersection(operands) => {
                let mut value: Option<SubjectSet> = None;
                let mut low = None;
                for e in operands {
                    let out = self.eval_expr(key, e, depth, fanout)?;
                    low = min_low(low, out.low);
                    let v = match value {
                        Some(v) => v.intersect(&out.value),
                        None => out.value,
                    };
                    let empty = v.is_empty();
                    value = Some(v);
                    if empty {
                        break;
                    }
                }
                Ok(Outcome {
                    value: value.unwrap_or_else(SubjectSet::empty),
                    low,
                })
            }
            Expr::Exclusion { base, sub } => {
                let b = self.eval_expr(key, base, depth, fanout)?;
                if b.value.is_empty() {
                    return Ok(b);
                }
                self.cycle.enter_sub();
                let s = self.eval_expr(key, sub, depth, fanout);
                self.cycle.leave_sub();
                let s = s?;
                // As in `check`: the subtracted side is always settled; negating an
                // unsettled value would be unsound, so fail closed.
                if s.low.is_some() {
                    return Err(invalid(format!(
                        "internal: unsettled value negated at {}",
                        name(self.schema, key)
                    )));
                }
                Ok(Outcome {
                    value: b.value.minus(&s.value),
                    low: b.low,
                })
            }
            Expr::Arrow {
                tupleset,
                mode,
                targets,
                ..
            } => {
                let resource = object_ref(self.schema, key);
                let relation = rel_name(self.schema, key.ty, *tupleset);
                let objects =
                    self.read(|s, rev| s.read_related_objects(rev, &resource, &relation))?;
                self.count_tuples(objects.len());
                *fanout = fanout.saturating_add(objects.len());
                self.check_fanout(key, *fanout)?;
                match mode {
                    ArrowMode::Any => self.arrow_any(&objects, targets, depth),
                    ArrowMode::All => self.arrow_all(&objects, targets, depth),
                }
            }
        }
    }

    fn arrow_any(
        &mut self,
        objects: &[ObjectRef],
        targets: &std::collections::BTreeMap<TypeId, RelId>,
        depth: u32,
    ) -> Result<SetOutcome, Error> {
        let mut value = SubjectSet::empty();
        let mut low = None;
        for x in objects {
            let Some(child) = self.arrow_target(x, targets) else {
                continue;
            };
            let out = self.eval(&child, depth + 1)?;
            low = min_low(low, out.low);
            value.join(&out.value);
        }
        Ok(Outcome { value, low })
    }

    /// Intersection over related objects: `Ids(∅)` over none, or as soon as one object's
    /// type lacks the target.
    fn arrow_all(
        &mut self,
        objects: &[ObjectRef],
        targets: &std::collections::BTreeMap<TypeId, RelId>,
        depth: u32,
    ) -> Result<SetOutcome, Error> {
        let mut value = SubjectSet::all();
        let mut low = None;
        if objects.is_empty() {
            return Ok(Outcome::settled(SubjectSet::empty()));
        }
        for x in objects {
            let Some(child) = self.arrow_target(x, targets) else {
                return Ok(Outcome {
                    value: SubjectSet::empty(),
                    low,
                });
            };
            let out = self.eval(&child, depth + 1)?;
            low = min_low(low, out.low);
            value = value.intersect(&out.value);
            if value.is_empty() {
                break;
            }
        }
        Ok(Outcome { value, low })
    }

    fn arrow_target(
        &self,
        x: &ObjectRef,
        targets: &std::collections::BTreeMap<TypeId, RelId>,
    ) -> Option<NodeKey> {
        let ty = self.schema.type_id(x.object_type.as_str())?;
        let rel = *targets.get(&ty)?;
        Some(NodeKey {
            ty,
            id: x.object_id.clone(),
            rel,
        })
    }

    fn check_fanout(&self, key: &NodeKey, read: usize) -> Result<(), Error> {
        if read > self.limits.max_fanout {
            return Err(Error::FanoutExceeded {
                node: name(self.schema, key),
                max: self.limits.max_fanout,
            });
        }
        Ok(())
    }

    /// One store read, counted against `max_check_reads` for the whole request.
    fn read<T>(
        &mut self,
        f: impl FnOnce(&dyn ReadStore, Revision) -> Result<T, StoreError>,
    ) -> Result<T, Error> {
        self.reads += 1;
        if self.reads > self.limits.max_reads {
            return Err(Error::CheckReadsExceeded {
                max: self.limits.max_reads,
            });
        }
        self.stats.store_reads += 1;
        f(self.store, self.rev).map_err(read_error)
    }

    fn count_tuples(&mut self, n: usize) {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        self.stats.tuples_read = self.stats.tuples_read.saturating_add(n);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use proptest::prelude::*;

    use super::*;

    fn set(ids: &[&str]) -> BTreeSet<ObjectId> {
        ids.iter().map(|s| s.parse().unwrap()).collect()
    }

    fn ids(v: &[&str]) -> SubjectSet {
        Ids(set(v))
    }

    fn except(v: &[&str]) -> SubjectSet {
        AllExcept(set(v))
    }

    // ---- the 12 cells of §6.1, operands {a, b} and {b, c} ----

    #[test]
    fn union_ids_ids() {
        assert_eq!(
            ids(&["a", "b"]).union(&ids(&["b", "c"])),
            ids(&["a", "b", "c"])
        );
    }
    #[test]
    fn union_ids_all_except() {
        assert_eq!(ids(&["a", "b"]).union(&except(&["b", "c"])), except(&["c"]));
    }
    #[test]
    fn union_all_except_ids() {
        assert_eq!(except(&["a", "b"]).union(&ids(&["b", "c"])), except(&["a"]));
    }
    #[test]
    fn union_all_except_all_except() {
        assert_eq!(
            except(&["a", "b"]).union(&except(&["b", "c"])),
            except(&["b"])
        );
    }
    #[test]
    fn intersect_ids_ids() {
        assert_eq!(ids(&["a", "b"]).intersect(&ids(&["b", "c"])), ids(&["b"]));
    }
    #[test]
    fn intersect_ids_all_except() {
        assert_eq!(
            ids(&["a", "b"]).intersect(&except(&["b", "c"])),
            ids(&["a"])
        );
    }
    #[test]
    fn intersect_all_except_ids() {
        assert_eq!(
            except(&["a", "b"]).intersect(&ids(&["b", "c"])),
            ids(&["c"])
        );
    }
    #[test]
    fn intersect_all_except_all_except() {
        assert_eq!(
            except(&["a", "b"]).intersect(&except(&["b", "c"])),
            except(&["a", "b", "c"])
        );
    }
    #[test]
    fn minus_ids_ids() {
        assert_eq!(ids(&["a", "b"]).minus(&ids(&["b", "c"])), ids(&["a"]));
    }
    #[test]
    fn minus_ids_all_except() {
        assert_eq!(ids(&["a", "b"]).minus(&except(&["b", "c"])), ids(&["b"]));
    }
    #[test]
    fn minus_all_except_ids() {
        assert_eq!(
            except(&["a", "b"]).minus(&ids(&["b", "c"])),
            except(&["a", "b", "c"])
        );
    }
    #[test]
    fn minus_all_except_all_except() {
        assert_eq!(except(&["a", "b"]).minus(&except(&["b", "c"])), ids(&["c"]));
    }

    #[test]
    fn order_examples() {
        assert!(ids(&["a"]).is_subset(&ids(&["a", "b"])));
        assert!(!ids(&["a", "c"]).is_subset(&ids(&["a", "b"])));
        assert!(ids(&["a"]).is_subset(&except(&["b"])));
        assert!(!ids(&["a"]).is_subset(&except(&["a"])));
        assert!(!except(&[]).is_subset(&ids(&["a", "b"])));
        assert!(except(&["a", "b"]).is_subset(&except(&["a"])));
        assert!(!except(&["a"]).is_subset(&except(&["a", "b"])));
        assert!(SubjectSet::all().is_top());
        assert!(!except(&["a"]).is_top());
    }

    // ---- the algebra against a finite model ----

    /// Ids drawn from `0..=8`; the model universe adds `9`, which no set ever names, so a
    /// cofinite set always contains it and a finite one never does, as in the unbounded
    /// universe.
    const UNIVERSE: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

    fn model(s: &SubjectSet) -> BTreeSet<&'static str> {
        UNIVERSE
            .iter()
            .copied()
            .filter(|u| s.contains(&u.parse().unwrap()))
            .collect()
    }

    fn subject_set() -> impl Strategy<Value = SubjectSet> {
        (any::<bool>(), prop::collection::btree_set(0..9usize, 0..=9)).prop_map(
            |(cofinite, members)| {
                let members = members
                    .iter()
                    .map(|i| UNIVERSE[*i].parse().unwrap())
                    .collect();
                if cofinite {
                    AllExcept(members)
                } else {
                    Ids(members)
                }
            },
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        #[test]
        fn algebra_matches_the_model(a in subject_set(), b in subject_set()) {
            let (ma, mb) = (model(&a), model(&b));
            prop_assert_eq!(model(&a.union(&b)), &ma | &mb);
            prop_assert_eq!(model(&a.intersect(&b)), &ma & &mb);
            prop_assert_eq!(model(&a.minus(&b)), &ma - &mb);
        }

        #[test]
        fn order_matches_the_model(a in subject_set(), b in subject_set()) {
            prop_assert_eq!(a.le(&b), model(&a).is_subset(&model(&b)));
            prop_assert_eq!(a.is_top(), model(&a).len() == UNIVERSE.len());
            // Join is the least upper bound in this order.
            let mut j = a.clone();
            j.join(&b);
            prop_assert!(a.le(&j) && b.le(&j));
        }
    }
}
