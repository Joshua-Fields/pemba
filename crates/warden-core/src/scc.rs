//! SCC fixed-point iteration over data-dependent cycles (`docs/04-evaluation.md` §3.6),
//! shared by `check` (values are `bool`) and `lookup_subjects` (values are subject sets,
//! §6.2).
//!
//! A node is pushed on the SCC stack on first visit and stays there until the head of its
//! region settles; an [`Outcome`]'s `low` is the lowest SCC-stack position whose unsettled
//! value it used. A hit on a node in progress returns its provisional value (`⊥` on the
//! first pass) and records what it assumed; a hit on a finished member returns its value
//! from this pass. When the head finishes, the region is settled into the memo if no value
//! grew past what a hit assumed for it (or the head is the top value), else the head
//! re-runs with the values found. Values only grow, so this ends, at the least fixed point.
//!
//! Negation is stratified: every frame records its negation depth, the number of
//! subtracted sides it sits inside. A cycle whose path from the repeated node to the
//! closing edge crosses a subtracted side has no least fixed point, so it is an error; it
//! is exactly the cycle whose ends differ in depth, since depth never falls along the
//! stack. A monotone cycle wholly inside a subtracted side settles like any other region,
//! before the exclusion negates its value.
//!
//! The evaluators own everything else (reads, limits, operators); this module only decides
//! what a repeated node is worth and when a region is settled.

use std::collections::HashMap;

use crate::error::Error;
use crate::eval::{NodeKey, invalid};

/// The values a node can take: a join-semilattice with a bottom and a top, ordered by `≤`.
pub(crate) trait Lattice: Clone + PartialEq {
    /// `⊥`: what a node in progress is assumed to be on the first pass.
    fn bottom() -> Self;
    /// `self := self ∪ other`.
    fn join(&mut self, other: &Self);
    /// `self ≤ other`.
    fn le(&self, other: &Self) -> bool;
    /// The greatest value. A top value is sound whatever was assumed to reach it, so it
    /// settles without iterating further.
    fn is_top(&self) -> bool;
}

impl Lattice for bool {
    fn bottom() -> Self {
        false
    }
    fn join(&mut self, other: &Self) {
        *self |= *other;
    }
    fn le(&self, other: &Self) -> bool {
        !*self || *other
    }
    fn is_top(&self) -> bool {
        *self
    }
}

/// A node's value and the lowest SCC-stack position whose unsettled value it used (`None`
/// if it used none, so the value is settled).
#[derive(Clone, Debug)]
pub(crate) struct Outcome<V> {
    pub(crate) value: V,
    pub(crate) low: Option<usize>,
}

impl<V> Outcome<V> {
    /// A settled value.
    pub(crate) fn settled(value: V) -> Self {
        Self { value, low: None }
    }
}

/// The lower of two lowlinks, `None` meaning "settled".
pub(crate) fn min_low(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// A node on the SCC stack: pushed on first visit, removed when its region's head settles.
/// Positions are stable while it is there (the stack only loses suffixes), unlike call-stack
/// indices, which a later sibling reuses once a frame pops.
#[derive(Debug)]
struct Entry<V> {
    key: NodeKey,
    /// Call-stack index while in progress; `None` once finished in this pass.
    frame: Option<usize>,
    /// The finished value (meaningful once `frame` is `None`, and for the head).
    value: V,
    /// Lowest SCC-stack position this node's value depends on.
    low: usize,
    /// The value a cycle hit returned for this node while it was in progress this pass.
    assumed: Option<V>,
}

/// A node in progress, with its negation depth: how many subtracted sides of exclusions
/// it was pushed inside.
#[derive(Debug)]
struct Frame {
    key: NodeKey,
    neg_depth: u32,
}

/// What [`Scc::hit`] found for a node already on the SCC stack.
pub(crate) enum Hit<V> {
    /// Use this value.
    Found(Outcome<V>),
    /// The cycle closed through a subtracted side; the cycle starts at call-stack frame
    /// `from` (see [`Scc::path_from`]).
    NonMonotonic {
        /// Call-stack index of the repeated node, or of its region's head.
        from: usize,
    },
}

/// What [`Scc::finish`] decided for a node whose expression was just evaluated.
pub(crate) enum Finish<V> {
    /// The node's value for this pass (settled if `low` is `None`).
    Done(Outcome<V>),
    /// The node heads a region whose assumptions failed: evaluate it again.
    Rerun,
}

/// SCC stack, call stack, provisional values and negation depth for one evaluation.
#[derive(Debug)]
pub(crate) struct Scc<V> {
    stack: Vec<Frame>,
    entries: Vec<Entry<V>>,
    pos: HashMap<NodeKey, usize>,
    /// Values found by earlier passes of a head still iterating; they only ever grow.
    provisional: HashMap<NodeKey, V>,
    /// How many subtracted sides of exclusions evaluation is currently inside.
    neg_depth: u32,
}

impl<V: Lattice> Scc<V> {
    pub(crate) fn new() -> Self {
        Self {
            stack: Vec::new(),
            entries: Vec::new(),
            pos: HashMap::new(),
            provisional: HashMap::new(),
            neg_depth: 0,
        }
    }

    /// Clears everything; unsettled values must not leak into the next evaluation.
    pub(crate) fn reset(&mut self) {
        self.stack.clear();
        self.entries.clear();
        self.pos.clear();
        self.provisional.clear();
        self.neg_depth = 0;
    }

    /// Nothing on any stack and no provisional values: the state between evaluations.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.stack.is_empty()
            && self.entries.is_empty()
            && self.pos.is_empty()
            && self.provisional.is_empty()
            && self.neg_depth == 0
    }

    /// The nodes in progress from call-stack frame `from` outwards, for error paths.
    pub(crate) fn path_from(&self, from: usize) -> impl Iterator<Item = &NodeKey> {
        self.stack
            .get(from..)
            .unwrap_or_default()
            .iter()
            .map(|f| &f.key)
    }

    /// Entering the subtracted side of an exclusion.
    pub(crate) fn enter_sub(&mut self) {
        self.neg_depth += 1;
    }

    /// Leaving it.
    pub(crate) fn leave_sub(&mut self) {
        self.neg_depth -= 1;
    }

    /// Whether evaluation is under at least one subtracted side.
    pub(crate) fn negated(&self) -> bool {
        self.neg_depth > 0
    }

    /// `key`'s value if it is on the SCC stack (in progress, or finished this pass);
    /// `None` if it is not, and must be evaluated.
    ///
    /// # Errors
    ///
    /// Internal errors only (an inconsistent stack).
    pub(crate) fn hit(&mut self, key: &NodeKey) -> Result<Option<Hit<V>>, Error> {
        let Some(&p) = self.pos.get(key) else {
            return Ok(None);
        };
        let Some(entry) = self.entries.get(p) else {
            return Err(invalid(format!("internal: SCC position {p} out of range")));
        };
        if let Some(i) = entry.frame {
            // Depth never falls along the stack, so the path from `key` (its own
            // expression included) to this closing edge crosses a subtracted side exactly
            // when the depths differ. In `view = viewer - parent->view` with
            // `doc:a#parent@doc:a` the only frame is `doc:a#view`, at depth 0; the edge
            // back to it is evaluated at depth 1.
            if self.crosses_negation(i) {
                return Ok(Some(Hit::NonMonotonic { from: i }));
            }
            let value = self.provisional.get(key).cloned().unwrap_or_else(V::bottom);
            self.entries[p].assumed = Some(value.clone());
            return Ok(Some(Hit::Found(Outcome {
                value,
                low: Some(p),
            })));
        }
        // A finished member: its value depends on its region's head, still in progress
        // (`low` names it; every member shares its head's depth). So this hit closes a
        // cycle through the head, and is non-monotone if the path from the head to here
        // crosses a subtracted side.
        let (value, low) = (entry.value.clone(), entry.low);
        let Some(i) = self
            .entries
            .get(low..)
            .and_then(|r| r.iter().find_map(|e| e.frame))
        else {
            return Err(invalid(format!(
                "internal: no node in progress at or above SCC position {low}"
            )));
        };
        if self.crosses_negation(i) {
            return Ok(Some(Hit::NonMonotonic { from: i }));
        }
        Ok(Some(Hit::Found(Outcome {
            value,
            low: Some(low),
        })))
    }

    /// Pushes `key` on the SCC stack on its first visit; returns its position.
    pub(crate) fn begin(&mut self, key: &NodeKey) -> usize {
        let p = self.entries.len();
        self.push_entry(key, p);
        self.pos.insert(key.clone(), p);
        p
    }

    fn push_entry(&mut self, key: &NodeKey, p: usize) {
        self.entries.push(Entry {
            key: key.clone(),
            frame: Some(self.stack.len()),
            value: V::bottom(),
            low: p,
            assumed: None,
        });
    }

    /// Pushes `key` on the call stack for one pass of its evaluation.
    pub(crate) fn push_frame(&mut self, key: &NodeKey) {
        self.stack.push(Frame {
            key: key.clone(),
            neg_depth: self.neg_depth,
        });
    }

    /// Pops the frame [`push_frame`](Self::push_frame) pushed.
    pub(crate) fn pop_frame(&mut self) {
        self.stack.pop();
    }

    /// `key`, at SCC position `p`, evaluated to `out` this pass: finish it as a member of a
    /// region headed further down, settle its region into `memo`, or ask for a re-run.
    pub(crate) fn finish(
        &mut self,
        key: &NodeKey,
        p: usize,
        out: Outcome<V>,
        memo: &mut HashMap<NodeKey, V>,
    ) -> Finish<V> {
        // Never below what a cycle hit assumed, so a re-run is only ever caused by an
        // assumed value that grew.
        let mut value = out.value;
        if let Some(prov) = self.provisional.get(key) {
            value.join(prov);
        }

        if let Some(l) = out.low.filter(|&l| l < p) {
            // A member of a region headed further down: finish for this pass. Nodes
            // above it on the SCC stack are in that region too.
            let entry = &mut self.entries[p];
            entry.frame = None;
            entry.value = value.clone();
            entry.low = l;
            for e in &mut self.entries[p + 1..] {
                e.low = e.low.min(l);
            }
            return Finish::Done(Outcome {
                value,
                low: Some(l),
            });
        }

        // `key` heads the region `entries[p..]`.
        self.entries[p].value = value.clone();
        let grew = self.entries[p..]
            .iter()
            .any(|e| e.assumed.as_ref().is_some_and(|a| !e.value.le(a)));
        if value.is_top() || !grew {
            // Consistent: every value is the least fixed point. Or the head is the top,
            // which is sound whatever was assumed, as is every member's top.
            for e in self.entries.drain(p..) {
                self.pos.remove(&e.key);
                self.provisional.remove(&e.key);
                if e.value.is_top() || !grew {
                    memo.insert(e.key, e.value);
                }
            }
            return Finish::Done(Outcome::settled(value));
        }

        let region: Vec<Entry<V>> = self.entries.drain(p..).collect();
        for e in region {
            if e.key != *key {
                self.pos.remove(&e.key);
            }
            self.provisional
                .entry(e.key)
                .or_insert_with(V::bottom)
                .join(&e.value);
        }
        self.push_entry(key, p);
        Finish::Rerun
    }

    /// Whether the path from call-stack frame `i` to the current point of evaluation
    /// crosses the subtracted side of an exclusion.
    fn crosses_negation(&self, i: usize) -> bool {
        self.stack
            .get(i)
            .is_none_or(|f| f.neg_depth != self.neg_depth)
    }
}
