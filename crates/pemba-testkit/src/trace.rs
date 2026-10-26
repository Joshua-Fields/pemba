//! The invariants every [`CheckTrace`] must satisfy (`docs/04-evaluation.md` §3.8),
//! checked on every generated case by the trace property in `tests/properties.rs`.

use pemba_core::{CheckTrace, TraceKind, TraceNode, TraceRole, TraceSource};

/// Every violation of `docs/04-evaluation.md` §3.8's invariants in `t`, one message each;
/// empty if there is none.
pub fn violations(t: &CheckTrace) -> Vec<String> {
    let mut out = Vec::new();
    if t.root.result != Some(t.allowed) {
        out.push(format!(
            "root result {:?} but allowed is {}",
            t.root.result, t.allowed
        ));
    }
    if t.root.role.is_some() {
        out.push("the root has a role".to_owned());
    }
    let nodes = t.nodes();
    for (i, n) in nodes.iter().enumerate() {
        if n.id as usize != i {
            out.push(format!("node {} is at pre-order position {i}", n.id));
        }
    }
    for n in &nodes {
        let mut fail = |what: String| {
            out.push(format!(
                "#{} {} ({}): {what}",
                n.id,
                n.node,
                n.kind.as_str()
            ));
        };
        shape(n, &nodes, &mut fail);
        if matches!(n.source, TraceSource::Evaluated | TraceSource::Cycle { .. }) && !n.truncated {
            value(n, &mut fail);
            skips(n, &mut fail);
        }
    }
    out
}

/// What holds of any node: sources, references, roles, tuples.
fn shape(n: &TraceNode, nodes: &[&TraceNode], fail: &mut impl FnMut(String)) {
    let skipped = n.source == TraceSource::Skipped;
    if skipped != n.result.is_none() {
        fail(format!("source {:?} with result {:?}", n.source, n.result));
    }
    if n.provisional && n.result != Some(false) {
        fail("provisional but not false".to_owned());
    }
    let full = matches!(n.source, TraceSource::Evaluated | TraceSource::Cycle { .. });
    if !full && !n.children.is_empty() {
        fail(format!("{} node with children", n.source.as_str()));
    }
    if let TraceSource::Cycle { passes } = n.source
        && passes == 0
    {
        fail("cycle with 0 passes".to_owned());
    }
    match n.source {
        TraceSource::MemoHit { of: Some(of) } | TraceSource::CycleHit { of: Some(of) } => {
            match nodes.get(of as usize) {
                None => fail(format!("refers to missing #{of}")),
                Some(target) => {
                    if target.node != n.node {
                        fail(format!("refers to #{of}, which is {}", target.node));
                    }
                    if !matches!(
                        target.source,
                        TraceSource::Evaluated | TraceSource::Cycle { .. }
                    ) {
                        fail(format!("refers to #{of}, which is not shown in full"));
                    }
                    if matches!(n.source, TraceSource::MemoHit { .. }) {
                        if of >= n.id {
                            fail(format!("memo hit refers forward to #{of}"));
                        }
                        if target.result != n.result {
                            fail(format!(
                                "memo hit {:?} but #{of} is {:?}",
                                n.result, target.result
                            ));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    for c in &n.children {
        let want = n.kind == TraceKind::Exclusion;
        if c.role.is_some() != want {
            fail(format!("child #{} has role {:?}", c.id, c.role));
        }
    }
    if matches!(n.kind, TraceKind::Direct | TraceKind::Wildcard)
        && n.result == Some(true)
        && n.tuple.as_deref() != Some(n.node.as_str())
    {
        fail(format!("true point query with tuple {:?}", n.tuple));
    }
    if n.kind == TraceKind::SubjectSet && n.tuple.is_none() {
        fail("subject set without its stored tuple".to_owned());
    }
}

/// A node shown in full: its result follows from its children's.
fn value(n: &TraceNode, fail: &mut impl FnMut(String)) {
    let evaluated: Vec<&TraceNode> = n
        .children
        .iter()
        .filter(|c| c.source != TraceSource::Skipped)
        .collect();
    let any = evaluated.iter().any(|c| c.result == Some(true));
    let all = evaluated.iter().all(|c| c.result == Some(true));
    let only_child = || match n.children.as_slice() {
        [c] => c.result,
        _ => None,
    };
    let expected = match n.kind {
        TraceKind::Relation | TraceKind::Union | TraceKind::ArrowAny => Some(any),
        TraceKind::Intersection => Some(!evaluated.is_empty() && all),
        TraceKind::ArrowAll => Some(!n.children.is_empty() && all),
        TraceKind::Exclusion => match n.children.as_slice() {
            [b, s] if b.role == Some(TraceRole::Base) && s.role == Some(TraceRole::Subtracted) => {
                Some(b.result == Some(true) && s.result != Some(true))
            }
            _ => {
                fail("exclusion without a base and a subtracted child".to_owned());
                return;
            }
        },
        TraceKind::Computed | TraceKind::SubjectSet => {
            if n.children.len() != 1 {
                fail(format!("{} children, want 1", n.children.len()));
                return;
            }
            only_child()
        }
        TraceKind::SelfMembership => Some(true),
        TraceKind::Direct | TraceKind::Wildcard | TraceKind::Undefined => {
            if !n.children.is_empty() {
                fail("leaf with children".to_owned());
            }
            if n.kind == TraceKind::Undefined {
                Some(false)
            } else {
                n.result
            }
        }
        _ => n.result,
    };
    if expected != n.result {
        fail(format!("result {:?}, children say {expected:?}", n.result));
    }
    if n.kind == TraceKind::Relation {
        let kinds_ok = n
            .children
            .first()
            .is_some_and(|c| c.kind == TraceKind::Direct)
            && n.children
                .iter()
                .skip(1)
                .all(|c| matches!(c.kind, TraceKind::Wildcard | TraceKind::SubjectSet));
        if !kinds_ok {
            fail("relation children are not direct, then wildcard / subject sets".to_owned());
        }
    }
}

/// Skipped siblings come only after the one that decided the parent, and nothing
/// evaluated follows them.
fn skips(n: &TraceNode, fail: &mut impl FnMut(String)) {
    if let Some(k) = n
        .children
        .iter()
        .position(|c| c.source == TraceSource::Skipped)
    {
        if n.children[k..]
            .iter()
            .any(|c| c.source != TraceSource::Skipped)
        {
            fail("an evaluated child after a skipped one".to_owned());
        }
        let before = k.checked_sub(1).map(|j| n.children[j].result);
        let ok = match n.kind {
            TraceKind::Relation | TraceKind::Union | TraceKind::ArrowAny => {
                before == Some(Some(true))
            }
            TraceKind::Intersection | TraceKind::ArrowAll => before == Some(Some(false)),
            TraceKind::Exclusion => k == 1 && before == Some(Some(false)),
            _ => false,
        };
        if !ok {
            fail(format!(
                "child #{} skipped, but no sibling before it decided",
                n.children[k].id
            ));
        }
    }
}
