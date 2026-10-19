//! `lookup_resources` (`docs/04-evaluation.md` §5, ADR-007): which objects of one type
//! have a permission for one subject?
//!
//! Two phases, both at one revision through one store snapshot:
//!
//! 1. **Candidates** ([`Candidates`]): a breadth-first walk *backwards* from the subject over
//!    nodes `(type, id, member)` meaning "the subject might be in `S(type:id, member)`".
//!    Every operator counts as a union, so intersections, exclusions and `.all` arrows
//!    over-approximate.
//! 2. **Filter** ([`filter`]): the `check` evaluator decides each candidate, ascending by
//!    id, with one memo shared by the whole page.
//!
//! Why the candidates contain every answer (§5.3). If `s ∈ S(o, m)` there is a finite
//! derivation: a chain from a stored tuple naming `s` (or, for a subject set, from `s`
//! itself by self membership) through steps to `(o, m)`. Each step is inverted by exactly
//! one seed or expansion rule:
//! - a stored tuple `x#y@s` (or `x#y@t:*` matching an object `s` of type `t`) is a seed;
//! - `s ∈ S(o', r)` by self membership is the seed for a subject-set subject;
//! - `Computed(r)` inside `m`'s expression on the same object: rule 1, `used_by`;
//! - a stored subject set `x#y@T:id#r` contributing `S(T:id, r)`: rule 2;
//! - an arrow `t->r` on `x` through a stored `x#t@T:id[#any]`: rule 3, `arrows_into`.
//!
//! A positive derivation through an intersection, exclusion or `.all` arrow passes
//! through a positive branch (every operand, the base side, every related object), which
//! the union approximation follows. So candidates ⊇ answer, and phase 2 makes it exact.
//! Property 2 (`docs/07-testing.md` §2.3) checks this against brute force.

use std::collections::{BTreeSet, HashSet, VecDeque};

use warden_schema::ir::{CompiledSchema, Member, RelId, TypeId};

use crate::EvalStats;
use crate::engine::read_error;
use crate::error::Error;
use crate::eval::{Evaluator, Limits, Memo, NodeKey, Subject, invalid};
use crate::model::{Consistency, ObjectId, ObjectRef, RelName, Revision, SubjectRef, TypeName};
use crate::store::{ReadStore, ReverseFilter, SubjectRelMatch};

/// Which objects of `resource_type` have `permission` for `subject`
/// (`docs/04-evaluation.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupResourcesRequest {
    /// The type of the objects to find.
    pub resource_type: TypeName,
    /// A permission or relation of `resource_type`.
    pub permission: RelName,
    /// An object or a subject set; never a wildcard.
    pub subject: SubjectRef,
    /// Which revision to read at. Pages after the first must pass
    /// `Consistency::Exact(first_response.revision)`, or they may answer at a different
    /// revision than the cursor came from.
    pub consistency: Consistency,
    /// Page size; `None` means `EngineConfig::max_lookup_results`, which is also the
    /// maximum.
    pub limit: Option<u32>,
    /// Start strictly after this id: the previous page's
    /// [`LookupResourcesResponse::next_cursor`].
    pub cursor: Option<ObjectId>,
}

impl LookupResourcesRequest {
    /// The first page at the latest revision, with the default page size.
    pub fn new(resource_type: TypeName, permission: RelName, subject: SubjectRef) -> Self {
        Self {
            resource_type,
            permission,
            subject,
            consistency: Consistency::Latest,
            limit: None,
            cursor: None,
        }
    }
}

/// One page of `lookup_resources`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupResourcesResponse {
    /// Ids of the objects that have the permission, ascending byte-wise.
    pub resource_ids: Vec<ObjectId>,
    /// Cursor for the next page, or `None` if no candidates remain. A page may be followed
    /// by an empty last page when the remaining candidates are all denied.
    pub next_cursor: Option<ObjectId>,
    /// The revision the page was evaluated at; pass `Consistency::Exact(revision)` for
    /// later pages.
    pub revision: Revision,
}

/// Validates the target `resource_type#permission` (`docs/04` §1 step 1).
///
/// # Errors
///
/// [`Error::InvalidRequest`] if the type or member is not defined.
pub(crate) fn resolve_target(
    schema: &CompiledSchema,
    resource_type: &TypeName,
    permission: &RelName,
) -> Result<(TypeId, RelId), Error> {
    let name = resource_type.as_str();
    let def = schema
        .type_id(name)
        .and_then(|t| schema.type_def(t))
        .ok_or_else(|| invalid(format!("type `{name}` is not defined in the schema")))?;
    let rel = def.member_id(permission.as_str()).ok_or_else(|| {
        invalid(format!(
            "type `{name}` has no relation or permission `{permission}`"
        ))
    })?;
    Ok((def.id, rel))
}

/// Phase 1 (§5.1): the backward walk.
pub(crate) struct Candidates<'a> {
    schema: &'a CompiledSchema,
    store: &'a dyn ReadStore,
    rev: Revision,
    max_nodes: usize,
    target: (TypeId, RelId),
    visited: HashSet<NodeKey>,
    queue: VecDeque<NodeKey>,
    found: BTreeSet<ObjectId>,
    reads: u64,
}

impl<'a> Candidates<'a> {
    pub(crate) fn new(
        schema: &'a CompiledSchema,
        store: &'a dyn ReadStore,
        rev: Revision,
        max_nodes: usize,
        target: (TypeId, RelId),
    ) -> Self {
        Self {
            schema,
            store,
            rev,
            max_nodes,
            target,
            visited: HashSet::new(),
            queue: VecDeque::new(),
            found: BTreeSet::new(),
            reads: 0,
        }
    }

    /// Every id of the target type whose target member might contain `subject`,
    /// ascending; plus the nodes visited and store reads made.
    ///
    /// # Errors
    ///
    /// [`Error::LookupTooLarge`] past `max_nodes` visited nodes; store errors as reads
    /// report them.
    pub(crate) fn run(
        mut self,
        subject: &Subject,
    ) -> Result<(BTreeSet<ObjectId>, usize, u64), Error> {
        match (&subject.reference, &subject.set) {
            (SubjectRef::Object(o), _) => {
                // Tuples naming the object itself, and wildcards of its type (which match
                // any object, including one that appears in no tuple).
                for id in [o.object_id.as_str(), "*"] {
                    self.seed_reverse(&o.object_type, id, SubjectRelMatch::None)?;
                }
            }
            (SubjectRef::Set { object, relation }, Some(node)) => {
                // Self membership. Rule 2 on this node then finds tuples naming the set
                // exactly; it runs unconditionally here, because `check` matches such a
                // tuple by exact lookup whether or not the schema still allows it.
                self.visit(node.clone())?;
                self.seed_reverse(
                    &object.object_type,
                    object.object_id.as_str(),
                    SubjectRelMatch::Exact(relation.clone()),
                )?;
            }
            _ => return Err(invalid("internal: unresolved lookup subject".to_owned())),
        }
        while let Some(node) = self.queue.pop_front() {
            self.expand(&node)?;
        }
        Ok((self.found, self.visited.len(), self.reads))
    }

    fn expand(&mut self, node: &NodeKey) -> Result<(), Error> {
        let schema = self.schema;
        // 1. Same-object use: permissions on this type that compute `rel`.
        for &(ty, perm) in schema.used_by(node.ty, node.rel) {
            self.visit(NodeKey {
                ty,
                id: node.id.clone(),
                rel: perm,
            })?;
        }
        let (ty_name, rel_name) = self.names(node.ty, node.rel)?;
        // 2. Subject-set use: tuples whose subject is exactly `T:id#rel`.
        if !schema.subject_set_uses(node.ty, node.rel).is_empty() {
            self.seed_reverse(&ty_name, node.id.as_str(), SubjectRelMatch::Exact(rel_name))?;
        }
        // 3. Arrow use: `S:sid#t@T:id` with any subject relation, for each arrow
        // `t->rel` in permission `p` on `S`. The list is sorted, so uses sharing
        // `(S, t)` are adjacent and share one read.
        let uses = schema.arrows_into(node.ty, node.rel);
        let mut i = 0;
        while let Some(&(source, tupleset, _)) = uses.get(i) {
            let group = uses[i..]
                .iter()
                .take_while(|&&(s, t, _)| (s, t) == (source, tupleset))
                .count();
            let (source_name, tupleset_name) = self.names(source, tupleset)?;
            let found = self.read_reverse(&ReverseFilter {
                subject_type: ty_name.clone(),
                subject_id: node.id.as_str().to_owned(),
                subject_relation: SubjectRelMatch::Any,
                resource_type: Some(source_name),
                relation: Some(tupleset_name),
            })?;
            for (resource, _) in found {
                for &(_, _, perm) in &uses[i..i + group] {
                    self.visit(NodeKey {
                        ty: source,
                        id: resource.object_id.clone(),
                        rel: perm,
                    })?;
                }
            }
            i += group;
        }
        Ok(())
    }

    /// Visits every `(resource, relation)` holding a tuple with this subject.
    fn seed_reverse(
        &mut self,
        subject_type: &TypeName,
        subject_id: &str,
        subject_relation: SubjectRelMatch,
    ) -> Result<(), Error> {
        let found = self.read_reverse(&ReverseFilter {
            subject_type: subject_type.clone(),
            subject_id: subject_id.to_owned(),
            subject_relation,
            resource_type: None,
            relation: None,
        })?;
        for (resource, relation) in found {
            // A type or relation the schema no longer defines is unreachable from any
            // schema member, so `check` can never pass through it.
            let Some(ty) = self.schema.type_id(resource.object_type.as_str()) else {
                continue;
            };
            let Some(rel) = self
                .schema
                .type_def(ty)
                .and_then(|t| t.member_id(relation.as_str()))
            else {
                continue;
            };
            self.visit(NodeKey {
                ty,
                id: resource.object_id,
                rel,
            })?;
        }
        Ok(())
    }

    fn visit(&mut self, node: NodeKey) -> Result<(), Error> {
        if self.visited.contains(&node) {
            return Ok(());
        }
        if self.visited.len() >= self.max_nodes {
            return Err(Error::LookupTooLarge {
                max: self.max_nodes,
            });
        }
        if (node.ty, node.rel) == self.target {
            self.found.insert(node.id.clone());
        }
        self.visited.insert(node.clone());
        self.queue.push_back(node);
        Ok(())
    }

    fn read_reverse(&mut self, f: &ReverseFilter) -> Result<Vec<(ObjectRef, RelName)>, Error> {
        self.reads += 1;
        self.store.read_reverse(self.rev, f).map_err(read_error)
    }

    fn names(&self, ty: TypeId, rel: RelId) -> Result<(TypeName, RelName), Error> {
        let def = self.schema.type_def(ty);
        match def.and_then(|d| Some((d, d.member(rel)?))) {
            Some((d, m)) => {
                let member = match m {
                    Member::Relation(r) => &r.name,
                    Member::Permission(p) => &p.name,
                };
                Ok((TypeName::from_schema(&d.name), RelName::from_schema(member)))
            }
            // Unreachable: every id comes from this schema.
            None => Err(invalid(format!("internal: no member {rel:?} on {ty:?}"))),
        }
    }
}

/// Phase 2 (§5.2): runs `check` on candidates after `cursor`, ascending, with one memo for
/// the page (valid: same subject, same revision), until `limit` are allowed. Returns the
/// allowed ids, whether candidates remain, and the evaluator's counters.
///
/// # Errors
///
/// The first error any candidate's `check` returns: the page fails closed rather than
/// leave out (or include) an object it could not decide.
#[allow(clippy::too_many_arguments, reason = "internal; one call site")]
pub(crate) fn filter(
    schema: &CompiledSchema,
    store: &dyn ReadStore,
    rev: Revision,
    limits: Limits,
    subject: &Subject,
    target: (TypeId, RelId),
    candidates: &BTreeSet<ObjectId>,
    cursor: Option<&ObjectId>,
    limit: usize,
) -> (Result<(Vec<ObjectId>, bool), Error>, EvalStats) {
    let mut memo = Memo::new();
    let mut evaluator = Evaluator::new(schema, store, rev, limits, subject, &mut memo);
    let mut remaining = candidates
        .iter()
        .filter(|id| cursor.is_none_or(|c| *id > c))
        .peekable();
    let mut allowed = Vec::new();
    let result = loop {
        if allowed.len() >= limit {
            break Ok(remaining.peek().is_some());
        }
        let Some(id) = remaining.next() else {
            break Ok(false);
        };
        let node = NodeKey {
            ty: target.0,
            id: id.clone(),
            rel: target.1,
        };
        match evaluator.check(&node) {
            Ok(true) => allowed.push(id.clone()),
            Ok(false) => {}
            Err(e) => break Err(e),
        }
    };
    let stats = evaluator.stats();
    (result.map(|more| (allowed, more)), stats)
}
