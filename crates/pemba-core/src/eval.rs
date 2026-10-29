//! The `check` evaluator (`docs/04-evaluation.md` §2–4).
//!
//! `check(o, m, s)` decides `s ∈ S(o, m)` by recursing over [`NodeKey`]s `(type, id,
//! member)` for one fixed subject at one revision. Relations read the store
//! ([`Evaluator::eval_direct`]); permissions evaluate their expression left to right with
//! short-circuiting ([`Evaluator::eval_expr`]).
//!
//! Data-dependent cycles (§3.6) are evaluated by fixed-point iteration over strongly
//! connected regions, Tarjan-style. A node stays on the SCC stack from its first visit
//! until the head of its region settles; a result's `low` is the lowest SCC-stack position
//! whose not-yet-settled value it used. A hit on a node in progress returns its provisional
//! value (`false` on the first pass); a hit on a finished member returns its value from
//! this pass. When the head finishes, the region is settled into the memo if every
//! assumption held (or the head is `true`), else the head re-runs with the values found.
//! Values only grow, so this ends, at the least fixed point.
//!
//! Negation is stratified: every frame records its negation depth, the number of
//! subtracted sides it sits inside. A cycle whose path from the repeated node to the
//! closing edge crosses a subtracted side has no least fixed point, so it is an error; it
//! is exactly the cycle whose ends differ in depth, since depth never falls along the
//! stack. A monotone cycle wholly inside a subtracted side settles like any other region,
//! before the exclusion negates its value.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use pemba_schema::ir::{AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, RelId, TypeId};

use crate::engine::read_error;
use crate::error::Error;
use crate::model::{
    Consistency, ObjectId, ObjectRef, RelName, Relationship, Revision, SubjectRef, TypeName,
};
use crate::scc::{self, Finish, Hit, Scc, min_low};
use crate::store::{ReadStore, StoreError};
use crate::trace::{NoTrace, TraceRole, Tracer};

/// Asks whether `subject` has `permission` on `resource` (`docs/04-evaluation.md` §3.1).
///
/// Use [`Self::new`] and set public fields; downstream struct literals are unsupported.
///
/// ```compile_fail
/// use pemba_core::CheckRequest;
/// use pemba_core::model::Consistency;
/// let request = CheckRequest {
///     resource: "document:x".parse().unwrap(),
///     permission: "view".parse().unwrap(),
///     subject: "user:a".parse().unwrap(),
///     consistency: Consistency::Latest,
/// };
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CheckRequest {
    /// The object the permission is on.
    pub resource: ObjectRef,
    /// A relation or permission defined on the resource's type.
    pub permission: RelName,
    /// An object or a subject set; a wildcard is rejected with `InvalidRequest`.
    pub subject: SubjectRef,
    /// Which revision to evaluate at.
    pub consistency: Consistency,
}

impl CheckRequest {
    /// A check at the latest revision. Set `consistency` for a different revision.
    pub fn new(resource: ObjectRef, permission: RelName, subject: SubjectRef) -> Self {
        Self {
            resource,
            permission,
            subject,
            consistency: Consistency::Latest,
        }
    }
}

/// The answer to a [`CheckRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckResponse {
    /// Whether the subject has the permission.
    pub allowed: bool,
    /// The revision the check was evaluated at.
    pub revision: Revision,
    /// What the evaluation cost.
    pub stats: EvalStats,
}

/// One question in a `check_bulk` call: a [`CheckRequest`] without its consistency,
/// which the call sets once for every item.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CheckItem {
    /// The object the permission is on.
    pub resource: ObjectRef,
    /// A relation or permission defined on the resource's type.
    pub permission: RelName,
    /// An object or a subject set; a wildcard fails this item with `InvalidRequest`.
    pub subject: SubjectRef,
}

impl CheckItem {
    /// One bulk-check question; the bulk call supplies its consistency.
    pub fn new(resource: ObjectRef, permission: RelName, subject: SubjectRef) -> Self {
        Self {
            resource,
            permission,
            subject,
        }
    }
}

/// Counters for one read request, also recorded on its tracing span
/// (`docs/04-evaluation.md` §1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EvalStats {
    /// Store calls made.
    pub store_reads: u64,
    /// Tuples those calls returned; a point query that finds its tuple counts one.
    pub tuples_read: u64,
    /// Nodes answered from the memo instead of being evaluated.
    pub memo_hits: u64,
    /// The deepest nested evaluation reached; the root node is depth 1.
    pub max_depth_reached: u32,
    /// Passes made over cyclic regions whose head had to re-run because a provisional value
    /// changed (`docs/04-evaluation.md` §3.6), counting each such head's first pass. A cycle
    /// settled in its first pass adds nothing.
    pub cycle_passes: u64,
    /// Wall-clock time spent in the request.
    pub elapsed: Duration,
}

impl EvalStats {
    /// Adds `other`'s counters to these, keeping the larger depth.
    pub(crate) fn absorb(&mut self, other: &Self) {
        self.store_reads += other.store_reads;
        self.tuples_read += other.tuples_read;
        self.memo_hits += other.memo_hits;
        self.cycle_passes += other.cycle_passes;
        self.max_depth_reached = self.max_depth_reached.max(other.max_depth_reached);
    }
}

/// A member of one object: `S(type:id, rel)`. The subject is fixed per evaluator, so it is
/// not part of the key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NodeKey {
    pub(crate) ty: TypeId,
    pub(crate) id: ObjectId,
    pub(crate) rel: RelId,
}

/// Settled node values (least fixed point) for one subject at one revision. Safe to share between
/// checks with the same subject and revision (`check_bulk`, lookups).
///
/// For a subject-set subject, a node's value is the same wherever it is evaluated, but
/// whether evaluating it must fail is not: under the subtracted side of an exclusion a
/// stored wildcard of the subject's type is an error (`AMBIGUOUS_WILDCARD_EXCLUSION`,
/// `docs/04-evaluation.md` §3.3), elsewhere it is not consulted. So values computed under a
/// subtracted side are kept apart: they serve any later lookup, while values computed
/// outside one serve only lookups outside one, never hiding that error.
#[derive(Debug, Default)]
pub(crate) struct Memo {
    /// Values computed outside any subtracted side.
    plain: HashMap<NodeKey, bool>,
    /// Values computed under at least one subtracted side, for a subject-set subject.
    negated: HashMap<NodeKey, bool>,
}

impl Memo {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The settled value usable outside any subtracted side.
    #[cfg(test)]
    pub(crate) fn get(&self, key: &NodeKey) -> Option<&bool> {
        self.plain.get(key).or_else(|| self.negated.get(key))
    }

    /// The settled value usable here (under a subtracted side when `negated`), and
    /// whether it came from the values computed under one; the trace refers to the node
    /// that settled that side (`docs/04-evaluation.md` §3.8).
    // Inlined into `Evaluator::eval`: one lookup per node, and out of line (as after T4.1
    // first split `eval_expr`) it cost the memory-store checks 2-3%.
    #[inline]
    fn lookup(&self, key: &NodeKey, negated: bool) -> Option<(bool, bool)> {
        let plain = if negated { None } else { self.plain.get(key) };
        match plain {
            Some(&v) => Some((v, false)),
            None => self.negated.get(key).map(|&v| (v, true)),
        }
    }

    fn side_mut(&mut self, negated: bool) -> &mut HashMap<NodeKey, bool> {
        if negated {
            &mut self.negated
        } else {
            &mut self.plain
        }
    }
}

#[allow(clippy::struct_field_names, reason = "each field is a `max_*` limit")]
/// Limits the evaluator enforces (`docs/04-evaluation.md` §3.5, §3.7).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) max_depth: u32,
    pub(crate) max_fanout: usize,
    /// Store reads per `check` (per `check_bulk` item).
    pub(crate) max_reads: u64,
    /// The read is at a revision before the schema was written, so it can meet tuples the
    /// schema rejects (`docs/05-storage.md` §7): probe for those it would otherwise not
    /// read, and fail with `StaleRelationship` if one is there.
    pub(crate) stale_possible: bool,
}

/// A request's subject, resolved against the schema once.
#[derive(Clone, Debug)]
pub(crate) struct Subject {
    pub(crate) reference: SubjectRef,
    /// The subject's type when it is an object, for wildcard hits.
    pub(crate) object_type: Option<TypeId>,
    /// The subject's node when it is a subject set, for self membership.
    pub(crate) set: Option<NodeKey>,
}

/// Validates a check against the schema (`docs/04-evaluation.md` §1 step 1) and resolves
/// its root node and subject.
///
/// # Errors
///
/// [`Error::InvalidRequest`] if the resource type, the permission, the subject type or the
/// subject relation is not defined, or the subject is a wildcard.
pub(crate) fn resolve(
    schema: &CompiledSchema,
    resource: &ObjectRef,
    permission: &RelName,
    subject: &SubjectRef,
) -> Result<(NodeKey, Subject), Error> {
    let root = node(schema, resource, permission, "")?;
    Ok((root, resolve_subject(schema, subject)?))
}

/// Validates a request's subject against the schema and resolves it.
///
/// # Errors
///
/// [`Error::InvalidRequest`] if the subject type or subject relation is not defined, or the
/// subject is a wildcard.
pub(crate) fn resolve_subject(
    schema: &CompiledSchema,
    subject: &SubjectRef,
) -> Result<Subject, Error> {
    let resolved = match subject {
        SubjectRef::Wildcard { object_type } => {
            return Err(invalid(format!(
                "a wildcard subject (`{object_type}:*`) cannot be checked; check a concrete \
                 object instead"
            )));
        }
        SubjectRef::Object(o) => {
            let ty = schema.type_id(o.object_type.as_str()).ok_or_else(|| {
                invalid(format!(
                    "subject type `{}` is not defined in the schema",
                    o.object_type
                ))
            })?;
            Subject {
                reference: subject.clone(),
                object_type: Some(ty),
                set: None,
            }
        }
        SubjectRef::Set { object, relation } => Subject {
            reference: subject.clone(),
            object_type: None,
            set: Some(node(schema, object, relation, "subject ")?),
        },
    };
    Ok(resolved)
}

/// The node for `object#member`.
///
/// # Errors
///
/// [`Error::InvalidRequest`] if the type or member is not defined; `what` prefixes the
/// message (`"subject "`).
pub(crate) fn node(
    schema: &CompiledSchema,
    object: &ObjectRef,
    member: &RelName,
    what: &str,
) -> Result<NodeKey, Error> {
    let type_name = object.object_type.as_str();
    let def = schema
        .type_id(type_name)
        .and_then(|t| schema.type_def(t))
        .ok_or_else(|| {
            invalid(format!(
                "{what}type `{type_name}` is not defined in the schema"
            ))
        })?;
    let rel = def.member_id(member.as_str()).ok_or_else(|| {
        invalid(format!(
            "{what}type `{type_name}` has no relation or permission `{member}`"
        ))
    })?;
    Ok(NodeKey {
        ty: def.id,
        id: object.object_id.clone(),
        rel,
    })
}

pub(crate) fn invalid(message: String) -> Error {
    Error::InvalidRequest { message }
}

/// A `check` node's value.
type Outcome = scc::Outcome<bool>;

impl Outcome {
    const TRUE: Self = Self {
        value: true,
        low: None,
    };
    const FALSE: Self = Self {
        value: false,
        low: None,
    };
}

/// Evaluates checks for one subject at one revision, reporting each step to `T`
/// (`docs/04-evaluation.md` §3.8; [`NoTrace`] for a plain `check`).
pub(crate) struct Evaluator<'a, T: Tracer = NoTrace> {
    schema: &'a CompiledSchema,
    store: &'a dyn ReadStore,
    rev: Revision,
    limits: Limits,
    subject: &'a Subject,
    memo: &'a mut Memo,
    /// Cycle handling (§3.6).
    cycle: Scc<bool>,
    /// Store reads by the current `check`, against `limits.max_reads`.
    check_reads: u64,
    stats: EvalStats,
    trace: T,
}

impl<'a> Evaluator<'a> {
    pub(crate) fn new(
        schema: &'a CompiledSchema,
        store: &'a dyn ReadStore,
        rev: Revision,
        limits: Limits,
        subject: &'a Subject,
        memo: &'a mut Memo,
    ) -> Self {
        Evaluator::with_trace(schema, store, rev, limits, subject, memo, NoTrace)
    }
}

impl<'a, T: Tracer> Evaluator<'a, T> {
    pub(crate) fn with_trace(
        schema: &'a CompiledSchema,
        store: &'a dyn ReadStore,
        rev: Revision,
        limits: Limits,
        subject: &'a Subject,
        memo: &'a mut Memo,
        trace: T,
    ) -> Self {
        Self {
            schema,
            store,
            rev,
            limits,
            subject,
            memo,
            cycle: Scc::new(),
            check_reads: 0,
            stats: EvalStats::default(),
            trace,
        }
    }

    /// The trace, once evaluation is over.
    pub(crate) fn into_trace(self) -> T {
        self.trace
    }

    /// `subject ∈ S(root)`. The memo may be shared with earlier calls; nothing else is.
    pub(crate) fn check(&mut self, root: &NodeKey) -> Result<bool, Error> {
        self.reset();
        self.check_reads = 0;
        let result = self.eval(root, 1).map(|o| o.value);
        // Empty after success already; after an error, unsettled values must not leak
        // into the next check.
        self.reset();
        result
    }

    fn reset(&mut self) {
        self.cycle.reset();
    }

    /// Counters accumulated over every [`check`](Self::check) so far; `elapsed` is left to
    /// the caller.
    pub(crate) fn stats(&self) -> EvalStats {
        self.stats
    }

    /// §3.2.
    fn eval(&mut self, key: &NodeKey, depth: u32) -> Result<Outcome, Error> {
        self.stats.max_depth_reached = self.stats.max_depth_reached.max(depth);
        if depth > self.limits.max_depth {
            let mut path: Vec<String> = self.cycle.path_from(0).map(|k| self.name(k)).collect();
            path.push(self.name(key));
            return Err(Error::MaxDepthExceeded { path });
        }
        let negated = self.negated();
        if let Some((value, side)) = self.memo.lookup(key, negated) {
            self.stats.memo_hits += 1;
            self.trace.memo_hit(key, value, side);
            return Ok(Outcome { value, low: None });
        }
        match self.cycle.hit(key)? {
            Some(Hit::Found(out)) => {
                self.trace.cycle_hit(key, &out, negated);
                return Ok(out);
            }
            Some(Hit::NonMonotonic { from }) => return Err(self.non_monotonic(from, key)),
            None => {}
        }
        if self.subject.set.as_ref() == Some(key) {
            self.trace.self_member(key);
            return Ok(Outcome::TRUE);
        }

        let schema = self.schema;
        let member = schema
            .type_def(key.ty)
            .and_then(|t| t.member(key.rel))
            .ok_or_else(|| {
                // Unreachable: every key is built from ids of this same schema.
                invalid(format!("internal: node {key:?} is not in the schema"))
            })?;
        let p = self.cycle.begin(key);
        self.trace.open_member(key, member, negated);
        let mut passes = 0;
        loop {
            passes += 1;
            self.cycle.push_frame(key);
            let out = match member {
                Member::Relation(r) => self.eval_direct(key, &r.allowed, depth),
                Member::Permission(perm) => {
                    let mut fanout = 0;
                    self.eval_op(key, &perm.expr, depth, &mut fanout)
                }
            };
            self.cycle.pop_frame();
            // A region never spans a subtracted side (that cycle is an error), so every
            // node it settles was evaluated with the same `negated`.
            match self.cycle.finish(key, p, out?, self.memo.side_mut(negated)) {
                Finish::Done(out, settle) => {
                    if out.low.is_none() && passes > 1 {
                        self.stats.cycle_passes += passes;
                    }
                    self.trace.close_member(
                        &out,
                        settle,
                        u32::try_from(passes).unwrap_or(u32::MAX),
                    );
                    return Ok(out);
                }
                Finish::Rerun => self.trace.rerun(),
            }
        }
    }

    /// Whether evaluation is under the subtracted side of an exclusion, for a subject-set
    /// subject: only there does it differ from evaluation elsewhere ([`Memo`]).
    fn negated(&self) -> bool {
        self.subject.set.is_some() && self.cycle.negated()
    }

    fn non_monotonic(&self, i: usize, key: &NodeKey) -> Error {
        let mut path: Vec<String> = self.cycle.path_from(i).map(|k| self.name(k)).collect();
        path.push(self.name(key));
        Error::NonMonotonicCycle { path }
    }

    /// §3.3: a relation's stored subjects.
    fn eval_direct(
        &mut self,
        key: &NodeKey,
        allowed: &[AllowedSubject],
        depth: u32,
    ) -> Result<Outcome, Error> {
        let resource = self.object_ref(key);
        let relation = self.rel_name(key.ty, key.rel);

        let exact = Relationship {
            resource: resource.clone(),
            relation: relation.clone(),
            subject: self.subject.reference.clone(),
        };
        let since = self.trace.mark();
        let found = self.exists(&exact)?;
        self.trace.point(false, &exact, found, since);
        if found {
            // A tuple the schema no longer allows (only at revisions before a schema
            // change): what it meant is unknown, so neither grant nor ignore it.
            if !self.subject_form_allowed(allowed) {
                return Err(stale(self.schema, key, &exact));
            }
            return Ok(Outcome::TRUE);
        }

        // A wildcard `t:*` never matches a subject set `t:x#r` (below), but SpiceDB matches
        // it. On a granting path that only makes pemba say `false` where SpiceDB says
        // `true`; under a subtracted side pemba would say `true` where SpiceDB says `false`,
        // so the answer is ambiguous and evaluation fails closed (KD-3, T3.4b).
        if let Some(set) = &self.subject.set
            && self.negated()
            && allowed.contains(&AllowedSubject::Wildcard(set.ty))
        {
            let wildcard = Relationship {
                resource: resource.clone(),
                relation: relation.clone(),
                subject: SubjectRef::Wildcard {
                    object_type: self.object_ref(set).object_type,
                },
            };
            if self.exists(&wildcard)? {
                return Err(Error::AmbiguousWildcardExclusion {
                    wildcard: wildcard.to_string(),
                    subject: self.subject.reference.to_string(),
                });
            }
        }

        // Wildcards match objects only, never subject sets. One the schema no longer
        // allows can only be stored below the schema's revision; probe for it there,
        // since ignoring it would shrink the relation (failing open on a subtracted side).
        if let (Some(t), SubjectRef::Object(o)) =
            (self.subject.object_type, &self.subject.reference)
        {
            let wildcard_allowed = allowed.contains(&AllowedSubject::Wildcard(t));
            if wildcard_allowed || self.limits.stale_possible {
                let wildcard = Relationship {
                    resource: resource.clone(),
                    relation: relation.clone(),
                    subject: SubjectRef::Wildcard {
                        object_type: o.object_type.clone(),
                    },
                };
                let since = self.trace.mark();
                let found = self.exists(&wildcard)?;
                self.trace.point(true, &wildcard, found, since);
                if found {
                    if !wildcard_allowed {
                        return Err(stale(self.schema, key, &wildcard));
                    }
                    return Ok(Outcome::TRUE);
                }
            }
        }

        // Read even when the relation allows no subject sets: a schema change can leave
        // stale ones behind, and skipping them would silently shrink the relation, which
        // on the subtracted side of an exclusion grants access (OQ-019).
        let sets = self.read(|s, rev| s.read_subject_sets(rev, &resource, &relation))?;
        self.count_tuples(sets.len());
        self.check_fanout(key, sets.len())?;
        let mut low = None;
        let mut sets = sets.into_iter();
        while let Some((object, rel)) = sets.next() {
            self.trace
                .open_subject_set(&resource, &relation, &object, &rel);
            // A stale set seen after an earlier `true` is never reached: it could only add
            // members, so the `true` stands whatever it meant.
            let child = self.resolve_set(key, &resource, &relation, object, rel, allowed)?;
            let out = self.eval(&child, depth + 1)?;
            self.trace.close(&out);
            low = min_low(low, out.low);
            if out.value {
                if T::ENABLED {
                    for (object, rel) in sets {
                        self.trace
                            .skipped_subject_set(&resource, &relation, &object, &rel);
                    }
                }
                return Ok(Outcome { value: true, low });
            }
        }
        Ok(Outcome { value: false, low })
    }

    /// §3.4: an operand of the permission on `key`'s object. `fanout` counts tuples read
    /// for this node across all its arrows. A bare name is traced as the member it names;
    /// anything else as an operator node of its own.
    fn eval_expr(
        &mut self,
        key: &NodeKey,
        expr: &Expr,
        depth: u32,
        fanout: &mut usize,
    ) -> Result<Outcome, Error> {
        if matches!(expr, Expr::Computed(_)) {
            return self.eval_op(key, expr, depth, fanout);
        }
        self.trace.open_expr(key, expr);
        let out = self.eval_op(key, expr, depth, fanout)?;
        self.trace.close(&out);
        Ok(out)
    }

    /// §3.4: one operator of a permission expression on `key`'s object (see
    /// [`eval_expr`](Self::eval_expr)).
    fn eval_op(
        &mut self,
        key: &NodeKey,
        expr: &Expr,
        depth: u32,
        fanout: &mut usize,
    ) -> Result<Outcome, Error> {
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
                let mut low = None;
                for (i, e) in operands.iter().enumerate() {
                    let out = self.eval_expr(key, e, depth, fanout)?;
                    low = min_low(low, out.low);
                    if out.value {
                        self.skipped(key, &operands[i + 1..]);
                        return Ok(Outcome { value: true, low });
                    }
                }
                Ok(Outcome { value: false, low })
            }
            Expr::Intersection(operands) => {
                let mut low = None;
                for (i, e) in operands.iter().enumerate() {
                    let out = self.eval_expr(key, e, depth, fanout)?;
                    low = min_low(low, out.low);
                    if !out.value {
                        self.skipped(key, &operands[i + 1..]);
                        return Ok(Outcome { value: false, low });
                    }
                }
                Ok(Outcome { value: true, low })
            }
            Expr::Exclusion { base, sub } => {
                self.trace.role(TraceRole::Base);
                let b = self.eval_expr(key, base, depth, fanout)?;
                if !b.value {
                    if T::ENABLED {
                        self.trace.role(TraceRole::Subtracted);
                        self.trace.skipped_expr(key, sub);
                    }
                    return Ok(b);
                }
                self.trace.role(TraceRole::Subtracted);
                self.cycle.enter_sub();
                let s = self.eval_expr(key, sub, depth, fanout);
                self.cycle.leave_sub();
                let s = s?;
                // Everything the subtracted side used is settled: a value depending on a
                // node in progress could only come from a hit at that node's depth, and
                // every node in progress at this depth or deeper finished inside `sub`.
                // Negating an unsettled value would be unsound, so fail closed.
                if s.low.is_some() {
                    return Err(invalid(format!(
                        "internal: unsettled value negated at {}",
                        self.name(key)
                    )));
                }
                Ok(Outcome {
                    value: !s.value,
                    low: b.low,
                })
            }
            Expr::Arrow {
                tupleset,
                computed,
                mode,
                targets,
            } => {
                let resource = self.object_ref(key);
                let relation = self.rel_name(key.ty, *tupleset);
                // Distinct objects, subject relation dropped, wildcards excluded (`docs/05` §2).
                let objects =
                    self.read(|s, rev| s.read_related_objects(rev, &resource, &relation))?;
                check_related_objects(self.schema, key, *tupleset, &objects)?;
                self.count_tuples(objects.len());
                *fanout = fanout.saturating_add(objects.len());
                self.check_fanout(key, *fanout)?;
                match mode {
                    ArrowMode::Any => self.arrow_any(&objects, computed, targets, depth),
                    ArrowMode::All => self.arrow_all(&objects, computed, targets, depth),
                }
            }
        }
    }

    /// Operands a union or intersection short-circuited past.
    fn skipped(&mut self, key: &NodeKey, rest: &[Expr]) {
        if T::ENABLED {
            for e in rest {
                self.trace.skipped_expr(key, e);
            }
        }
    }

    /// Arrow objects after the one that decided.
    fn skipped_objects(
        &mut self,
        rest: &[ObjectRef],
        computed: &str,
        targets: &BTreeMap<TypeId, RelId>,
    ) {
        if T::ENABLED {
            for x in rest {
                let target = self.arrow_target(x, targets);
                self.trace.skipped_arrow_object(x, computed, target);
            }
        }
    }

    /// Union over related objects; an object whose type lacks the target contributes ∅.
    fn arrow_any(
        &mut self,
        objects: &[ObjectRef],
        computed: &str,
        targets: &BTreeMap<TypeId, RelId>,
        depth: u32,
    ) -> Result<Outcome, Error> {
        let mut low = None;
        for (i, x) in objects.iter().enumerate() {
            let Some(child) = self.arrow_target(x, targets) else {
                self.trace.undefined(x, computed);
                continue;
            };
            let out = self.eval(&child, depth + 1)?;
            low = min_low(low, out.low);
            if out.value {
                self.skipped_objects(&objects[i + 1..], computed, targets);
                return Ok(Outcome { value: true, low });
            }
        }
        Ok(Outcome { value: false, low })
    }

    /// Intersection over related objects: `false` over none, and `false` as soon as one
    /// object's type lacks the target (its set is ∅).
    fn arrow_all(
        &mut self,
        objects: &[ObjectRef],
        computed: &str,
        targets: &BTreeMap<TypeId, RelId>,
        depth: u32,
    ) -> Result<Outcome, Error> {
        if objects.is_empty() {
            return Ok(Outcome::FALSE);
        }
        let mut low = None;
        for (i, x) in objects.iter().enumerate() {
            let Some(child) = self.arrow_target(x, targets) else {
                self.trace.undefined(x, computed);
                self.skipped_objects(&objects[i + 1..], computed, targets);
                return Ok(Outcome { value: false, low });
            };
            let out = self.eval(&child, depth + 1)?;
            low = min_low(low, out.low);
            if !out.value {
                self.skipped_objects(&objects[i + 1..], computed, targets);
                return Ok(Outcome { value: false, low });
            }
        }
        Ok(Outcome { value: true, low })
    }

    fn arrow_target(&self, x: &ObjectRef, targets: &BTreeMap<TypeId, RelId>) -> Option<NodeKey> {
        let ty = self.schema.type_id(x.object_type.as_str())?;
        let rel = *targets.get(&ty)?;
        Some(NodeKey {
            ty,
            id: x.object_id.clone(),
            rel,
        })
    }

    /// The node for a stored subject set `object#rel` on `resource#relation`.
    ///
    /// # Errors
    ///
    /// [`Error::StaleRelationship`] if the relation does not allow that form, including
    /// when its type or relation is no longer defined.
    fn resolve_set(
        &self,
        key: &NodeKey,
        resource: &ObjectRef,
        relation: &RelName,
        object: ObjectRef,
        rel: RelName,
        allowed: &[AllowedSubject],
    ) -> Result<NodeKey, Error> {
        resolve_set(self.schema, key, resource, relation, object, rel, allowed)
    }

    /// Whether the subject's own form (its type, or its type and relation) is an allowed
    /// subject of a relation.
    fn subject_form_allowed(&self, allowed: &[AllowedSubject]) -> bool {
        match (self.subject.object_type, &self.subject.set) {
            (Some(t), _) => allowed.contains(&AllowedSubject::Object(t)),
            (None, Some(k)) => allowed.contains(&AllowedSubject::Set(k.ty, k.rel)),
            (None, None) => false,
        }
    }

    fn check_fanout(&self, key: &NodeKey, read: usize) -> Result<(), Error> {
        if read > self.limits.max_fanout {
            return Err(Error::FanoutExceeded {
                node: self.name(key),
                max: self.limits.max_fanout,
            });
        }
        Ok(())
    }

    fn exists(&mut self, rel: &Relationship) -> Result<bool, Error> {
        let found = self.read(|s, rev| s.exists(rev, rel))?;
        self.count_tuples(usize::from(found));
        Ok(found)
    }

    fn read<R>(
        &mut self,
        f: impl FnOnce(&dyn ReadStore, Revision) -> Result<R, StoreError>,
    ) -> Result<R, Error> {
        self.check_reads += 1;
        if self.check_reads > self.limits.max_reads {
            return Err(Error::ReadLimitExceeded {
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

    fn object_ref(&self, key: &NodeKey) -> ObjectRef {
        object_ref(self.schema, key)
    }

    fn rel_name(&self, ty: TypeId, rel: RelId) -> RelName {
        rel_name(self.schema, ty, rel)
    }

    fn name(&self, key: &NodeKey) -> String {
        name(self.schema, key)
    }
}

/// The node for a stored subject set `object#rel` on `resource#relation` (`key`).
///
/// # Errors
///
/// [`Error::StaleRelationship`] if the relation does not allow that form, including
/// when its type or relation is no longer defined.
pub(crate) fn resolve_set(
    schema: &CompiledSchema,
    key: &NodeKey,
    resource: &ObjectRef,
    relation: &RelName,
    object: ObjectRef,
    rel: RelName,
    allowed: &[AllowedSubject],
) -> Result<NodeKey, Error> {
    let resolved = schema
        .type_id(object.object_type.as_str())
        .and_then(|ty| Some((ty, schema.type_def(ty)?.member_id(rel.as_str())?)));
    match resolved {
        Some((ty, r)) if allowed.contains(&AllowedSubject::Set(ty, r)) => Ok(NodeKey {
            ty,
            id: object.object_id,
            rel: r,
        }),
        _ => Err(Error::StaleRelationship {
            tuple: Relationship {
                resource: resource.clone(),
                relation: relation.clone(),
                subject: SubjectRef::Set {
                    object,
                    relation: rel,
                },
            }
            .to_string(),
            relation: format!("{}#{relation}", type_name(schema, key.ty)),
        }),
    }
}

/// `StaleRelationship` for stored tuple `tuple`, found on `key`'s relation.
pub(crate) fn stale(schema: &CompiledSchema, key: &NodeKey, tuple: &Relationship) -> Error {
    Error::StaleRelationship {
        tuple: tuple.to_string(),
        relation: format!("{}#{}", type_name(schema, key.ty), tuple.relation),
    }
}

/// Fails with `StaleRelationship` if an object related through `key`'s `tupleset` has a type
/// the tupleset no longer allows (as an object or a subject set): an arrow cannot tell what
/// such a tuple meant, and treating it as contributing nothing fails open under exclusion.
/// Only possible at revisions before a schema change (`docs/05-storage.md` §7).
pub(crate) fn check_related_objects(
    schema: &CompiledSchema,
    key: &NodeKey,
    tupleset: RelId,
    objects: &[ObjectRef],
) -> Result<(), Error> {
    let Some(Member::Relation(r)) = schema.type_def(key.ty).and_then(|t| t.member(tupleset)) else {
        return Err(invalid(format!(
            "internal: tupleset of {} is not a relation",
            name(schema, key)
        )));
    };
    for x in objects {
        let ok = schema.type_id(x.object_type.as_str()).is_some_and(|t| {
            r.allowed.iter().any(|a| match *a {
                AllowedSubject::Object(u) | AllowedSubject::Set(u, _) => u == t,
                AllowedSubject::Wildcard(_) => false,
            })
        });
        if !ok {
            return Err(stale(
                schema,
                key,
                &Relationship {
                    resource: object_ref(schema, key),
                    relation: rel_name(schema, key.ty, tupleset),
                    subject: SubjectRef::Object(x.clone()),
                },
            ));
        }
    }
    Ok(())
}

/// The object `key` is a member of.
pub(crate) fn object_ref(schema: &CompiledSchema, key: &NodeKey) -> ObjectRef {
    ObjectRef {
        object_type: TypeName::from_schema(&type_name(schema, key.ty)),
        object_id: key.id.clone(),
    }
}

/// The name of member `rel` of type `ty`.
pub(crate) fn rel_name(schema: &CompiledSchema, ty: TypeId, rel: RelId) -> RelName {
    RelName::from_schema(&member_name(schema, ty, rel))
}

fn type_name(schema: &CompiledSchema, ty: TypeId) -> Arc<str> {
    schema
        .type_def(ty)
        .map_or_else(|| Arc::from("?"), |t| Arc::clone(&t.name))
}

fn member_name(schema: &CompiledSchema, ty: TypeId, rel: RelId) -> Arc<str> {
    match schema.type_def(ty).and_then(|t| t.member(rel)) {
        Some(Member::Relation(r)) => Arc::clone(&r.name),
        Some(Member::Permission(p)) => Arc::clone(&p.name),
        None => Arc::from("?"),
    }
}

/// `type:id#member`, for error paths.
pub(crate) fn name(schema: &CompiledSchema, key: &NodeKey) -> String {
    format!(
        "{}:{}#{}",
        type_name(schema, key.ty),
        key.id,
        member_name(schema, key.ty, key.rel)
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::model::RelationshipFilter;
    use crate::store::{RelationCount, ReverseFilter, StoredSchema};

    /// Just enough of a store for the evaluator: visible tuples, all at revision 1.
    struct Tuples(Vec<Relationship>);

    impl ReadStore for Tuples {
        fn head_revision(&self) -> Result<Revision, StoreError> {
            Ok(Revision(1))
        }
        fn gc_horizon(&self) -> Result<Revision, StoreError> {
            Ok(Revision(0))
        }
        fn exists(&self, _: Revision, rel: &Relationship) -> Result<bool, StoreError> {
            Ok(self.0.contains(rel))
        }
        fn read_subject_sets(
            &self,
            _: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            let mut out: Vec<_> = self
                .0
                .iter()
                .filter(|r| &r.resource == resource && &r.relation == relation)
                .filter_map(|r| match &r.subject {
                    SubjectRef::Set { object, relation } => {
                        Some((object.clone(), relation.clone()))
                    }
                    _ => None,
                })
                .collect();
            out.sort();
            Ok(out)
        }
        fn read_related_objects(
            &self,
            _: Revision,
            _: &ObjectRef,
            _: &RelName,
        ) -> Result<Vec<ObjectRef>, StoreError> {
            Ok(Vec::new())
        }
        fn read_reverse(
            &self,
            _: Revision,
            _: &ReverseFilter,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            Ok(Vec::new())
        }
        fn read_relationships(
            &self,
            _: Revision,
            _: &RelationshipFilter,
            _: u32,
            _: Option<&Relationship>,
        ) -> Result<Vec<Relationship>, StoreError> {
            Ok(Vec::new())
        }
        fn read_tuples(
            &self,
            _: Revision,
            _: &ObjectRef,
            _: &RelName,
        ) -> Result<Vec<SubjectRef>, StoreError> {
            Ok(Vec::new())
        }
        fn count_by_relation(&self, _: Revision) -> Result<Vec<RelationCount>, StoreError> {
            Ok(Vec::new())
        }
        fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
            Ok(None)
        }
    }

    const GROUPS: &str = "definition user {}\n\
        definition group { relation member: user | group#member }\n";

    /// §3.6's `A = B + C`, `B = A`: after evaluating A, the memo holds C (settled) and A
    /// (a head finishing `true` is sound whatever it assumed), but not B, whose `false` was
    /// computed assuming A was `false`.
    #[test]
    fn tainted_nodes_never_enter_the_memo() {
        let schema = pemba_schema::compile(GROUPS).unwrap().schema;
        let store = Tuples(
            [
                "group:a#member@group:b#member",
                "group:a#member@group:c#member",
                "group:b#member@group:a#member",
                "group:c#member@user:anne",
            ]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect(),
        );
        let subject_ref: SubjectRef = "user:anne".parse().unwrap();
        let key = |g: &str| {
            let object: ObjectRef = format!("group:{g}").parse().unwrap();
            resolve(&schema, &object, &"member".parse().unwrap(), &subject_ref)
                .unwrap()
                .0
        };
        let (_, subject) = resolve(
            &schema,
            &"group:a".parse().unwrap(),
            &"member".parse().unwrap(),
            &subject_ref,
        )
        .unwrap();
        let limits = Limits {
            max_depth: 50,
            max_fanout: 100,
            max_reads: 1_000,
            stale_possible: false,
        };
        let mut memo = Memo::new();
        let mut ev = Evaluator::new(&schema, &store, Revision(1), limits, &subject, &mut memo);

        assert!(ev.check(&key("a")).unwrap());
        assert_eq!(memo.get(&key("c")), Some(&true));
        assert_eq!(memo.get(&key("a")), Some(&true), "head-true shortcut");
        assert_eq!(memo.get(&key("b")), None, "B=false must not be memoised");

        // B asked next, with the same memo: correct despite the earlier tainted `false`.
        let mut ev = Evaluator::new(&schema, &store, Revision(1), limits, &subject, &mut memo);
        assert!(ev.check(&key("b")).unwrap());
    }

    /// An error mid-region leaves nothing behind: no SCC entries, pass memo or provisional
    /// values, and no unsettled value in the memo, so the next check on the same evaluator
    /// (as for the next `check_bulk` item) starts clean.
    #[test]
    fn unsettled_state_is_cleared_after_an_error() {
        let schema = pemba_schema::compile(GROUPS).unwrap().schema;
        let store = Tuples(
            [
                "group:a#member@group:b#member",
                "group:b#member@group:a#member",
                "group:b#member@group:c#member",
                "group:c#member@group:z#member",
                "group:z#member@user:anne",
            ]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect(),
        );
        let subject_ref: SubjectRef = "user:anne".parse().unwrap();
        let key = |g: &str| {
            let object: ObjectRef = format!("group:{g}").parse().unwrap();
            resolve(&schema, &object, &"member".parse().unwrap(), &subject_ref)
                .unwrap()
                .0
        };
        let (_, subject) = resolve(
            &schema,
            &"group:a".parse().unwrap(),
            &"member".parse().unwrap(),
            &subject_ref,
        )
        .unwrap();
        let limits = Limits {
            max_depth: 50,
            max_fanout: 100,
            max_reads: 5,
            stale_possible: false,
        };
        let mut memo = Memo::new();
        let mut ev = Evaluator::new(&schema, &store, Revision(1), limits, &subject, &mut memo);

        // a, b (cycle back to a), then c's subject-set read is the sixth.
        assert!(matches!(
            ev.check(&key("a")),
            Err(Error::ReadLimitExceeded { max: 5 })
        ));
        assert!(ev.cycle.is_empty());
        // The read budget is per check.
        assert!(ev.check(&key("z")).unwrap());
        for g in ["a", "b", "c"] {
            assert_eq!(memo.get(&key(g)), None, "{g} never settled");
        }
    }
}
