//! Semantic validation (`docs/03-schema-language.md` §5): every error code has a failing
//! example with its rendered diagnostic snapshotted, all errors are collected and sorted
//! by span, and warnings are reported as specified.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use warden_schema::{ErrorCode, SchemaErrors, compile};

fn errors(src: &str) -> SchemaErrors {
    match compile(src) {
        Ok(_) => panic!("expected {src:?} to be rejected"),
        Err(e) => e,
    }
}

/// Compiles `src`, asserts it fails with exactly `codes` (in span order), and snapshots
/// the rendered report with the source as the description.
fn check(name: &str, src: &str, codes: &[ErrorCode]) {
    let errs = errors(src);
    let got: Vec<_> = errs.errors.iter().map(|e| e.code).collect();
    assert_eq!(got, codes, "{name}:\n{errs}");
    insta::with_settings!({ description => src, omit_expression => true }, {
        insta::assert_snapshot!(name, errs.to_string());
    });
}

/// Compiles `src` successfully and snapshots its warnings.
fn check_warnings(name: &str, src: &str, codes: &[ErrorCode]) {
    let compiled = match compile(src) {
        Ok(c) => c,
        Err(e) => panic!("expected {src:?} to compile:\n{e}"),
    };
    let got: Vec<_> = compiled.warnings.iter().map(|w| w.code).collect();
    assert_eq!(got, codes, "{name}");
    assert!(got.iter().all(|c| c.is_warning()));
    let rendered = compiled
        .warnings
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n\n");
    insta::with_settings!({ description => src, omit_expression => true }, {
        insta::assert_snapshot!(name, rendered);
    });
}

// ---- E_IDENT_FORMAT ----

#[test]
fn ident_format_type_name() {
    check(
        "ident_format_type_name",
        "definition ab {}",
        &[ErrorCode::IdentFormat],
    );
}

#[test]
fn ident_format_type_prefix() {
    check(
        "ident_format_type_prefix",
        "definition docs/document_ {}",
        &[ErrorCode::IdentFormat],
    );
}

#[test]
fn ident_format_member_names() {
    check(
        "ident_format_member_names",
        "definition user {}\ndefinition doc {\n    relation trailing_: user\n    permission vw = trailing_\n}",
        &[ErrorCode::IdentFormat, ErrorCode::IdentFormat],
    );
}

#[test]
fn ident_format_too_long() {
    let long = "a".repeat(65);
    check(
        "ident_format_too_long",
        &format!("definition user {{\n    relation {long}: user\n}}"),
        &[ErrorCode::IdentFormat],
    );
}

// ---- duplicates ----

#[test]
fn duplicate_type() {
    check(
        "duplicate_type",
        "definition user {}\n\ndefinition user {}",
        &[ErrorCode::DuplicateType],
    );
}

#[test]
fn duplicate_member() {
    check(
        "duplicate_member",
        "definition user {}\ndefinition doc {\n    relation viewer: user\n    permission viewer = viewer\n}",
        &[ErrorCode::DuplicateMember],
    );
}

#[test]
fn duplicate_allowed_type() {
    check(
        "duplicate_allowed_type",
        "definition user {}\ndefinition group {\n    relation member: user | group#member | user:* | user | group#member | user:*\n}",
        &[
            ErrorCode::DuplicateAllowedType,
            ErrorCode::DuplicateAllowedType,
            ErrorCode::DuplicateAllowedType,
        ],
    );
}

#[test]
fn distinct_forms_of_one_type_are_not_duplicates() {
    assert!(
        compile("definition user {}\ndefinition doc {\n relation viewer: user | user:*\n}").is_ok()
    );
}

// ---- resolution ----

#[test]
fn unknown_type_with_suggestion() {
    check(
        "unknown_type",
        "definition user {}\ndefinition doc {\n    relation viewer: usr | team\n}",
        &[ErrorCode::UnknownType, ErrorCode::UnknownType],
    );
}

#[test]
fn unknown_subject_relation() {
    check(
        "unknown_subject_relation",
        "definition user {}\ndefinition group {\n    relation member: user\n}\ndefinition doc {\n    relation viewer: group#membr\n}",
        &[ErrorCode::UnknownSubjectRelation],
    );
}

#[test]
fn unknown_member() {
    check(
        "unknown_member",
        "definition user {}\ndefinition doc {\n    relation viewer: user\n    permission view = viewr + parnt->view\n}",
        &[ErrorCode::UnknownMember, ErrorCode::UnknownMember],
    );
}

// ---- arrows ----

#[test]
fn arrow_left_not_relation() {
    check(
        "arrow_left_not_relation",
        "definition user {}\ndefinition doc {\n    relation owner: user\n    permission edit = owner\n    permission view = edit->view\n}",
        &[ErrorCode::ArrowLeftNotRelation],
    );
}

#[test]
fn arrow_over_wildcard() {
    check(
        "arrow_over_wildcard",
        "definition folder {\n    relation parent: folder | folder:*\n    permission view = parent->view\n}",
        &[ErrorCode::ArrowOverWildcard],
    );
}

#[test]
fn arrow_target_missing() {
    check(
        "arrow_target_missing",
        "definition user {}\ndefinition team {}\ndefinition doc {\n    relation owner: user | team\n    permission view = owner->member\n}",
        &[ErrorCode::ArrowTargetMissing],
    );
}

#[test]
fn arrow_target_missing_suggests_member_of_target_type() {
    // `memebr` is a typo for `member` on `team`, one of the types allowed on `owner`.
    check(
        "arrow_target_missing_did_you_mean",
        "definition user {}\ndefinition team {\n    relation member: user\n}\ndefinition doc {\n    relation owner: user | team\n    permission view = owner->memebr\n}",
        &[ErrorCode::ArrowTargetMissing],
    );
}

#[test]
fn arrow_target_missing_single_type_all() {
    // One allowed type: singular wording; `.all` is shown as written; no close match, so
    // the hint says to define it. `parent` is not a candidate: it is on `doc`, not `folder`.
    check(
        "arrow_target_missing_single_type",
        "definition folder {\n    relation viewer: folder\n    permission view = viewer\n}\ndefinition doc {\n    relation parent: folder\n    permission edit = parent.all(admin)\n}",
        &[ErrorCode::ArrowTargetMissing],
    );
}

#[test]
fn arrow_target_on_one_type_is_enough() {
    let src = "definition user {}\ndefinition team {\n relation member: user\n}\ndefinition doc {\n relation owner: user | team\n permission view = owner.all(member)\n}";
    assert!(compile(src).is_ok(), "{:?}", compile(src).err());
}

// ---- E_DIRECT_CYCLE ----

#[test]
fn direct_cycle_spec_example() {
    check(
        "direct_cycle",
        "definition user {}\ndefinition doc {\n    relation x_rel: user\n    permission aaa = bbb\n    permission bbb = aaa + x_rel\n}",
        &[ErrorCode::DirectCycle],
    );
}

#[test]
fn direct_cycle_self_reference_inside_exclusion() {
    check(
        "direct_cycle_self",
        "definition user {}\ndefinition doc {\n    relation banned: user\n    permission view = banned - view\n}",
        &[ErrorCode::DirectCycle],
    );
}

#[test]
fn direct_cycles_are_reported_once_each() {
    let src = "definition user {}\ndefinition doc {\n relation rel: user\n permission aaa = bbb\n permission bbb = ccc & aaa\n permission ccc = aaa\n permission ddd = ddd\n}";
    let errs = errors(src);
    let codes: Vec<_> = errs.errors.iter().map(|e| e.code).collect();
    assert_eq!(
        codes,
        [ErrorCode::DirectCycle, ErrorCode::DirectCycle],
        "{errs}"
    );
}

#[test]
fn recursion_through_arrows_and_relations_is_legal() {
    let src = "definition user {}\ndefinition group {\n relation member: user | group#member\n permission all_members = member\n}\ndefinition folder {\n relation parent: folder\n relation viewer: user\n permission view = viewer + parent->view\n}";
    assert!(compile(src).is_ok(), "{:?}", compile(src).err());
}

// ---- E_RESERVED_NAME ----

#[test]
fn reserved_member_names() {
    check(
        "reserved_name",
        "definition user {}\ndefinition doc {\n    relation nil: user\n    relation any: user\n    permission self = any\n}",
        &[
            ErrorCode::ReservedName,
            ErrorCode::ReservedName,
            ErrorCode::ReservedName,
        ],
    );
}

#[test]
fn every_reserved_word_is_rejected() {
    for word in [
        "definition",
        "relation",
        "permission",
        "self",
        "nil",
        "caveat",
        "use",
        "with",
        "any",
        "all",
    ] {
        let src = format!("definition user {{}}\ndefinition doc {{ relation {word}: user }}");
        let codes: Vec<_> = errors(&src).errors.iter().map(|e| e.code).collect();
        assert_eq!(codes, [ErrorCode::ReservedName], "{word}");
    }
}

// ---- syntax errors surface through compile ----

#[test]
fn parse_errors_are_returned_by_compile() {
    let cases = [
        (
            "definition doc { permission p_ab = a_ab + b_ab & c_ab }",
            ErrorCode::AmbiguousOperators,
        ),
        (
            "definition doc { permission p_ab = a_ab->b_ab->c_ab }",
            ErrorCode::ArrowChain,
        ),
        ("use typechecking", ErrorCode::UnsupportedFeature),
        (
            "definition doc { relation r_ab: }",
            ErrorCode::UnexpectedToken,
        ),
    ];
    for (src, code) in cases {
        let codes: Vec<_> = errors(src).errors.iter().map(|e| e.code).collect();
        assert_eq!(codes, [code], "{src}");
    }
}

// ---- collection and ordering ----

#[test]
fn all_errors_collected_sorted_by_span() {
    let src = "\
definition user {}
definition doc {
    relation viewer: user | grp#member
    relation viewer: user
    permission view = viewer + editor
    permission nil = view
}
definition user {}
definition ab {
    relation parent: doc
    permission view = parent->nope
}
";
    let errs = errors(src);
    let codes: Vec<_> = errs.errors.iter().map(|e| e.code).collect();
    assert_eq!(
        codes,
        [
            ErrorCode::UnknownType,
            ErrorCode::DuplicateMember,
            ErrorCode::UnknownMember,
            ErrorCode::ReservedName,
            ErrorCode::DuplicateType,
            ErrorCode::IdentFormat,
            ErrorCode::ArrowTargetMissing,
        ],
        "{errs}"
    );
    let starts: Vec<_> = errs.errors.iter().map(|e| e.span.start).collect();
    assert!(starts.is_sorted(), "{starts:?}");
    insta::with_settings!({ description => src, omit_expression => true }, {
        insta::assert_snapshot!("all_errors_collected", errs.to_string());
    });
}

// ---- warnings ----

#[test]
fn warning_unused_relation() {
    check_warnings(
        "warning_unused_relation",
        "definition user {}\ndefinition doc {\n    relation owner: user\n    relation legacy_viewer: user\n    permission edit = owner\n}",
        &[ErrorCode::UnusedRelation],
    );
}

#[test]
fn relations_used_only_by_arrows_or_subject_sets_are_used() {
    let src = "definition user {}\ndefinition group {\n relation member: user\n}\ndefinition folder {\n relation viewer: user\n}\ndefinition doc {\n relation folder: folder\n relation viewer: group#member\n permission view = folder->viewer + viewer\n}";
    let compiled = compile(src).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
}

#[test]
fn warning_wildcard_on_write_like_permission() {
    check_warnings(
        "warning_wildcard_write_like",
        "definition user {}\ndefinition doc {\n    relation editor: user | user:*\n    permission can_edit = editor\n    permission view = editor\n}",
        &[ErrorCode::WildcardOnWriteLikePermission],
    );
}

#[test]
fn wildcard_flows_through_subject_sets_and_arrows() {
    let src = "definition user {}\n\
definition group {\n relation member: user:*\n}\n\
definition folder {\n relation reader: group#member\n permission read = reader\n}\n\
definition doc {\n relation folder: folder\n permission manage = folder->read\n}";
    let codes: Vec<_> = compile(src)
        .unwrap()
        .warnings
        .iter()
        .map(|w| w.code)
        .collect();
    assert_eq!(codes, [ErrorCode::WildcardOnWriteLikePermission]);
}

#[test]
fn wildcard_in_excluded_operand_does_not_warn() {
    let src = "definition user {}\ndefinition doc {\n relation owner: user\n relation public: user:*\n permission delete = owner - public\n permission view = public\n}";
    let compiled = compile(src).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
}

#[test]
fn warning_dangling_doc_comments() {
    check_warnings(
        "warning_dangling_doc",
        "definition user {}\ndefinition doc {\n    relation owner: user\n    permission view = owner /** inside */ + owner\n    /** before close */\n}\n/** at end */\n",
        &[
            ErrorCode::DanglingDocComment,
            ErrorCode::DanglingDocComment,
            ErrorCode::DanglingDocComment,
        ],
    );
}

#[test]
fn dangling_doc_warning_is_kept_when_compile_fails() {
    let errs = errors("definition ab {}\n/** orphan */");
    let codes: Vec<_> = errs.warnings.iter().map(|w| w.code).collect();
    assert_eq!(codes, [ErrorCode::DanglingDocComment]);
    assert!(errs.to_string().contains("warning[W_DANGLING_DOC_COMMENT]"));
}

#[test]
fn spec_example_has_no_warnings() {
    let compiled = compile(include_str!("../../../testdata/schemas/spec_example.zed")).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
}

#[test]
fn codes_display_as_spec_strings() {
    let codes = [
        (ErrorCode::IdentFormat, "E_IDENT_FORMAT"),
        (ErrorCode::DuplicateType, "E_DUPLICATE_TYPE"),
        (ErrorCode::DuplicateMember, "E_DUPLICATE_MEMBER"),
        (ErrorCode::UnknownType, "E_UNKNOWN_TYPE"),
        (
            ErrorCode::UnknownSubjectRelation,
            "E_UNKNOWN_SUBJECT_RELATION",
        ),
        (ErrorCode::UnknownMember, "E_UNKNOWN_MEMBER"),
        (ErrorCode::ArrowLeftNotRelation, "E_ARROW_LEFT_NOT_RELATION"),
        (ErrorCode::ArrowOverWildcard, "E_ARROW_OVER_WILDCARD"),
        (ErrorCode::ArrowTargetMissing, "E_ARROW_TARGET_MISSING"),
        (ErrorCode::DirectCycle, "E_DIRECT_CYCLE"),
        (ErrorCode::EmptyRelationTypes, "E_EMPTY_RELATION_TYPES"),
        (ErrorCode::DuplicateAllowedType, "E_DUPLICATE_ALLOWED_TYPE"),
        (ErrorCode::ReservedName, "E_RESERVED_NAME"),
        (
            ErrorCode::WildcardOnWriteLikePermission,
            "W_WILDCARD_ON_WRITE_LIKE_PERMISSION",
        ),
        (ErrorCode::UnusedRelation, "W_UNUSED_RELATION"),
        (ErrorCode::DanglingDocComment, "W_DANGLING_DOC_COMMENT"),
    ];
    for (code, s) in codes {
        assert_eq!(code.as_str(), s);
        assert_eq!(code.is_warning(), s.starts_with("W_"), "{s}");
    }
}
