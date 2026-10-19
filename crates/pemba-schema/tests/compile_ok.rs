//! Compiled IR (`docs/03-schema-language.md` §6) for the §1 example: interning, expression
//! shape, per-arrow target maps and the derived indexes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use warden_schema::compile;
use warden_schema::ir::{
    AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, RelId, TypeDef, TypeId,
};

const SPEC_EXAMPLE: &str = include_str!("../../../testdata/schemas/spec_example.zed");

fn schema(src: &str) -> CompiledSchema {
    match compile(src) {
        Ok(c) => c.schema,
        Err(e) => panic!("expected {src:?} to compile:\n{e}"),
    }
}

fn ty<'a>(s: &'a CompiledSchema, name: &str) -> &'a TypeDef {
    let id = s.type_id(name).unwrap_or_else(|| panic!("no type {name}"));
    s.type_def(id).unwrap()
}

/// `(TypeId, RelId)` for `type#member`.
fn id(s: &CompiledSchema, t: &str, m: &str) -> (TypeId, RelId) {
    let def = ty(s, t);
    let rel = def
        .member_id(m)
        .unwrap_or_else(|| panic!("no member {t}#{m}"));
    (def.id, rel)
}

fn rel(s: &CompiledSchema, t: &str, m: &str) -> RelId {
    id(s, t, m).1
}

fn perm_expr<'a>(s: &'a CompiledSchema, t: &str, m: &str) -> &'a Expr {
    let (tid, rid) = id(s, t, m);
    match s.type_def(tid).unwrap().member(rid) {
        Some(Member::Permission(p)) => &p.expr,
        other => panic!("{t}#{m} is not a permission: {other:?}"),
    }
}

fn computed(s: &CompiledSchema, t: &str, m: &str) -> Expr {
    Expr::Computed(rel(s, t, m))
}

fn arrow(
    s: &CompiledSchema,
    t: &str,
    tupleset: &str,
    target: &str,
    mode: ArrowMode,
    targets: &[(&str, &str)],
) -> Expr {
    Expr::Arrow {
        tupleset: rel(s, t, tupleset),
        computed: target.into(),
        mode,
        targets: targets
            .iter()
            .map(|&(t, m)| id(s, t, m))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn types_and_members_are_interned_in_source_order() {
    let s = schema(SPEC_EXAMPLE);
    let names: Vec<_> = s.types.iter().map(|t| &*t.name).collect();
    assert_eq!(names, ["user", "group", "folder", "document"]);
    for (i, t) in s.types.iter().enumerate() {
        assert_eq!(t.id.index(), i);
        assert_eq!(s.type_id(&t.name), Some(t.id));
    }
    assert_eq!(s.type_id("nope"), None);

    let doc = ty(&s, "document");
    let members: Vec<_> = doc.members.iter().map(Member::name).collect();
    assert_eq!(
        members,
        [
            "folder",
            "owner",
            "viewer",
            "banned",
            "edit",
            "view",
            "view_if_in_all_groups"
        ]
    );
    for (i, m) in doc.members.iter().enumerate() {
        assert_eq!(doc.member_id(m.name()).map(RelId::index), Some(i));
    }
    assert_eq!(doc.member_id("nope"), None);
    assert_eq!(&*s.source, SPEC_EXAMPLE);
}

#[test]
fn docs_are_carried_into_the_ir() {
    let s = schema(SPEC_EXAMPLE);
    assert_eq!(ty(&s, "user").doc.as_deref(), Some("a person"));
    assert_eq!(ty(&s, "group").doc, None);
}

#[test]
fn allowed_subjects() {
    let s = schema(SPEC_EXAMPLE);
    let user = s.type_id("user").unwrap();
    let group = s.type_id("group").unwrap();
    let (doc, viewer) = id(&s, "document", "viewer");
    let Some(Member::Relation(r)) = s.type_def(doc).unwrap().member(viewer) else {
        panic!("viewer is a relation")
    };
    assert_eq!(
        r.allowed,
        [
            AllowedSubject::Object(user),
            AllowedSubject::Wildcard(user),
            AllowedSubject::Set(group, rel(&s, "group", "member")),
        ]
    );
}

#[test]
fn expressions_with_arrow_target_maps() {
    let s = schema(SPEC_EXAMPLE);
    assert_eq!(
        *perm_expr(&s, "folder", "view"),
        Expr::Union(vec![
            computed(&s, "folder", "viewer"),
            computed(&s, "folder", "edit"),
            arrow(
                &s,
                "folder",
                "parent",
                "view",
                ArrowMode::Any,
                &[("folder", "view")]
            ),
        ])
    );
    assert_eq!(
        *perm_expr(&s, "document", "view"),
        Expr::Exclusion {
            base: Box::new(Expr::Union(vec![
                computed(&s, "document", "viewer"),
                computed(&s, "document", "edit"),
                arrow(
                    &s,
                    "document",
                    "folder",
                    "view",
                    ArrowMode::Any,
                    &[("folder", "view")]
                ),
            ])),
            sub: Box::new(computed(&s, "document", "banned")),
        }
    );
    assert_eq!(
        *perm_expr(&s, "document", "view_if_in_all_groups"),
        arrow(
            &s,
            "document",
            "folder",
            "view",
            ArrowMode::All,
            &[("folder", "view")]
        )
    );
}

#[test]
fn arrow_targets_only_include_types_that_have_the_member() {
    let s = schema(
        "definition user {}\ndefinition team {\n relation member: user\n}\ndefinition doc {\n relation owner: user | team#member\n permission view = owner->member\n}",
    );
    assert_eq!(
        *perm_expr(&s, "doc", "view"),
        arrow(
            &s,
            "doc",
            "owner",
            "member",
            ArrowMode::Any,
            &[("team", "member")]
        )
    );
}

#[test]
fn unions_and_intersections_flatten_but_exclusions_do_not() {
    let s = schema(
        "definition user {}\ndefinition doc {\n relation aaa: user\n relation bbb: user\n relation ccc: user\n relation ddd: user\n\
         permission p_union = (aaa + bbb) + (ccc + ddd)\n\
         permission p_inter = aaa & (bbb & (ccc & ddd))\n\
         permission p_mixed = aaa + (bbb & ccc)\n\
         permission p_excl = aaa - (bbb - ccc)\n}",
    );
    let c = |m| computed(&s, "doc", m);
    assert_eq!(
        *perm_expr(&s, "doc", "p_union"),
        Expr::Union(vec![c("aaa"), c("bbb"), c("ccc"), c("ddd")])
    );
    assert_eq!(
        *perm_expr(&s, "doc", "p_inter"),
        Expr::Intersection(vec![c("aaa"), c("bbb"), c("ccc"), c("ddd")])
    );
    assert_eq!(
        *perm_expr(&s, "doc", "p_mixed"),
        Expr::Union(vec![c("aaa"), Expr::Intersection(vec![c("bbb"), c("ccc")])])
    );
    assert_eq!(
        *perm_expr(&s, "doc", "p_excl"),
        Expr::Exclusion {
            base: Box::new(c("aaa")),
            sub: Box::new(Expr::Exclusion {
                base: Box::new(c("bbb")),
                sub: Box::new(c("ccc")),
            }),
        }
    );
}

#[test]
fn index_used_by() {
    let s = schema(SPEC_EXAMPLE);
    let used_by = |t, m| {
        let (tid, rid) = id(&s, t, m);
        s.used_by(tid, rid).to_vec()
    };
    assert_eq!(used_by("folder", "owner"), [id(&s, "folder", "edit")]);
    assert_eq!(used_by("folder", "viewer"), [id(&s, "folder", "view")]);
    assert_eq!(used_by("folder", "edit"), [id(&s, "folder", "view")]);
    // An arrow's tupleset is walked, not computed: that use is in `arrows_into`.
    assert_eq!(used_by("folder", "parent"), []);
    assert_eq!(used_by("folder", "view"), []);
    assert_eq!(used_by("document", "owner"), [id(&s, "document", "edit")]);
    assert_eq!(used_by("document", "viewer"), [id(&s, "document", "view")]);
    assert_eq!(used_by("document", "edit"), [id(&s, "document", "view")]);
    assert_eq!(used_by("document", "banned"), [id(&s, "document", "view")]);
    assert_eq!(used_by("document", "folder"), []);
    assert_eq!(used_by("group", "member"), []);
}

#[test]
fn index_arrows_into() {
    let s = schema(SPEC_EXAMPLE);
    let arrows_into = |t, m| {
        let (tid, rid) = id(&s, t, m);
        s.arrows_into(tid, rid).to_vec()
    };
    let folder = s.type_id("folder").unwrap();
    let document = s.type_id("document").unwrap();
    assert_eq!(
        arrows_into("folder", "edit"),
        [
            (
                folder,
                rel(&s, "folder", "parent"),
                rel(&s, "folder", "edit")
            ),
            (
                document,
                rel(&s, "document", "folder"),
                rel(&s, "document", "edit")
            ),
        ]
    );
    assert_eq!(
        arrows_into("folder", "view"),
        [
            (
                folder,
                rel(&s, "folder", "parent"),
                rel(&s, "folder", "view")
            ),
            (
                document,
                rel(&s, "document", "folder"),
                rel(&s, "document", "view")
            ),
            (
                document,
                rel(&s, "document", "folder"),
                rel(&s, "document", "view_if_in_all_groups")
            ),
        ]
    );
    assert_eq!(arrows_into("folder", "owner"), []);
    assert_eq!(arrows_into("document", "view"), []);
}

#[test]
fn index_subject_set_uses() {
    let s = schema(SPEC_EXAMPLE);
    let (group, member) = id(&s, "group", "member");
    assert_eq!(
        s.subject_set_uses(group, member),
        [
            id(&s, "group", "member"),
            id(&s, "folder", "viewer"),
            id(&s, "document", "viewer"),
        ]
    );
    let (folder, viewer) = id(&s, "folder", "viewer");
    assert_eq!(s.subject_set_uses(folder, viewer), []);
}

#[test]
fn indexes_are_deduplicated() {
    let s = schema(
        "definition user {}\ndefinition folder {\n relation viewer: user\n}\ndefinition doc {\n relation owner: user\n relation parent: folder\n permission view = owner + (owner & parent->viewer) + parent.all(viewer)\n}",
    );
    let (doc, owner) = id(&s, "doc", "owner");
    assert_eq!(s.used_by(doc, owner), [id(&s, "doc", "view")]);
    let (folder, viewer) = id(&s, "folder", "viewer");
    assert_eq!(
        s.arrows_into(folder, viewer),
        [(doc, rel(&s, "doc", "parent"), rel(&s, "doc", "view"))]
    );
}

#[test]
fn prefixed_types_resolve() {
    let s = schema(
        "definition iam/user {}\ndefinition docs/document {\n relation viewer: iam/user\n permission view = viewer\n}",
    );
    let user = s.type_id("iam/user").unwrap();
    let (doc, viewer) = id(&s, "docs/document", "viewer");
    let Some(Member::Relation(r)) = s.type_def(doc).unwrap().member(viewer) else {
        panic!()
    };
    assert_eq!(r.allowed, [AllowedSubject::Object(user)]);
}
