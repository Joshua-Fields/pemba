//! `check_with_trace`'s answer: the tree of steps one `check` took
//! (`docs/04-evaluation.md` §3.8), its public JSON form (`docs/06-api.md` §3.1) and the text
//! `pemba check --explain` prints.
//!
//! The evaluator is generic over a [`Tracer`]. `check` uses [`NoTrace`], whose hooks are
//! empty and inlined, so tracing costs nothing when it is off; `check_with_trace` uses the
//! [`Recorder`], which builds the tree.

mod record;
mod render;

use std::fmt::Write as _;
use std::time::Duration;

use crate::eval::EvalStats;
use crate::model::{ObjectRef, RelName, Revision, SubjectRef};

pub(crate) use record::{NoTrace, Recorder, Tracer};

/// One `check`, step by step (`docs/04-evaluation.md` §3.8).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckTrace {
    /// The object the permission was checked on.
    pub resource: ObjectRef,
    /// The permission or relation checked.
    pub permission: RelName,
    /// The subject checked.
    pub subject: SubjectRef,
    /// The answer; always the root's result.
    pub allowed: bool,
    /// The revision the check was evaluated at.
    pub revision: Revision,
    /// What the evaluation cost, as in the `CheckResponse`.
    pub stats: EvalStats,
    /// Some nodes were not recorded because the trace reached
    /// `EngineConfig::max_trace_nodes`; the answer is complete regardless.
    pub truncated: bool,
    /// The requested `resource#permission`.
    pub root: TraceNode,
}

/// One step of a [`CheckTrace`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraceNode {
    /// Position in a pre-order walk of the tree (root `0`), what references point at.
    pub id: u32,
    /// What was evaluated: `type:id#member`, or for [`TraceKind::Direct`] and
    /// [`TraceKind::Wildcard`] the relationship queried.
    pub node: String,
    /// How it was evaluated.
    pub kind: TraceKind,
    /// The expression's canonical source text, for permissions and operators.
    pub expr: Option<String>,
    /// Which side of an exclusion this node is, for an exclusion's two children.
    pub role: Option<TraceRole>,
    /// The value; `None` exactly when [`source`](Self::source) is
    /// [`TraceSource::Skipped`].
    pub result: Option<bool>,
    /// Where the value came from.
    pub source: TraceSource,
    /// A `false` computed from a cycle assumption that turned out wrong; it says nothing
    /// about this node's real value (`docs/04-evaluation.md` §3.8).
    pub provisional: bool,
    /// The stored relationship: the match of a `true` direct or wildcard query, or the
    /// stored subject set of a [`TraceKind::SubjectSet`].
    pub tuple: Option<String>,
    /// Wall time inside this node, children included; zero for hits and skipped nodes.
    pub duration: Duration,
    /// Some children were not recorded (the node cap was reached).
    pub truncated: bool,
    /// The steps this node took, in evaluation order.
    pub children: Vec<TraceNode>,
}

/// How a [`TraceNode`] was evaluated (`docs/04-evaluation.md` §3.8).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TraceKind {
    /// A relation, from its stored tuples.
    Relation,
    /// An exact point query for the subject.
    Direct,
    /// A point query for a wildcard of the subject's type.
    Wildcard,
    /// A stored subject set, evaluated as its member.
    SubjectSet,
    /// The subject is this subject set itself.
    SelfMembership,
    /// A permission defined as another member's name.
    Computed,
    /// `a + b`.
    Union,
    /// `a & b`.
    Intersection,
    /// `a - b`.
    Exclusion,
    /// `t->p`: any related object.
    ArrowAny,
    /// `t.all(p)`: every related object.
    ArrowAll,
    /// A related object whose type has no member of the arrow's name: no subjects.
    Undefined,
}

impl TraceKind {
    /// The name used in JSON and rendered text, e.g. `arrow_any`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relation => "relation",
            Self::Direct => "direct",
            Self::Wildcard => "wildcard",
            Self::SubjectSet => "subject_set",
            Self::SelfMembership => "self",
            Self::Computed => "computed",
            Self::Union => "union",
            Self::Intersection => "intersection",
            Self::Exclusion => "exclusion",
            Self::ArrowAny => "arrow_any",
            Self::ArrowAll => "arrow_all",
            Self::Undefined => "undefined",
        }
    }
}

/// Where a [`TraceNode`]'s value came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TraceSource {
    /// Evaluated here.
    Evaluated,
    /// A settled value from earlier in this check; the node is shown in full at `of`.
    MemoHit {
        /// The [`TraceNode::id`] shown in full; `None` if it was not recorded.
        of: Option<u32>,
    },
    /// The head of a cyclic region, settled at its least fixed point after `passes`
    /// passes.
    Cycle {
        /// Passes over the region, at least 1.
        passes: u32,
    },
    /// Reached again while on the evaluation stack in the same pass: the value the
    /// evaluator used there.
    CycleHit {
        /// The [`TraceNode::id`] reached again; `None` if it was not recorded.
        of: Option<u32>,
    },
    /// Never evaluated: a sibling had already decided the parent.
    Skipped,
}

impl TraceSource {
    /// The name used in JSON, e.g. `memo_hit`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Evaluated => "evaluated",
            Self::MemoHit { .. } => "memo_hit",
            Self::Cycle { .. } => "cycle",
            Self::CycleHit { .. } => "cycle_hit",
            Self::Skipped => "skipped",
        }
    }

    /// The node a hit refers to.
    pub const fn reference(self) -> Option<u32> {
        match self {
            Self::MemoHit { of } | Self::CycleHit { of } => of,
            _ => None,
        }
    }
}

/// Which side of an exclusion a node is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TraceRole {
    /// `a` in `a - b`.
    Base,
    /// `b` in `a - b`.
    Subtracted,
}

impl TraceRole {
    /// `base` or `subtracted`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Subtracted => "subtracted",
        }
    }
}

impl TraceNode {
    /// This node and every node below it, in pre-order (so in [`id`](Self::id) order).
    pub fn walk(&self) -> Vec<&Self> {
        fn go<'a>(n: &'a TraceNode, out: &mut Vec<&'a TraceNode>) {
            out.push(n);
            for c in &n.children {
                go(c, out);
            }
        }
        let mut out = Vec::new();
        go(self, &mut out);
        out
    }
}

/// The version of the JSON format [`CheckTrace::to_json`] writes.
pub const TRACE_FORMAT_VERSION: u32 = 1;

impl CheckTrace {
    /// Every node, in pre-order: `nodes()[i].id == i`.
    pub fn nodes(&self) -> Vec<&TraceNode> {
        self.root.walk()
    }

    /// The text `pemba check --explain` prints, without colour (`docs/06-api.md` §3).
    pub fn render(&self) -> String {
        render::render(self, false)
    }

    /// [`render`](Self::render) with ANSI colour, the same text once the escapes are
    /// removed: the granting path bold, evaluated nodes off it in the normal colour, skipped
    /// nodes and memo references dimmed.
    pub fn render_ansi(&self) -> String {
        render::render(self, true)
    }

    /// The public JSON form (`docs/06-api.md` §3.1): every key always present, in a fixed
    /// order, `null` where it does not apply.
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(256 + 256 * self.root.walk().len());
        out.push_str("{\"format\":\"pemba.check_trace\",\"version\":");
        let _ = write!(out, "{TRACE_FORMAT_VERSION}");
        out.push_str(",\"resource\":");
        json_str(&mut out, &self.resource.to_string());
        out.push_str(",\"permission\":");
        json_str(&mut out, self.permission.as_str());
        out.push_str(",\"subject\":");
        json_str(&mut out, &self.subject.to_string());
        let s = &self.stats;
        let _ = write!(
            out,
            ",\"allowed\":{},\"revision\":{},\"truncated\":{},\"stats\":{{\"store_reads\":{},\
             \"tuples_read\":{},\"memo_hits\":{},\"max_depth_reached\":{},\"cycle_passes\":{},\
             \"elapsed_ns\":{}}},\"root\":",
            self.allowed,
            self.revision.0,
            self.truncated,
            s.store_reads,
            s.tuples_read,
            s.memo_hits,
            s.max_depth_reached,
            s.cycle_passes,
            nanos(s.elapsed),
        );
        json_node(&mut out, &self.root);
        out.push('}');
        out
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

fn json_node(out: &mut String, n: &TraceNode) {
    let _ = write!(out, "{{\"id\":{},\"node\":", n.id);
    json_str(out, &n.node);
    out.push_str(",\"kind\":");
    json_str(out, n.kind.as_str());
    out.push_str(",\"expr\":");
    json_opt_str(out, n.expr.as_deref());
    out.push_str(",\"role\":");
    json_opt_str(out, n.role.map(TraceRole::as_str));
    out.push_str(",\"result\":");
    match n.result {
        Some(b) => {
            let _ = write!(out, "{b}");
        }
        None => out.push_str("null"),
    }
    out.push_str(",\"source\":");
    json_str(out, n.source.as_str());
    out.push_str(",\"ref\":");
    match n.source.reference() {
        Some(id) => {
            let _ = write!(out, "{id}");
        }
        None => out.push_str("null"),
    }
    out.push_str(",\"passes\":");
    match n.source {
        TraceSource::Cycle { passes } => {
            let _ = write!(out, "{passes}");
        }
        _ => out.push_str("null"),
    }
    let _ = write!(out, ",\"provisional\":{},\"tuple\":", n.provisional);
    json_opt_str(out, n.tuple.as_deref());
    let _ = write!(
        out,
        ",\"duration_ns\":{},\"truncated\":{},\"children\":[",
        nanos(n.duration),
        n.truncated
    );
    for (i, c) in n.children.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_node(out, c);
    }
    out.push_str("]}");
}

fn json_opt_str(out: &mut String, s: Option<&str>) {
    match s {
        Some(s) => json_str(out, s),
        None => out.push_str("null"),
    }
}

/// `s` as a JSON string literal (RFC 8259 §7): quotes, backslashes and control characters
/// escaped, everything else as is.
pub(crate) fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use proptest::prelude::*;

    use super::*;

    proptest! {
        // At least 2 000, or `PROPTEST_CASES` if higher (the weekly soak, proptest.yml).
        #![proptest_config(ProptestConfig::with_cases(ProptestConfig::default().cases.max(2000)))]

        /// Any string at all, with a quote, a backslash, a control character and non-ASCII
        /// always in it (none of which the id grammar admits today, `docs/02` §1.2), comes
        /// back exactly from a real JSON parser.
        #[test]
        fn json_strings_round_trip_through_serde_json(s in any::<String>(), c in 0u32..0x20) {
            let s = format!("{s}\"\\{}é\u{10FFFF}", char::from_u32(c).unwrap());
            let mut out = String::new();
            json_str(&mut out, &s);
            let back: String = serde_json::from_str(&out).unwrap();
            prop_assert_eq!(back, s);
        }
    }

    #[test]
    fn json_strings_escape_quotes_backslashes_and_controls() {
        let mut out = String::new();
        json_str(&mut out, "a\"b\\c\nd\u{1}é");
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\u0001é\"");
    }
}
