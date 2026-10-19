//! Round-trip property of the canonical printer, as stated in
//! `docs/03-schema-language.md` §9 (OQ-012): with `a = compile(s)` and
//! `b = compile(print(a))`, (1) `a` and `b` are the same schema by name with equal hashes,
//! (2) printing is a fixed point, so the IR round-trips exactly once canonical, and
//! (3) input already in canonical member order keeps its ids.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;

use proptest::prelude::*;
use pemba_schema::ir::{AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, TypeDef};
use pemba_schema::{compile, format};

// ---- generated schemas ----
//
// Choices are drawn as raw numbers and resolved against what exists when the source is
// built, so every generated schema is valid: references resolve, arrows only walk
// wildcard-free relations, and permissions only reference earlier permissions (no
// `E_DIRECT_CYCLE`).

#[derive(Clone, Debug)]
enum RawExpr {
    Leaf(usize),
    Arrow {
        tupleset: usize,
        target_type: usize,
        target_member: usize,
        all: bool,
    },
    Union(Vec<RawExpr>),
    Intersection(Vec<RawExpr>),
    Exclusion(Box<RawExpr>, Box<RawExpr>),
    Paren(Box<RawExpr>),
}

#[derive(Clone, Debug)]
struct RawRelation {
    doc: Option<String>,
    /// `(kind, type, member)`: kind 0 object, 1 wildcard, 2 subject set.
    allowed: Vec<(u8, usize, usize)>,
}

#[derive(Clone, Debug)]
struct RawPermission {
    doc: Option<String>,
    expr: RawExpr,
    /// Spell `->` as `.any()`.
    any_alias: bool,
}

#[derive(Clone, Debug)]
struct RawType {
    prefixed: bool,
    doc: Option<String>,
    relations: Vec<RawRelation>,
    permissions: Vec<RawPermission>,
    /// Consumed in order to interleave members; `true` takes the next permission.
    interleave: Vec<bool>,
}

#[derive(Clone, Debug)]
struct RawSchema {
    types: Vec<RawType>,
    compact: bool,
    plain_comments: bool,
}

fn doc_strategy() -> impl Strategy<Value = Option<String>> {
    // Raw comment bodies; fixed up so the comment ends where intended and is a doc comment.
    proptest::option::of("[a-z *\t/\n]{0,16}".prop_map(|body| {
        let body = body.replace("*/", "* /");
        if body.starts_with('/') {
            format!(" {body}")
        } else {
            body
        }
    }))
}

fn expr_strategy() -> impl Strategy<Value = RawExpr> {
    let leaf = prop_oneof![
        3 => any::<usize>().prop_map(RawExpr::Leaf),
        1 => (any::<usize>(), any::<usize>(), any::<usize>(), any::<bool>()).prop_map(
            |(tupleset, target_type, target_member, all)| RawExpr::Arrow {
                tupleset,
                target_type,
                target_member,
                all,
            }
        ),
    ];
    leaf.prop_recursive(4, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(RawExpr::Union),
            prop::collection::vec(inner.clone(), 2..4).prop_map(RawExpr::Intersection),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| RawExpr::Exclusion(Box::new(a), Box::new(b))),
            inner.prop_map(|e| RawExpr::Paren(Box::new(e))),
        ]
    })
}

fn type_strategy() -> impl Strategy<Value = RawType> {
    let relation = (
        doc_strategy(),
        prop::collection::vec((0u8..3, any::<usize>(), any::<usize>()), 1..4),
    )
        .prop_map(|(doc, allowed)| RawRelation { doc, allowed });
    let permission =
        (doc_strategy(), expr_strategy(), any::<bool>()).prop_map(|(doc, expr, any_alias)| {
            RawPermission {
                doc,
                expr,
                any_alias,
            }
        });
    (
        any::<bool>(),
        doc_strategy(),
        prop::collection::vec(relation, 0..4),
        prop::collection::vec(permission, 0..4),
        prop::collection::vec(any::<bool>(), 0..8),
    )
        .prop_map(
            |(prefixed, doc, relations, permissions, interleave)| RawType {
                prefixed,
                doc,
                // A permission needs a relation to bottom out in.
                permissions: if relations.is_empty() {
                    Vec::new()
                } else {
                    permissions
                },
                relations,
                interleave,
            },
        )
}

fn schema_strategy() -> impl Strategy<Value = RawSchema> {
    (
        prop::collection::vec(type_strategy(), 1..5),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(types, compact, plain_comments)| RawSchema {
            types,
            compact,
            plain_comments,
        })
}

/// A resolved allowed subject: `(type index, is wildcard, text as written)`.
type Allowed = (usize, bool, String);

/// Turns a [`RawSchema`] into valid source.
struct Builder<'a> {
    raw: &'a RawSchema,
    out: String,
}

impl Builder<'_> {
    fn type_name(&self, i: usize) -> String {
        if self.raw.types[i].prefixed {
            format!("app/ty{i}")
        } else {
            format!("ty{i}")
        }
    }

    /// All member names of type `i` (every permission exists when it has relations).
    fn member_names(&self, i: usize) -> Vec<String> {
        let t = &self.raw.types[i];
        (0..t.relations.len())
            .map(|j| format!("rel{j}"))
            .chain((0..t.permissions.len()).map(|j| format!("perm{j}")))
            .collect()
    }

    /// Resolved allowed subjects of `rel`, deduplicated.
    fn allowed(&self, rel: &RawRelation) -> Vec<Allowed> {
        let n = self.raw.types.len();
        let mut seen = Vec::new();
        for &(kind, ty, member) in &rel.allowed {
            let ty = ty % n;
            let name = self.type_name(ty);
            let rels = self.raw.types[ty].relations.len();
            let (wildcard, text) = match kind {
                1 => (true, format!("{name}:*")),
                2 if rels > 0 => (false, format!("{name}#rel{}", member % rels)),
                _ => (false, name),
            };
            if !seen.iter().any(|(_, _, t)| *t == text) {
                seen.push((ty, wildcard, text));
            }
        }
        seen
    }

    fn sep(&self, spaced: &'static str, compact: &'static str) -> &'static str {
        if self.raw.compact { compact } else { spaced }
    }

    fn expr(&self, ty: usize, perm: usize, e: &RawExpr, any_alias: bool) -> String {
        let t = &self.raw.types[ty];
        // Relations, then earlier permissions: never a direct cycle.
        let leaves: Vec<String> = (0..t.relations.len())
            .map(|j| format!("rel{j}"))
            .chain((0..perm).map(|j| format!("perm{j}")))
            .collect();
        let leaf = |i: usize| leaves[i % leaves.len()].clone();
        let wrap = |s: String, compound: bool| if compound { format!("({s})") } else { s };
        match e {
            RawExpr::Leaf(i) => leaf(*i),
            RawExpr::Arrow {
                tupleset,
                target_type,
                target_member,
                all,
            } => {
                let walkable: Vec<(usize, Vec<Allowed>)> = t
                    .relations
                    .iter()
                    .enumerate()
                    .map(|(j, r)| (j, self.allowed(r)))
                    .filter(|(_, a)| !a.iter().any(|(_, wildcard, _)| *wildcard))
                    .collect();
                if walkable.is_empty() {
                    return leaf(*tupleset);
                }
                let (rel, allowed) = &walkable[tupleset % walkable.len()];
                let target = allowed[target_type % allowed.len()].0;
                let names = self.member_names(target);
                if names.is_empty() {
                    return leaf(*tupleset);
                }
                let computed = &names[target_member % names.len()];
                if *all {
                    format!("rel{rel}.all({computed})")
                } else if any_alias {
                    format!("rel{rel}.any({computed})")
                } else {
                    format!("rel{rel}->{computed}")
                }
            }
            RawExpr::Union(xs) | RawExpr::Intersection(xs) => {
                let op = match e {
                    RawExpr::Union(_) => self.sep(" + ", "+"),
                    _ => self.sep(" & ", "&"),
                };
                xs.iter()
                    .map(|x| wrap(self.expr(ty, perm, x, any_alias), is_compound(x)))
                    .collect::<Vec<_>>()
                    .join(op)
            }
            RawExpr::Exclusion(a, b) => format!(
                "{}{}{}",
                wrap(self.expr(ty, perm, a, any_alias), is_compound(a)),
                self.sep(" - ", "-"),
                wrap(self.expr(ty, perm, b, any_alias), is_compound(b)),
            ),
            RawExpr::Paren(x) => format!("({})", self.expr(ty, perm, x, any_alias)),
        }
    }

    fn doc(&mut self, doc: Option<&String>) {
        if let Some(body) = doc {
            let _ = writeln!(self.out, "/**{body}*/");
        }
    }

    /// The source, and whether its members are already in canonical order (relations
    /// before permissions in every definition).
    fn build(mut self) -> (String, bool) {
        let mut canonical_order = true;
        for (i, t) in self.raw.types.iter().enumerate() {
            if self.raw.plain_comments {
                let _ = writeln!(self.out, "// type {i}");
            }
            self.doc(t.doc.as_ref());
            let _ = write!(self.out, "definition {} {{", self.type_name(i));
            let (mut r, mut p) = (0, 0);
            let mut choices = t.interleave.iter();
            while r < t.relations.len() || p < t.permissions.len() {
                let take_perm = p < t.permissions.len()
                    && (r == t.relations.len() || choices.next().copied().unwrap_or(false));
                self.out.push('\n');
                if take_perm {
                    if r < t.relations.len() {
                        canonical_order = false;
                    }
                    let perm = &t.permissions[p];
                    self.doc(perm.doc.as_ref());
                    let body = self.expr(i, p, &perm.expr, perm.any_alias);
                    let eq = self.sep(" = ", "=");
                    let _ = write!(self.out, "permission perm{p}{eq}{body}");
                    p += 1;
                } else {
                    let rel = &t.relations[r];
                    self.doc(rel.doc.as_ref());
                    let allowed: Vec<String> =
                        self.allowed(rel).into_iter().map(|(_, _, s)| s).collect();
                    let colon = self.sep(": ", ":");
                    let bar = self.sep(" | ", "|");
                    let _ = write!(self.out, "relation rel{r}{colon}{}", allowed.join(bar));
                    r += 1;
                }
                if self.raw.plain_comments {
                    self.out.push_str(" // member");
                }
            }
            self.out.push_str("\n}\n");
        }
        (self.out, canonical_order)
    }
}

fn is_compound(e: &RawExpr) -> bool {
    matches!(
        e,
        RawExpr::Union(_) | RawExpr::Intersection(_) | RawExpr::Exclusion(..)
    )
}

// ---- comparison by name ----

/// The schema with every id replaced by the name it stands for and members grouped as the
/// printer orders them, so schemas that differ only in `RelId` numbering compare equal.
fn by_name(s: &CompiledSchema) -> Vec<String> {
    s.types.iter().map(|t| type_by_name(s, t)).collect()
}

fn type_by_name(s: &CompiledSchema, t: &TypeDef) -> String {
    let mut out = format!("{} doc={:?}\n", t.name, t.doc);
    let relations = t
        .members
        .iter()
        .filter(|m| matches!(m, Member::Relation(_)));
    let permissions = t
        .members
        .iter()
        .filter(|m| matches!(m, Member::Permission(_)));
    for m in relations.chain(permissions) {
        match m {
            Member::Relation(r) => {
                let allowed: Vec<String> = r
                    .allowed
                    .iter()
                    .map(|a| match *a {
                        AllowedSubject::Object(ty) => format!("O({})", s.types[ty.index()].name),
                        AllowedSubject::Wildcard(ty) => {
                            format!("W({})", s.types[ty.index()].name)
                        }
                        AllowedSubject::Set(ty, rel) => {
                            let def = &s.types[ty.index()];
                            format!("S({}#{})", def.name, def.members[rel.index()].name())
                        }
                    })
                    .collect();
                let _ = writeln!(out, "  rel {} doc={:?} {allowed:?}", r.name, r.doc);
            }
            Member::Permission(p) => {
                let _ = writeln!(
                    out,
                    "  perm {} doc={:?} {}",
                    p.name,
                    p.doc,
                    expr_by_name(s, t, &p.expr)
                );
            }
        }
    }
    out
}

fn expr_by_name(s: &CompiledSchema, t: &TypeDef, e: &Expr) -> String {
    let name = |rel: pemba_schema::ir::RelId| t.members[rel.index()].name().to_owned();
    match e {
        Expr::Computed(rel) => name(*rel),
        Expr::Arrow {
            tupleset,
            computed,
            mode,
            targets,
        } => {
            let targets: Vec<String> = targets
                .iter()
                .map(|(ty, rel)| {
                    let def = &s.types[ty.index()];
                    format!("{}#{}", def.name, def.members[rel.index()].name())
                })
                .collect();
            let mode = match mode {
                ArrowMode::Any => "any",
                ArrowMode::All => "all",
            };
            format!("{}.{mode}({computed}){targets:?}", name(*tupleset))
        }
        Expr::Union(xs) => format!(
            "U[{}]",
            xs.iter()
                .map(|x| expr_by_name(s, t, x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Intersection(xs) => format!(
            "I[{}]",
            xs.iter()
                .map(|x| expr_by_name(s, t, x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Exclusion { base, sub } => format!(
            "X[{}, {}]",
            expr_by_name(s, t, base),
            expr_by_name(s, t, sub)
        ),
    }
}

// ---- the property ----

fn compile_ok(src: &str) -> CompiledSchema {
    match compile(src) {
        Ok(c) => c.schema,
        Err(e) => panic!("expected to compile:\n{src}\n{e}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn compile_print_round_trips(raw in schema_strategy()) {
        let (src, canonical_order) = Builder { raw: &raw, out: String::new() }.build();
        let ir = compile_ok(&src);
        let printed = ir.to_canonical_string();
        let reparsed = compile_ok(&printed);

        // Same schema, by name, for any input.
        prop_assert_eq!(by_name(&reparsed), by_name(&ir), "printed:\n{}", printed);
        prop_assert_eq!(reparsed.hash, ir.hash);
        prop_assert_eq!(&*reparsed.source, printed.as_str());

        // Printing is a fixed point, so on canonical input the IR round-trips exactly,
        // `source` included.
        let printed_again = reparsed.to_canonical_string();
        prop_assert_eq!(&printed_again, &printed);
        prop_assert_eq!(compile_ok(&printed_again), reparsed.clone());

        // Inputs already in canonical member order keep their ids.
        if canonical_order {
            prop_assert_eq!(&reparsed.types, &ir.types);
        }
    }
}

/// Byte offsets of whitespace outside comments: inserting a comment there keeps the
/// source valid and leaves every existing comment as it was.
fn comment_spots(src: &str) -> Vec<usize> {
    let mut spots = Vec::new();
    let mut i = 0;
    while let Some(c) = src[i..].chars().next() {
        let rest = &src[i..];
        if rest.starts_with("//") {
            i += rest.find('\n').map_or(rest.len(), |n| n + 1);
        } else if rest.starts_with("/*") {
            i += rest.find("*/").map_or(rest.len(), |n| n + 2);
        } else {
            if c.is_whitespace() {
                spots.push(i);
            }
            i += c.len_utf8();
        }
    }
    spots
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// T3.8: comments scattered through any valid schema survive `format`, which is a
    /// fixed point, keeps the schema (hash), and equals the canonical printer when there
    /// are no comments.
    #[test]
    fn format_keeps_every_comment(
        raw in schema_strategy(),
        cuts in prop::collection::vec((any::<prop::sample::Index>(), any::<bool>()), 0..12),
    ) {
        let (src, _) = Builder { raw: &raw, out: String::new() }.build();
        let spots = comment_spots(&src);
        let mut picks: Vec<(usize, bool, usize)> = cuts
            .iter()
            .enumerate()
            .map(|(n, (at, line))| (spots[at.index(spots.len())], *line, n))
            .collect();
        picks.sort_unstable_by(|a, b| b.cmp(a));
        let mut with = src.clone();
        for (at, line, n) in &picks {
            let comment = if *line { format!(" // c{n}\n") } else { format!(" /* c{n} */ ") };
            with.insert_str(*at, &comment);
        }

        let out = format(&with).map_err(|e| TestCaseError::fail(format!("{e}\n{with}")))?;
        prop_assert_eq!(&format(&out).unwrap(), &out, "not a fixed point; input:\n{}", with);
        prop_assert_eq!(compile_ok(&out).hash, compile_ok(&src).hash, "output:\n{}", out);
        for n in 0..cuts.len() {
            let found = out.matches(&format!("// c{n}\n")).count()
                + out.matches(&format!("/* c{n} */")).count();
            prop_assert_eq!(found, 1, "c{} in:\n{}\noutput:\n{}", n, with, out);
        }
        if cuts.is_empty() && !raw.plain_comments {
            prop_assert_eq!(&out, &compile_ok(&src).to_canonical_string());
        }
    }
}

#[test]
fn generator_produces_interleaved_and_canonical_inputs() {
    // Guard against the generator collapsing to one case and hiding half the property.
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut interleaved, mut canonical) = (0, 0);
    for _ in 0..200 {
        let raw = schema_strategy().new_tree(&mut runner).unwrap().current();
        let (_, canonical_order) = Builder {
            raw: &raw,
            out: String::new(),
        }
        .build();
        if canonical_order {
            canonical += 1;
        } else {
            interleaved += 1;
        }
    }
    assert!(
        interleaved > 20 && canonical > 20,
        "{interleaved} / {canonical}"
    );
}
