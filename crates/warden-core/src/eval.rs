//! The `check` evaluator (`docs/04-evaluation.md` §2–4).
//!
//! `check(o, m, s)` decides `s ∈ S(o, m)` by recursing over [`NodeKey`]s `(type, id,
//! member)` for one fixed subject at one revision. Relations read the store
//! ([`Evaluator::eval_direct`]); permissions evaluate their expression left to right with
//! short-circuiting ([`Evaluator::eval_expr`]).
//!
//! Data-dependent cycles (§3.6) are cut by treating a node already on the stack as `false`.
//! That is the least fixed point for the monotone operators, but every result computed
//! under such an assumption is *tainted* and never memoised: it may be wrong out of
//! context. A cycle through the subtracted side of an exclusion has no least fixed point,
//! so it is an error.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use warden_schema::ir::{AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, RelId, TypeId};

use crate::engine::read_error;
use crate::error::Error;
use crate::model::{
    Consistency, ObjectId, ObjectRef, RelName, Relationship, Revision, SubjectRef, TypeName,
};
use crate::store::{ReadStore, StoreError};

/// Asks whether `subject` has `permission` on `resource` (`docs/04-evaluation.md` §3.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

/// The answer to a [`CheckRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
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
pub struct CheckItem {
    /// The object the permission is on.
    pub resource: ObjectRef,
    /// A relation or permission defined on the resource's type.
    pub permission: RelName,
    /// An object or a subject set; a wildcard fails this item with `InvalidRequest`.
    pub subject: SubjectRef,
}

/// Counters for one read request, also recorded on its tracing span
/// (`docs/04-evaluation.md` §1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EvalStats {
    /// Store calls made.
    pub store_reads: u64,
    /// Tuples those calls returned; a point query that finds its tuple counts one.
    pub tuples_read: u64,
    /// Nodes answered from the memo instead of being evaluated.
    pub memo_hits: u64,
    /// The deepest nested evaluation reached; the root node is depth 1.
    pub max_depth_reached: u32,
    /// Wall-clock time spent in the request.
    pub elapsed: Duration,
}

impl EvalStats {
    /// Adds `other`'s counters to these, keeping the larger depth.
    pub(crate) fn absorb(&mut self, other: &Self) {
        self.store_reads += other.store_reads;
        self.tuples_read += other.tuples_read;
        self.memo_hits += other.memo_hits;
        self.max_depth_reached = self.max_depth_reached.max(other.max_depth_reached);
    }
}

/// A member of one object: `S(type:id, rel)`. The subject is fixed per evaluator, so it is
/// not part of the key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NodeKey {
    ty: TypeId,
    id: ObjectId,
    rel: RelId,
}

/// Settled, untainted node values for one subject at one revision. Safe to share between
/// checks with the same subject and revision (`check_bulk`, lookups).
pub(crate) type Memo = HashMap<NodeKey, bool>;

/// Limits the evaluator enforces (`docs/04-evaluation.md` §3.5, §3.7).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) max_depth: u32,
    pub(crate) max_fanout: usize,
}

/// A request's subject, resolved against the schema once.
#[derive(Clone, Debug)]
pub(crate) struct Subject {
    reference: SubjectRef,
    /// The subject's type when it is an object, for wildcard hits.
    object_type: Option<TypeId>,
    /// The subject's node when it is a subject set, for self membership.
    set: Option<NodeKey>,
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
    Ok((root, resolved))
}

fn node(
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

fn invalid(message: String) -> Error {
    Error::InvalidRequest { message }
}

/// A node's value and whether it depends on an in-progress node assumed `false`.
#[derive(Clone, Copy, Debug)]
struct Outcome {
    value: bool,
    tainted: bool,
}

impl Outcome {
    const TRUE: Self = Self {
        value: true,
        tainted: false,
    };
    const FALSE: Self = Self {
        value: false,
        tainted: false,
    };
}

/// A node in progress. `negated` is set when the frame was pushed while evaluating the
/// subtracted side of an exclusion, and so for everything beneath it (§3.6).
#[derive(Debug)]
struct Frame {
    key: NodeKey,
    negated: bool,
}

/// Evaluates checks for one subject at one revision.
pub(crate) struct Evaluator<'a> {
    schema: &'a CompiledSchema,
    store: &'a dyn ReadStore,
    rev: Revision,
    limits: Limits,
    subject: &'a Subject,
    memo: &'a mut Memo,
    stack: Vec<Frame>,
    /// Whether evaluation is currently inside the subtracted side of an exclusion.
    negated: bool,
    stats: EvalStats,
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
        Self {
            schema,
            store,
            rev,
            limits,
            subject,
            memo,
            stack: Vec::new(),
            negated: false,
            stats: EvalStats::default(),
        }
    }

    /// `subject ∈ S(root)`. The memo may be shared with earlier calls; the stack is not.
    pub(crate) fn check(&mut self, root: NodeKey) -> Result<bool, Error> {
        self.stack.clear();
        self.negated = false;
        self.eval(root, 1).map(|o| o.value)
    }

    /// Counters accumulated over every [`check`](Self::check) so far; `elapsed` is left to
    /// the caller.
    pub(crate) fn stats(&self) -> EvalStats {
        self.stats
    }

    /// §3.2.
    fn eval(&mut self, key: NodeKey, depth: u32) -> Result<Outcome, Error> {
        self.stats.max_depth_reached = self.stats.max_depth_reached.max(depth);
        if depth > self.limits.max_depth {
            let mut path: Vec<String> = self.stack.iter().map(|f| self.name(&f.key)).collect();
            path.push(self.name(&key));
            return Err(Error::MaxDepthExceeded { path });
        }
        if let Some(&value) = self.memo.get(&key) {
            self.stats.memo_hits += 1;
            return Ok(Outcome {
                value,
                tainted: false,
            });
        }
        if let Some(i) = self.stack.iter().position(|f| f.key == key) {
            return self.on_cycle(i, &key);
        }
        if self.subject.set.as_ref() == Some(&key) {
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
        self.stack.push(Frame {
            key: key.clone(),
            negated: self.negated,
        });
        let out = match member {
            Member::Relation(r) => self.eval_direct(&key, &r.allowed, depth),
            Member::Permission(p) => {
                let mut fanout = 0;
                self.eval_expr(&key, &p.expr, depth, &mut fanout)
            }
        };
        self.stack.pop();
        let out = out?;
        if !out.tainted {
            self.memo.insert(key, out.value);
        }
        Ok(out)
    }

    /// §3.6: `key` is already in progress at stack index `i`.
    fn on_cycle(&self, i: usize, key: &NodeKey) -> Result<Outcome, Error> {
        // The closing edge (from the top frame back to `key`) is part of the cycle too: in
        // `view = viewer - parent->view` with `doc:a#parent@doc:a` no frame above `key`
        // exists, but the edge back to it runs through the subtracted side.
        let through_negation = self.negated || self.stack[i..].iter().any(|f| f.negated);
        if through_negation {
            let mut path: Vec<String> = self.stack[i..].iter().map(|f| self.name(&f.key)).collect();
            path.push(self.name(key));
            return Err(Error::NonMonotonicCycle { path });
        }
        Ok(Outcome {
            value: false,
            tainted: true,
        })
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
        if self.exists(&exact)? {
            return Ok(Outcome::TRUE);
        }

        // Wildcards match objects only, never subject sets.
        if let (Some(t), SubjectRef::Object(o)) =
            (self.subject.object_type, &self.subject.reference)
            && allowed.contains(&AllowedSubject::Wildcard(t))
        {
            let wildcard = Relationship {
                resource: resource.clone(),
                relation: relation.clone(),
                subject: SubjectRef::Wildcard {
                    object_type: o.object_type.clone(),
                },
            };
            if self.exists(&wildcard)? {
                return Ok(Outcome::TRUE);
            }
        }

        // Read even when the relation allows no subject sets: a schema change can leave
        // stale ones behind, and skipping them would silently shrink the relation, which
        // on the subtracted side of an exclusion grants access (OQ-019).
        let sets = self.read(|s, rev| s.read_subject_sets(rev, &resource, &relation))?;
        self.count_tuples(sets.len());
        self.check_fanout(key, sets.len())?;
        let mut tainted = false;
        for (object, rel) in sets {
            // A stale set seen after an earlier `true` is never reached: it could only add
            // members, so the `true` stands whatever it meant.
            let child = self.resolve_set(key, &resource, &relation, object, rel, allowed)?;
            let out = self.eval(child, depth + 1)?;
            tainted |= out.tainted;
            if out.value {
                return Ok(Outcome {
                    value: true,
                    tainted,
                });
            }
        }
        Ok(Outcome {
            value: false,
            tainted,
        })
    }

    /// §3.4: a permission expression on `key`'s object. `fanout` counts tuples read for
    /// this node across all its arrows.
    fn eval_expr(
        &mut self,
        key: &NodeKey,
        expr: &Expr,
        depth: u32,
        fanout: &mut usize,
    ) -> Result<Outcome, Error> {
        match expr {
            Expr::Computed(rel) => self.eval(
                NodeKey {
                    ty: key.ty,
                    id: key.id.clone(),
                    rel: *rel,
                },
                depth + 1,
            ),
            Expr::Union(operands) => {
                let mut tainted = false;
                for e in operands {
                    let out = self.eval_expr(key, e, depth, fanout)?;
                    tainted |= out.tainted;
                    if out.value {
                        return Ok(Outcome {
                            value: true,
                            tainted,
                        });
                    }
                }
                Ok(Outcome {
                    value: false,
                    tainted,
                })
            }
            Expr::Intersection(operands) => {
                let mut tainted = false;
                for e in operands {
                    let out = self.eval_expr(key, e, depth, fanout)?;
                    tainted |= out.tainted;
                    if !out.value {
                        return Ok(Outcome {
                            value: false,
                            tainted,
                        });
                    }
                }
                Ok(Outcome {
                    value: true,
                    tainted,
                })
            }
            Expr::Exclusion { base, sub } => {
                let b = self.eval_expr(key, base, depth, fanout)?;
                if !b.value {
                    return Ok(b);
                }
                let outer = self.negated;
                self.negated = true;
                let s = self.eval_expr(key, sub, depth, fanout);
                self.negated = outer;
                let s = s?;
                Ok(Outcome {
                    value: !s.value,
                    tainted: b.tainted || s.tainted,
                })
            }
            Expr::Arrow {
                tupleset,
                mode,
                targets,
                ..
            } => {
                let resource = self.object_ref(key);
                let relation = self.rel_name(key.ty, *tupleset);
                // Distinct objects, subject relation dropped, wildcards excluded (`docs/05` §2).
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

    /// Union over related objects; an object whose type lacks the target contributes ∅.
    fn arrow_any(
        &mut self,
        objects: &[ObjectRef],
        targets: &BTreeMap<TypeId, RelId>,
        depth: u32,
    ) -> Result<Outcome, Error> {
        let mut tainted = false;
        for x in objects {
            let Some(child) = self.arrow_target(x, targets) else {
                continue;
            };
            let out = self.eval(child, depth + 1)?;
            tainted |= out.tainted;
            if out.value {
                return Ok(Outcome {
                    value: true,
                    tainted,
                });
            }
        }
        Ok(Outcome {
            value: false,
            tainted,
        })
    }

    /// Intersection over related objects: `false` over none, and `false` as soon as one
    /// object's type lacks the target (its set is ∅).
    fn arrow_all(
        &mut self,
        objects: &[ObjectRef],
        targets: &BTreeMap<TypeId, RelId>,
        depth: u32,
    ) -> Result<Outcome, Error> {
        if objects.is_empty() {
            return Ok(Outcome::FALSE);
        }
        let mut tainted = false;
        for x in objects {
            let Some(child) = self.arrow_target(x, targets) else {
                return Ok(Outcome {
                    value: false,
                    tainted,
                });
            };
            let out = self.eval(child, depth + 1)?;
            tainted |= out.tainted;
            if !out.value {
                return Ok(Outcome {
                    value: false,
                    tainted,
                });
            }
        }
        Ok(Outcome {
            value: true,
            tainted,
        })
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
        let resolved = self
            .schema
            .type_id(object.object_type.as_str())
            .and_then(|ty| Some((ty, self.schema.type_def(ty)?.member_id(rel.as_str())?)));
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
                relation: format!("{}#{relation}", self.type_name(key.ty)),
            }),
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

    fn read<T>(
        &mut self,
        f: impl FnOnce(&dyn ReadStore, Revision) -> Result<T, StoreError>,
    ) -> Result<T, Error> {
        self.stats.store_reads += 1;
        f(self.store, self.rev).map_err(read_error)
    }

    fn count_tuples(&mut self, n: usize) {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        self.stats.tuples_read = self.stats.tuples_read.saturating_add(n);
    }

    fn object_ref(&self, key: &NodeKey) -> ObjectRef {
        ObjectRef {
            object_type: TypeName::from_schema(&self.type_name(key.ty)),
            object_id: key.id.clone(),
        }
    }

    fn rel_name(&self, ty: TypeId, rel: RelId) -> RelName {
        RelName::from_schema(&self.member_name(ty, rel))
    }

    fn type_name(&self, ty: TypeId) -> Arc<str> {
        self.schema
            .type_def(ty)
            .map_or_else(|| Arc::from("?"), |t| Arc::clone(&t.name))
    }

    fn member_name(&self, ty: TypeId, rel: RelId) -> Arc<str> {
        match self.schema.type_def(ty).and_then(|t| t.member(rel)) {
            Some(Member::Relation(r)) => Arc::clone(&r.name),
            Some(Member::Permission(p)) => Arc::clone(&p.name),
            None => Arc::from("?"),
        }
    }

    /// `type:id#member`, for error paths.
    fn name(&self, key: &NodeKey) -> String {
        format!(
            "{}:{}#{}",
            self.type_name(key.ty),
            key.id,
            self.member_name(key.ty, key.rel)
        )
    }
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

    /// §3.6's `A = B + C`, `B = A`: after evaluating A, the memo holds C (settled) but
    /// neither A nor B, both of which were computed assuming A was `false`.
    #[test]
    fn tainted_nodes_never_enter_the_memo() {
        let schema = warden_schema::compile(GROUPS).unwrap().schema;
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
        };
        let mut memo = Memo::new();
        let mut ev = Evaluator::new(&schema, &store, Revision(1), limits, &subject, &mut memo);

        assert!(ev.check(key("a")).unwrap());
        assert_eq!(memo.get(&key("c")), Some(&true));
        assert_eq!(memo.get(&key("a")), None, "A is tainted by its own cycle");
        assert_eq!(memo.get(&key("b")), None, "B=false must not be memoised");

        // B asked next, with the same memo: correct despite the earlier tainted `false`.
        let mut ev = Evaluator::new(&schema, &store, Revision(1), limits, &subject, &mut memo);
        assert!(ev.check(key("b")).unwrap());
    }
}
