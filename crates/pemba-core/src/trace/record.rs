//! The trace sink the evaluator reports to (`docs/04-evaluation.md` §3.8).
//!
//! [`NoTrace`] does nothing and compiles away: its hooks are empty and inlined, and the
//! evaluator guards any loop that only feeds the trace with [`Tracer::ENABLED`].
//! [`Recorder`] keeps every node in an arena while evaluation runs, because a cyclic
//! region's head may discard a whole pass and a memo hit may point at a node of a
//! discarded pass. Only at the end is the arena flattened into the tree: each node is
//! shown in full once, at its first occurrence in pre-order, and every later occurrence
//! becomes a reference.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pemba_schema::ir::{ArrowMode, CompiledSchema, Expr, Member, TypeId};

use super::{TraceKind, TraceNode, TraceRole, TraceSource};
use crate::eval::{NodeKey, name};
use crate::model::{ObjectRef, RelName, Relationship};
use crate::scc::{Outcome, Settle};

/// What the evaluator tells a trace. Every hook is called at the point of evaluation it
/// describes; see [`Recorder`] for what each records.
pub(crate) trait Tracer {
    /// Whether hooks do anything; loops that only feed the trace are skipped when not.
    const ENABLED: bool;
    /// A point in time, for timing a store query.
    type Mark: Copy;

    fn mark(&self) -> Self::Mark;
    /// The next node recorded is this side of an exclusion.
    fn role(&mut self, role: TraceRole);
    /// `key` answered from the request memo.
    fn memo_hit(&mut self, key: &NodeKey, value: bool);
    /// `key` reached again while on the SCC stack.
    fn cycle_hit(&mut self, key: &NodeKey, out: &Outcome<bool>);
    /// `key` is the subject itself.
    fn self_member(&mut self, key: &NodeKey);
    /// `key` is about to be evaluated.
    fn open_member(&mut self, key: &NodeKey, member: &Member);
    /// The head now open is re-running: forget this pass.
    fn rerun(&mut self);
    /// The member opened last finished.
    fn close_member(&mut self, out: &Outcome<bool>, settle: Settle, passes: u32);
    /// An exact (`wildcard` false) or wildcard (`true`) point query, started at `since`.
    fn point(&mut self, wildcard: bool, rel: &Relationship, found: bool, since: Self::Mark);
    /// A stored subject set `object#rel` on `resource#relation` is about to be evaluated.
    fn open_subject_set(
        &mut self,
        resource: &ObjectRef,
        relation: &RelName,
        object: &ObjectRef,
        rel: &RelName,
    );
    /// One not evaluated because an earlier one was `true`.
    fn skipped_subject_set(
        &mut self,
        resource: &ObjectRef,
        relation: &RelName,
        object: &ObjectRef,
        rel: &RelName,
    );
    /// An operand of `key`'s permission (never a bare name) is about to be evaluated.
    fn open_expr(&mut self, key: &NodeKey, expr: &Expr);
    /// The subject set or operand opened last finished.
    fn close(&mut self, out: &Outcome<bool>);
    /// An operand of `key`'s permission that short-circuiting skipped.
    fn skipped_expr(&mut self, key: &NodeKey, expr: &Expr);
    /// An arrow reached `x`, whose type has no member `computed`.
    fn undefined(&mut self, x: &ObjectRef, computed: &str);
    /// An arrow object short-circuiting skipped: `x#computed`, whose member is `target`.
    fn skipped_arrow_object(&mut self, x: &ObjectRef, computed: &str, target: Option<NodeKey>);
}

/// The trace `check` uses: nothing.
pub(crate) struct NoTrace;

impl Tracer for NoTrace {
    const ENABLED: bool = false;
    type Mark = ();

    #[inline(always)]
    fn mark(&self) {}
    #[inline(always)]
    fn role(&mut self, _: TraceRole) {}
    #[inline(always)]
    fn memo_hit(&mut self, _: &NodeKey, _: bool) {}
    #[inline(always)]
    fn cycle_hit(&mut self, _: &NodeKey, _: &Outcome<bool>) {}
    #[inline(always)]
    fn self_member(&mut self, _: &NodeKey) {}
    #[inline(always)]
    fn open_member(&mut self, _: &NodeKey, _: &Member) {}
    #[inline(always)]
    fn rerun(&mut self) {}
    #[inline(always)]
    fn close_member(&mut self, _: &Outcome<bool>, _: Settle, _: u32) {}
    #[inline(always)]
    fn point(&mut self, _: bool, _: &Relationship, _: bool, (): ()) {}
    #[inline(always)]
    fn open_subject_set(&mut self, _: &ObjectRef, _: &RelName, _: &ObjectRef, _: &RelName) {}
    #[inline(always)]
    fn skipped_subject_set(&mut self, _: &ObjectRef, _: &RelName, _: &ObjectRef, _: &RelName) {}
    #[inline(always)]
    fn open_expr(&mut self, _: &NodeKey, _: &Expr) {}
    #[inline(always)]
    fn close(&mut self, _: &Outcome<bool>) {}
    #[inline(always)]
    fn skipped_expr(&mut self, _: &NodeKey, _: &Expr) {}
    #[inline(always)]
    fn undefined(&mut self, _: &ObjectRef, _: &str) {}
    #[inline(always)]
    fn skipped_arrow_object(&mut self, _: &ObjectRef, _: &str, _: Option<NodeKey>) {}
}

/// Where a recorded node's value came from; references are arena indices until the tree
/// is built.
#[derive(Clone, Copy, Debug)]
enum Source {
    Evaluated,
    MemoHit(Option<usize>),
    Cycle(u32),
    CycleHit(Option<usize>),
    Skipped,
}

/// A node in the arena.
#[derive(Debug)]
struct Raw {
    node: String,
    kind: TraceKind,
    expr: Option<String>,
    role: Option<TraceRole>,
    result: Option<bool>,
    source: Source,
    provisional: bool,
    /// The value depended on a cycle value not yet settled when this node finished; its
    /// region's head clears this once it knows whether the value is exact.
    unsettled: bool,
    tuple: Option<String>,
    start: Instant,
    duration: Duration,
    truncated: bool,
    children: Vec<usize>,
}

/// Records every step of one check, up to `max` nodes.
pub(crate) struct Recorder {
    schema: Arc<CompiledSchema>,
    max: usize,
    nodes: Vec<Raw>,
    /// Nodes entered and not yet left; `None` for one past the cap.
    open: Vec<Option<usize>>,
    /// The most recent member node of each key: what a memo hit or cycle hit points at.
    latest: HashMap<NodeKey, usize>,
    /// The role of the next node recorded.
    role: Option<TraceRole>,
    truncated: bool,
}

impl Recorder {
    pub(crate) fn new(schema: Arc<CompiledSchema>, max: usize) -> Self {
        Self {
            schema,
            max,
            nodes: Vec::new(),
            open: Vec::new(),
            latest: HashMap::new(),
            role: None,
            truncated: false,
        }
    }

    /// Adds a node under the innermost open one; `None` past the cap.
    fn push(&mut self, node: String, kind: TraceKind, source: Source) -> Option<usize> {
        let role = self.role.take();
        let parent = self.open.last().copied();
        if self.nodes.len() >= self.max || matches!(parent, Some(None)) {
            self.truncated = true;
            if let Some(Some(p)) = parent {
                self.nodes[p].truncated = true;
            }
            return None;
        }
        let i = self.nodes.len();
        self.nodes.push(Raw {
            node,
            kind,
            expr: None,
            role,
            result: None,
            source,
            provisional: false,
            unsettled: false,
            tuple: None,
            start: Instant::now(),
            duration: Duration::ZERO,
            truncated: false,
            children: Vec::new(),
        });
        if let Some(Some(p)) = parent {
            self.nodes[p].children.push(i);
        }
        Some(i)
    }

    /// A finished leaf with `result`.
    fn leaf(&mut self, node: String, kind: TraceKind, source: Source, result: Option<bool>) {
        if let Some(i) = self.push(node, kind, source) {
            self.nodes[i].result = result;
        }
    }

    /// Leaves the innermost open node with `out`.
    fn finish(&mut self, out: &Outcome<bool>) -> Option<usize> {
        let i = self.open.pop().flatten()?;
        let n = &mut self.nodes[i];
        n.result = Some(out.value);
        n.unsettled = out.low.is_some();
        n.duration = n.start.elapsed();
        Some(i)
    }

    /// A region headed at `i` settled: every value it left unsettled is now known exact,
    /// except a `false` when the region settled by the head-`true` shortcut (`exact` is
    /// false), which may be below its real value. Settled subtrees were resolved by their
    /// own heads, so only unsettled nodes are visited.
    fn resolve(&mut self, i: usize, exact: bool) {
        let mut stack: Vec<usize> = self.nodes[i].children.clone();
        while let Some(j) = stack.pop() {
            let n = &mut self.nodes[j];
            if !n.unsettled {
                continue;
            }
            n.unsettled = false;
            if !exact && n.result == Some(false) {
                n.provisional = true;
            }
            stack.extend(n.children.iter().copied());
        }
    }

    fn member_kind(&self, key: &NodeKey) -> TraceKind {
        match self.schema.type_def(key.ty).and_then(|t| t.member(key.rel)) {
            Some(Member::Permission(p)) => expr_kind(&p.expr),
            _ => TraceKind::Relation,
        }
    }

    fn expr_text(&self, ty: TypeId, expr: &Expr) -> Option<String> {
        self.schema.type_def(ty).map(|t| t.expr_source(expr))
    }

    /// The tree. The first node recorded is the root's.
    pub(crate) fn into_tree(self) -> Option<(TraceNode, bool)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut b = Builder {
            nodes: &self.nodes,
            ids: vec![None; self.nodes.len()],
            next: 0,
            cycle_refs: Vec::new(),
        };
        let mut root = b.emit(0, None);
        let fix: HashMap<u32, Option<u32>> = b
            .cycle_refs
            .iter()
            .map(|&(id, target)| (id, b.ids[target]))
            .collect();
        if !fix.is_empty() {
            patch_cycle_refs(&mut root, &fix);
        }
        Some((root, self.truncated))
    }
}

fn expr_kind(expr: &Expr) -> TraceKind {
    match expr {
        Expr::Computed(_) => TraceKind::Computed,
        Expr::Union(_) => TraceKind::Union,
        Expr::Intersection(_) => TraceKind::Intersection,
        Expr::Exclusion { .. } => TraceKind::Exclusion,
        Expr::Arrow { mode, .. } => match mode {
            ArrowMode::Any => TraceKind::ArrowAny,
            ArrowMode::All => TraceKind::ArrowAll,
        },
    }
}

impl Tracer for Recorder {
    const ENABLED: bool = true;
    type Mark = Instant;

    fn mark(&self) -> Instant {
        Instant::now()
    }

    fn role(&mut self, role: TraceRole) {
        self.role = Some(role);
    }

    fn memo_hit(&mut self, key: &NodeKey, value: bool) {
        let of = self.latest.get(key).copied();
        let kind = self.member_kind(key);
        self.leaf(
            name(&self.schema, key),
            kind,
            Source::MemoHit(of),
            Some(value),
        );
    }

    fn cycle_hit(&mut self, key: &NodeKey, out: &Outcome<bool>) {
        let of = self.latest.get(key).copied();
        let kind = self.member_kind(key);
        if let Some(i) = self.push(name(&self.schema, key), kind, Source::CycleHit(of)) {
            let n = &mut self.nodes[i];
            n.result = Some(out.value);
            n.unsettled = out.low.is_some();
        }
    }

    fn self_member(&mut self, key: &NodeKey) {
        self.leaf(
            name(&self.schema, key),
            TraceKind::SelfMembership,
            Source::Evaluated,
            Some(true),
        );
    }

    fn open_member(&mut self, key: &NodeKey, member: &Member) {
        let (kind, expr) = match member {
            Member::Relation(_) => (TraceKind::Relation, None),
            Member::Permission(p) => (expr_kind(&p.expr), self.expr_text(key.ty, &p.expr)),
        };
        let i = self.push(name(&self.schema, key), kind, Source::Evaluated);
        if let Some(i) = i {
            self.nodes[i].expr = expr;
            self.latest.insert(key.clone(), i);
        }
        self.open.push(i);
    }

    fn rerun(&mut self) {
        if let Some(Some(i)) = self.open.last().copied() {
            let n = &mut self.nodes[i];
            n.children.clear();
            n.truncated = false;
        }
    }

    fn close_member(&mut self, out: &Outcome<bool>, settle: Settle, passes: u32) {
        let Some(i) = self.finish(out) else { return };
        if let Settle::Head { cyclic, exact } = settle {
            if cyclic {
                self.nodes[i].source = Source::Cycle(passes);
            }
            self.resolve(i, exact);
        }
    }

    fn point(&mut self, wildcard: bool, rel: &Relationship, found: bool, since: Instant) {
        let text = rel.to_string();
        let kind = if wildcard {
            TraceKind::Wildcard
        } else {
            TraceKind::Direct
        };
        let tuple = found.then(|| text.clone());
        if let Some(i) = self.push(text, kind, Source::Evaluated) {
            let n = &mut self.nodes[i];
            n.result = Some(found);
            n.tuple = tuple;
            n.start = since;
            n.duration = since.elapsed();
        }
    }

    fn open_subject_set(
        &mut self,
        resource: &ObjectRef,
        relation: &RelName,
        object: &ObjectRef,
        rel: &RelName,
    ) {
        let i = self.push(
            format!("{object}#{rel}"),
            TraceKind::SubjectSet,
            Source::Evaluated,
        );
        if let Some(i) = i {
            self.nodes[i].tuple = Some(format!("{resource}#{relation}@{object}#{rel}"));
        }
        self.open.push(i);
    }

    fn skipped_subject_set(
        &mut self,
        resource: &ObjectRef,
        relation: &RelName,
        object: &ObjectRef,
        rel: &RelName,
    ) {
        let i = self.push(
            format!("{object}#{rel}"),
            TraceKind::SubjectSet,
            Source::Skipped,
        );
        if let Some(i) = i {
            self.nodes[i].tuple = Some(format!("{resource}#{relation}@{object}#{rel}"));
        }
    }

    fn open_expr(&mut self, key: &NodeKey, expr: &Expr) {
        let i = self.push(name(&self.schema, key), expr_kind(expr), Source::Evaluated);
        if let Some(i) = i {
            self.nodes[i].expr = self.expr_text(key.ty, expr);
        }
        self.open.push(i);
    }

    fn close(&mut self, out: &Outcome<bool>) {
        self.finish(out);
    }

    fn skipped_expr(&mut self, key: &NodeKey, expr: &Expr) {
        if let Expr::Computed(rel) = expr {
            let target = NodeKey {
                ty: key.ty,
                id: key.id.clone(),
                rel: *rel,
            };
            let kind = self.member_kind(&target);
            self.leaf(name(&self.schema, &target), kind, Source::Skipped, None);
            return;
        }
        let i = self.push(name(&self.schema, key), expr_kind(expr), Source::Skipped);
        if let Some(i) = i {
            self.nodes[i].expr = self.expr_text(key.ty, expr);
        }
    }

    fn undefined(&mut self, x: &ObjectRef, computed: &str) {
        self.leaf(
            format!("{x}#{computed}"),
            TraceKind::Undefined,
            Source::Evaluated,
            Some(false),
        );
    }

    fn skipped_arrow_object(&mut self, x: &ObjectRef, computed: &str, target: Option<NodeKey>) {
        match target {
            Some(key) => {
                let kind = self.member_kind(&key);
                self.leaf(name(&self.schema, &key), kind, Source::Skipped, None);
            }
            None => self.leaf(
                format!("{x}#{computed}"),
                TraceKind::Undefined,
                Source::Skipped,
                None,
            ),
        }
    }
}

/// Flattens the arena into the tree, numbering nodes in pre-order.
struct Builder<'a> {
    nodes: &'a [Raw],
    /// The tree id each arena node was shown in full under.
    ids: Vec<Option<u32>>,
    next: u32,
    /// Cycle hits to point at their target's tree id once every id is known:
    /// `(hit's id, target's arena index)`.
    cycle_refs: Vec<(u32, usize)>,
}

impl Builder<'_> {
    /// Arena node `i` at this position, with this position's `role` (which differs from
    /// the node's own when it stands in for a memo hit elsewhere).
    fn emit(&mut self, i: usize, role: Option<TraceRole>) -> TraceNode {
        let raw = &self.nodes[i];
        let id = self.next;
        self.next += 1;
        // Already shown in full (this node sits inside a subtree shown at a memo hit's
        // place, and was also reached another way): a reference to it.
        if let Some(of) = self.ids[i] {
            return node(
                raw,
                id,
                role,
                TraceSource::MemoHit { of: Some(of) },
                Vec::new(),
            );
        }
        let source = match raw.source {
            Source::MemoHit(Some(target)) => {
                // Shown in full at its first occurrence, which may be this one when it
                // was evaluated in a pass that was later discarded.
                if self.ids[target].is_none() {
                    self.next -= 1;
                    return self.emit(target, role);
                }
                TraceSource::MemoHit {
                    of: self.ids[target],
                }
            }
            Source::MemoHit(None) => TraceSource::MemoHit { of: None },
            Source::CycleHit(target) => {
                if let Some(t) = target {
                    self.cycle_refs.push((id, t));
                }
                TraceSource::CycleHit { of: None }
            }
            Source::Evaluated => {
                self.ids[i] = Some(id);
                TraceSource::Evaluated
            }
            Source::Cycle(passes) => {
                self.ids[i] = Some(id);
                TraceSource::Cycle { passes }
            }
            Source::Skipped => TraceSource::Skipped,
        };
        let children = raw
            .children
            .iter()
            .map(|&c| self.emit(c, self.nodes[c].role))
            .collect();
        node(raw, id, role, source, children)
    }
}

fn node(
    raw: &Raw,
    id: u32,
    role: Option<TraceRole>,
    source: TraceSource,
    children: Vec<TraceNode>,
) -> TraceNode {
    let full = matches!(source, TraceSource::Evaluated | TraceSource::Cycle { .. });
    TraceNode {
        id,
        node: raw.node.clone(),
        kind: raw.kind,
        expr: raw.expr.clone(),
        role,
        result: raw.result,
        source,
        provisional: raw.provisional,
        tuple: raw.tuple.clone(),
        duration: if full { raw.duration } else { Duration::ZERO },
        truncated: full && raw.truncated,
        children,
    }
}

fn patch_cycle_refs(n: &mut TraceNode, fix: &HashMap<u32, Option<u32>>) {
    if let Some(&of) = fix.get(&n.id) {
        n.source = TraceSource::CycleHit { of };
    }
    for c in &mut n.children {
        patch_cycle_refs(c, fix);
    }
}
