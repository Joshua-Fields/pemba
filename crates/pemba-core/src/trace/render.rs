//! The text form of a [`CheckTrace`]: what `pemba check --explain` prints and Python's
//! `CheckTrace.render()` returns (`docs/06-api.md` §3, OQ-043 (g)).
//!
//! One line per node, drawn as a tree. The *granting path* is the set of nodes that decided
//! the answer: from the root down, the children that decided each node (the `true` child
//! of an OR, every child of a `true` AND, and so on). An exclusion denied by its subtracted
//! side is decided by both sides: the path runs through the base's grant and through what
//! removed it. Evaluated nodes off the path are marked `(not needed)` once, at the top of
//! their branch. A subject set is drawn as the member it names, and a relation shows the
//! stored tuple that matched instead of its point queries, so the tree reads as a chain of
//! members.
//!
//! Colour only styles the same text (`docs/04-evaluation.md` §3.8): the path bold, its
//! glyphs green or red; evaluated nodes off it in the terminal's normal colour; dim only
//! for lines that show no evaluation of their own, skipped nodes and memo references.

use std::fmt::Write as _;

use super::{CheckTrace, TraceKind, TraceNode, TraceRole, TraceSource};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

/// Where a node stands relative to the granting path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    /// It decided the answer.
    On,
    /// The top of an evaluated branch that did not.
    Leaves,
    /// Inside such a branch, or skipped.
    Off,
}

pub(super) fn render(trace: &CheckTrace, color: bool) -> String {
    let mut out = String::new();
    let verdict = if trace.allowed { "allowed" } else { "denied" };
    let _ = writeln!(
        out,
        "{verdict}: {}#{}@{} (revision {})",
        trace.resource, trace.permission, trace.subject, trace.revision
    );
    if trace.truncated {
        out.push_str(
            "note: the trace reached its node limit (max_trace_nodes) and is incomplete; the \
             answer is not\n",
        );
    }
    out.push('\n');
    let r = Renderer { color };
    r.line(&mut out, "", "", &trace.root, None, Path::On);
    r.children(&mut out, "", &trace.root, Path::On);
    out
}

struct Renderer {
    color: bool,
}

impl Renderer {
    /// `n`'s own line: `lead` then the branch glyph, then the node.
    fn line(
        &self,
        out: &mut String,
        lead: &str,
        branch: &str,
        n: &TraceNode,
        parent: Option<&TraceNode>,
        path: Path,
    ) {
        // A subject set is drawn as the member it names, with the stored tuple.
        let (shown, via) = shown(n);
        let mut text = String::new();
        if let Some(role) = n.role {
            text.push_str(match role {
                TraceRole::Base => "base: ",
                TraceRole::Subtracted => "subtracted: ",
            });
        }
        text.push_str(&label(shown, parent));
        if let Some(t) = via {
            let _ = write!(text, "  via {t}");
        }
        if let Some(t) = matched_tuple(shown) {
            let _ = write!(text, "  ← {t}");
        }
        let mut notes = suffixes(n, shown);
        if path == Path::Leaves {
            notes.push("not needed".to_owned());
        }
        if !notes.is_empty() {
            let _ = write!(text, "  ({})", notes.join("; "));
        }

        let glyph = match n.result {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "·",
        };
        out.push_str(lead);
        out.push_str(branch);
        if !self.color {
            let _ = writeln!(out, "{glyph} {text}");
            return;
        }
        match path {
            Path::On => {
                let c = if n.result == Some(true) { GREEN } else { RED };
                let _ = writeln!(out, "{BOLD}{c}{glyph}{RESET}{BOLD} {text}{RESET}");
            }
            Path::Leaves | Path::Off if is_reference(n) || is_reference(shown) => {
                let _ = writeln!(out, "{DIM}{glyph} {text}{RESET}");
            }
            Path::Leaves | Path::Off => {
                let _ = writeln!(out, "{glyph} {text}");
            }
        }
    }

    /// The lines below `n`, which is drawn at `path`.
    fn children(&self, out: &mut String, lead: &str, n: &TraceNode, path: Path) {
        let (shown, _) = shown(n);
        let kids: Vec<&TraceNode> = shown.children.iter().filter(|c| drawn(shown, c)).collect();
        let decisive = decisive(shown);
        let truncated = shown.truncated;
        for (i, c) in kids.iter().enumerate() {
            let last = i + 1 == kids.len() && !truncated;
            let (branch, next) = if last {
                ("└── ", "    ")
            } else {
                ("├── ", "│   ")
            };
            let cpath = match path {
                Path::On if c.source == TraceSource::Skipped => Path::Off,
                Path::On if decisive.contains(&c.id) => Path::On,
                Path::On => Path::Leaves,
                Path::Leaves | Path::Off => Path::Off,
            };
            self.line(out, lead, branch, c, Some(shown), cpath);
            self.children(out, &format!("{lead}{next}"), c, cpath);
        }
        if truncated {
            let _ = writeln!(out, "{lead}└── … (not recorded: trace node limit reached)");
        }
    }
}

/// The node to draw for `n` and the stored tuple it was reached through: a subject set
/// stands in for its member.
fn shown(n: &TraceNode) -> (&TraceNode, Option<&str>) {
    match (n.kind, n.children.as_slice()) {
        (TraceKind::SubjectSet, [member]) => (member, n.tuple.as_deref()),
        _ => (
            n,
            if n.kind == TraceKind::SubjectSet {
                n.tuple.as_deref()
            } else {
                None
            },
        ),
    }
}

/// A line that shows no evaluation of its own: skipped, or a memo hit referring to the
/// node shown in full elsewhere.
fn is_reference(n: &TraceNode) -> bool {
    matches!(n.source, TraceSource::Skipped | TraceSource::MemoHit { .. })
}

/// A relation's point queries are folded into its line (the match) or left out (a miss).
fn drawn(parent: &TraceNode, c: &TraceNode) -> bool {
    !(parent.kind == TraceKind::Relation
        && matches!(c.kind, TraceKind::Direct | TraceKind::Wildcard))
}

/// The stored tuple that made a relation `true` directly.
fn matched_tuple(n: &TraceNode) -> Option<&str> {
    if n.kind != TraceKind::Relation {
        return None;
    }
    n.children
        .iter()
        .find(|c| {
            matches!(c.kind, TraceKind::Direct | TraceKind::Wildcard) && c.result == Some(true)
        })
        .and_then(|c| c.tuple.as_deref())
}

/// `n`'s text without suffixes: a member as `type:id#member` (`= expr` for a permission),
/// an operand of the same permission as its expression alone.
fn label(n: &TraceNode, parent: Option<&TraceNode>) -> String {
    let operand = matches!(n.source, TraceSource::Evaluated | TraceSource::Skipped)
        && is_operator(n.kind)
        && parent.is_some_and(|p| p.node == n.node && is_operator(p.kind));
    match (&n.expr, operand) {
        (Some(e), true) => e.clone(),
        (Some(e), false) => format!("{} = {e}", n.node),
        (None, _) => n.node.clone(),
    }
}

fn is_operator(k: TraceKind) -> bool {
    matches!(
        k,
        TraceKind::Computed
            | TraceKind::Union
            | TraceKind::Intersection
            | TraceKind::Exclusion
            | TraceKind::ArrowAny
            | TraceKind::ArrowAll
    )
}

/// The notes in parentheses after a node: where its value came from, and why it may say
/// little.
fn suffixes(n: &TraceNode, shown: &TraceNode) -> Vec<String> {
    let mut s = Vec::new();
    // A reference names where the node is shown in full; `None` only past the node cap
    // (memo hits) or when that node is in no pass the trace kept (`04` §3.8, "unresolved").
    let refer = |of: Option<u32>, what: &str| match of {
        Some(of) if of < shown.id => format!("{what}see above"),
        Some(_) => format!("{what}see below"),
        None if what.is_empty() => "not recorded: trace node limit reached".to_owned(),
        None => format!("{what}unresolved: that node is not in this trace"),
    };
    match shown.source {
        TraceSource::Evaluated => {}
        TraceSource::MemoHit { of } => s.push(refer(of, "")),
        TraceSource::Cycle { passes } => s.push(format!(
            "cycle: settled after {passes} pass{}",
            if passes == 1 { "" } else { "es" }
        )),
        TraceSource::CycleHit { of } => s.push(refer(of, "cycle: reached again, ")),
        TraceSource::Skipped => s.push("skipped".to_owned()),
    }
    match shown.kind {
        TraceKind::SelfMembership => s.push("the subject itself".to_owned()),
        TraceKind::Undefined => s.push("no such member: contributes nothing".to_owned()),
        TraceKind::Relation
            if shown.result == Some(false)
                && shown.source == TraceSource::Evaluated
                && !shown
                    .children
                    .iter()
                    .any(|c| c.kind == TraceKind::SubjectSet) =>
        {
            s.push("no matching relationship".to_owned());
        }
        TraceKind::ArrowAny | TraceKind::ArrowAll
            if shown.result == Some(false)
                && shown.source == TraceSource::Evaluated
                && shown.children.is_empty() =>
        {
            s.push("no related objects".to_owned());
        }
        _ => {}
    }
    if shown.provisional || n.provisional {
        s.push("provisional: computed from a cycle assumption that did not hold".to_owned());
    }
    s
}

/// The ids of the children that decided `n`'s value.
fn decisive(n: &TraceNode) -> Vec<u32> {
    let evaluated: Vec<&TraceNode> = n
        .children
        .iter()
        .filter(|c| c.source != TraceSource::Skipped)
        .collect();
    let first = |want: bool| -> Vec<u32> {
        evaluated
            .iter()
            .find(|c| c.result == Some(want))
            .map(|c| vec![c.id])
            .unwrap_or_default()
    };
    let all = || evaluated.iter().map(|c| c.id).collect();
    match (n.kind, n.result) {
        (_, None) => Vec::new(),
        (TraceKind::Relation | TraceKind::Union | TraceKind::ArrowAny, Some(true)) => first(true),
        (TraceKind::Intersection | TraceKind::ArrowAll, Some(false)) => first(false),
        (TraceKind::Exclusion, Some(false)) => match evaluated.as_slice() {
            // Removed by the subtracted side: the grant and what removed it.
            [base, sub] if base.result == Some(true) => vec![base.id, sub.id],
            [base] | [base, _] => vec![base.id],
            _ => all(),
        },
        _ => all(),
    }
}
