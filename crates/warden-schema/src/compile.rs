//! Semantic validation and compilation to IR (`docs/03-schema-language.md` §5–6).
//!
//! Every check runs over the whole schema and errors are collected rather than returned
//! at the first. Resolution failures produce placeholder IR so checking can carry on; the
//! IR is only returned when there are no errors, so placeholders never escape.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::ast;
use crate::diagnostic::{ErrorCode, SchemaError, SchemaErrors, line_of, sort_by_span};
use crate::ident::{IdentError, IdentErrorKind, validate_identifier, validate_type_name};
use crate::ir::{
    AllowedSubject, CompiledSchema, Expr, Member, PermissionDef, RelId, RelationDef, TypeDef,
    TypeId,
};
use crate::parser::parse;
use crate::span::Span;

/// Words that may not name a relation or permission (§5 `E_RESERVED_NAME`).
const RESERVED: &[&str] = &[
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
];

/// Permission-name fragments that suggest write access (§5
/// `W_WILDCARD_ON_WRITE_LIKE_PERMISSION`), matched anywhere in the name.
const WRITE_LIKE: &[&str] = &["edit", "write", "delete", "admin", "manage", "owner"];

/// A successfully compiled schema and the warnings found while compiling it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Compiled {
    /// The compiled schema.
    pub schema: CompiledSchema,
    /// Warnings (`W_...`), sorted by span. Never fatal.
    pub warnings: Vec<SchemaError>,
}

/// Parses, validates and compiles schema source text.
///
/// # Errors
///
/// A syntax error is returned on its own (parsing stops at the first one). Otherwise every
/// semantic error in `docs/03-schema-language.md` §5 is collected and returned sorted by
/// span, together with any warnings found so far.
pub fn compile(source: &str) -> Result<Compiled, SchemaErrors> {
    let schema = parse(source).map_err(|e| SchemaErrors::new(vec![e], Vec::new()))?;
    compile_ast(&schema, source)
}

/// Validates and compiles an already parsed schema. `source` is the text `ast` was parsed
/// from; diagnostics quote it.
pub(crate) fn compile_ast(ast: &ast::Schema, source: &str) -> Result<Compiled, SchemaErrors> {
    let mut errors = Vec::new();
    let mut warnings: Vec<SchemaError> = ast
        .dangling_docs
        .iter()
        .map(|doc| dangling_doc_warning(source, doc))
        .collect();

    let tables = Tables::build(ast, source, &mut errors);
    let types: Vec<TypeDef> = ast
        .definitions
        .iter()
        .enumerate()
        .map(|(i, def)| compile_definition(&tables, i, def, &mut errors))
        .collect();
    for scope in &tables.scopes {
        check_direct_cycles(&tables, scope, &mut errors);
    }
    if !errors.is_empty() {
        return Err(SchemaErrors::new(errors, warnings));
    }

    // No errors means no duplicate types or members, so `TypeId(i)` / `RelId(j)` are the
    // AST positions and the warnings below can map IR ids back to source spans.
    let schema = CompiledSchema::new(types, Arc::from(source));
    warnings.extend(unused_relations(&schema, ast, source));
    warnings.extend(wildcards_on_write_like(&schema, ast, source));
    sort_by_span(&mut warnings);
    Ok(Compiled { schema, warnings })
}

// ---- symbol tables ----

struct MemberInfo {
    rel: RelId,
    /// Position in the definition's AST members.
    index: usize,
}

/// One definition's members, first occurrence of each name winning.
struct Scope<'a> {
    def: &'a ast::Definition,
    members: HashMap<&'a str, MemberInfo>,
}

impl Scope<'_> {
    fn member_names(&self) -> impl Iterator<Item = &str> {
        self.def
            .members
            .iter()
            .map(|m| member_ident(m).name.as_str())
    }
}

struct Tables<'a> {
    src: &'a str,
    /// One scope per AST definition, duplicates included, so their bodies are checked too.
    scopes: Vec<Scope<'a>>,
    type_ids: HashMap<&'a str, TypeId>,
    /// `TypeId` → AST definition index.
    canonical: Vec<usize>,
}

impl<'a> Tables<'a> {
    fn build(ast: &'a ast::Schema, src: &'a str, errors: &mut Vec<SchemaError>) -> Self {
        let mut type_ids: HashMap<&str, TypeId> = HashMap::new();
        let mut canonical = Vec::new();
        let mut scopes = Vec::new();
        for (i, def) in ast.definitions.iter().enumerate() {
            let name = &def.name;
            if let Err(e) = validate_type_name(&name.name) {
                errors.push(ident_error(src, "type name", &name.name, name.span, e));
            }
            match type_ids.entry(name.name.as_str()) {
                Entry::Occupied(first) => {
                    let first_def = canonical
                        .get(first.get().index())
                        .and_then(|&d| ast.definitions.get(d));
                    errors.push(SchemaError::new(
                        ErrorCode::DuplicateType,
                        format!("type `{}` is defined more than once", name.name),
                        name.span,
                        first_def.map(|d: &ast::Definition| {
                            format!(
                                "`{}` was first defined on line {}; merge the two definitions",
                                name.name,
                                line_of(src, d.name.span.start)
                            )
                        }),
                        src,
                    ));
                }
                Entry::Vacant(slot) => {
                    slot.insert(TypeId(canonical.len()));
                    canonical.push(i);
                }
            }
            scopes.push(Self::scope(def, src, errors));
        }
        Self {
            src,
            scopes,
            type_ids,
            canonical,
        }
    }

    fn scope(def: &'a ast::Definition, src: &str, errors: &mut Vec<SchemaError>) -> Scope<'a> {
        let mut members = HashMap::new();
        for (index, member) in def.members.iter().enumerate() {
            let (ident, what) = match member {
                ast::Member::Relation(r) => (&r.name, "relation"),
                ast::Member::Permission(p) => (&p.name, "permission"),
            };
            if RESERVED.contains(&ident.name.as_str()) {
                errors.push(SchemaError::new(
                    ErrorCode::ReservedName,
                    format!(
                        "`{}` is a reserved word and cannot name a {what}",
                        ident.name
                    ),
                    ident.span,
                    Some(format!(
                        "reserved words are {}",
                        RESERVED
                            .iter()
                            .map(|w| format!("`{w}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    src,
                ));
            } else if let Err(e) = validate_identifier(&ident.name) {
                errors.push(ident_error(
                    src,
                    &format!("{what} name"),
                    &ident.name,
                    ident.span,
                    e,
                ));
            }
            let rel = RelId(members.len());
            match members.entry(ident.name.as_str()) {
                Entry::Occupied(first) => {
                    let first: &MemberInfo = first.get();
                    let first_line = def
                        .members
                        .get(first.index)
                        .map(|m| line_of(src, member_ident(m).span.start));
                    errors.push(SchemaError::new(
                        ErrorCode::DuplicateMember,
                        format!(
                            "`{}` is defined more than once on `{}`",
                            ident.name, def.name.name
                        ),
                        ident.span,
                        first_line.map(|line| {
                            format!("`{}` was first defined on line {line}", ident.name)
                        }),
                        src,
                    ));
                }
                Entry::Vacant(slot) => {
                    slot.insert(MemberInfo { rel, index });
                }
            }
        }
        Scope { def, members }
    }

    fn scope_of(&self, ty: TypeId) -> Option<&Scope<'a>> {
        self.canonical
            .get(ty.index())
            .and_then(|&i| self.scopes.get(i))
    }

    fn member(&self, ty: TypeId, name: &str) -> Option<&MemberInfo> {
        self.scope_of(ty).and_then(|s| s.members.get(name))
    }

    fn type_name(&self, ty: TypeId) -> &str {
        self.scope_of(ty).map_or("", |s| s.def.name.name.as_str())
    }

    fn resolve_type(&self, name: &ast::TypeName, errors: &mut Vec<SchemaError>) -> Option<TypeId> {
        if let Some(&id) = self.type_ids.get(name.name.as_str()) {
            return Some(id);
        }
        let candidates = self
            .canonical
            .iter()
            .filter_map(|&i| self.scopes.get(i).map(|s| s.def.name.name.as_str()));
        let help = suggest(&name.name, candidates).map_or_else(
            || format!("define it with `definition {} {{}}`", name.name),
            |s| format!("did you mean `{s}`?"),
        );
        errors.push(self.error(
            ErrorCode::UnknownType,
            format!("unknown type `{}`", name.name),
            name.span,
            Some(help),
        ));
        None
    }

    fn error(
        &self,
        code: ErrorCode,
        message: String,
        span: Span,
        help: Option<String>,
    ) -> SchemaError {
        SchemaError::new(code, message, span, help, self.src)
    }
}

fn member_ident(member: &ast::Member) -> &ast::Ident {
    match member {
        ast::Member::Relation(r) => &r.name,
        ast::Member::Permission(p) => &p.name,
    }
}

fn ident_error(src: &str, what: &str, name: &str, span: Span, e: IdentError) -> SchemaError {
    let at = span.start + e.offset;
    let span = match e.kind {
        IdentErrorKind::InvalidChar(c)
        | IdentErrorKind::InvalidStart(c)
        | IdentErrorKind::InvalidEnd(c) => Span::new(at, (at + c.len_utf8()).min(span.end)),
        IdentErrorKind::TooLong { .. } => Span::new(at.min(span.end), span.end),
        _ => span,
    };
    let mut help = "identifiers are 3 to 64 characters of `a-z`, `0-9` and `_`, start with a \
                    letter or `_`, and end with a letter or digit"
        .to_owned();
    if what == "type name" {
        help.push_str("; each `/`-separated prefix follows the same rule");
    }
    SchemaError::new(
        ErrorCode::IdentFormat,
        format!("{what} `{name}` {}", e.kind),
        span,
        Some(help),
        src,
    )
}

/// The closest candidate within a small edit distance, for "did you mean" hints.
/// Candidates are taken in order, so ties go to the one written first.
fn suggest<'c>(name: &str, candidates: impl IntoIterator<Item = &'c str>) -> Option<&'c str> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|&c| c != name)
        .map(|c| (edit_distance(name, c), c))
        .filter(|&(d, _)| d <= limit)
        .min_by_key(|&(d, _)| d)
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row.first().copied().unwrap_or(i);
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
        for (j, &cb) in b.iter().enumerate() {
            let above = row.get(j + 1).copied().unwrap_or(usize::MAX);
            let left = row.get(j).copied().unwrap_or(usize::MAX);
            let substitute = diagonal + usize::from(ca != cb);
            diagonal = above;
            if let Some(cell) = row.get_mut(j + 1) {
                *cell = substitute.min(above + 1).min(left + 1);
            }
        }
    }
    row.last().copied().unwrap_or(0)
}

// ---- definitions and relations ----

fn compile_definition(
    tables: &Tables<'_>,
    index: usize,
    def: &ast::Definition,
    errors: &mut Vec<SchemaError>,
) -> TypeDef {
    let members = def
        .members
        .iter()
        .map(|m| match m {
            ast::Member::Relation(r) => Member::Relation(compile_relation(tables, r, errors)),
            ast::Member::Permission(p) => Member::Permission(PermissionDef {
                name: Arc::from(p.name.name.as_str()),
                doc: p.doc.as_ref().map(|d| d.text.clone()),
                expr: tables
                    .scopes
                    .get(index)
                    .map_or_else(placeholder, |scope| resolve(tables, scope, &p.expr, errors)),
            }),
        })
        .collect();
    TypeDef::new(
        TypeId(index),
        Arc::from(def.name.name.as_str()),
        def.doc.as_ref().map(|d| d.text.clone()),
        members,
    )
}

fn compile_relation(
    tables: &Tables<'_>,
    rel: &ast::Relation,
    errors: &mut Vec<SchemaError>,
) -> RelationDef {
    if rel.allowed.is_empty() {
        errors.push(tables.error(
            ErrorCode::EmptyRelationTypes,
            format!("relation `{}` allows no subject types", rel.name.name),
            rel.name.span,
            Some(format!(
                "list at least one type, e.g. `relation {}: user`",
                rel.name.name
            )),
        ));
    }
    let mut seen = HashSet::new();
    let allowed = rel
        .allowed
        .iter()
        .filter_map(|a| {
            let written = match &a.kind {
                ast::AllowedKind::Object => a.type_name.name.clone(),
                ast::AllowedKind::Wildcard => format!("{}:*", a.type_name.name),
                ast::AllowedKind::SubjectSet(r) => format!("{}#{}", a.type_name.name, r.name),
            };
            if !seen.insert(written.clone()) {
                errors.push(tables.error(
                    ErrorCode::DuplicateAllowedType,
                    format!(
                        "`{written}` is listed more than once on relation `{}`",
                        rel.name.name
                    ),
                    a.span,
                    Some("remove the duplicate".to_owned()),
                ));
                return None;
            }
            let ty = tables.resolve_type(&a.type_name, errors)?;
            Some(match &a.kind {
                ast::AllowedKind::Object => AllowedSubject::Object(ty),
                ast::AllowedKind::Wildcard => AllowedSubject::Wildcard(ty),
                ast::AllowedKind::SubjectSet(r) => {
                    let Some(info) = tables.member(ty, &r.name) else {
                        let candidates = tables
                            .scope_of(ty)
                            .into_iter()
                            .flat_map(Scope::member_names);
                        errors.push(tables.error(
                            ErrorCode::UnknownSubjectRelation,
                            format!(
                                "`{}` has no relation or permission named `{}`",
                                a.type_name.name, r.name
                            ),
                            r.span,
                            suggest(&r.name, candidates).map(|s| format!("did you mean `{s}`?")),
                        ));
                        return None;
                    };
                    AllowedSubject::Set(ty, info.rel)
                }
            })
        })
        .collect();
    RelationDef {
        name: Arc::from(rel.name.name.as_str()),
        doc: rel.doc.as_ref().map(|d| d.text.clone()),
        allowed,
    }
}

// ---- expressions ----

/// Stands in for an expression that failed to resolve; only ever seen alongside an error.
fn placeholder() -> Expr {
    Expr::Union(Vec::new())
}

fn resolve(
    tables: &Tables<'_>,
    scope: &Scope<'_>,
    expr: &ast::Expr,
    errors: &mut Vec<SchemaError>,
) -> Expr {
    match &expr.kind {
        ast::ExprKind::Computed(name) => {
            if let Some(info) = scope.members.get(name.name.as_str()) {
                Expr::Computed(info.rel)
            } else {
                errors.push(unknown_member(tables, scope, name));
                placeholder()
            }
        }
        ast::ExprKind::Arrow {
            tupleset,
            computed,
            mode,
        } => resolve_arrow(tables, scope, tupleset, computed, *mode, errors),
        ast::ExprKind::Union(xs) => {
            let mut out = Vec::new();
            for x in xs {
                match resolve(tables, scope, x, errors) {
                    Expr::Union(inner) => out.extend(inner),
                    other => out.push(other),
                }
            }
            Expr::Union(out)
        }
        ast::ExprKind::Intersection(xs) => {
            let mut out = Vec::new();
            for x in xs {
                match resolve(tables, scope, x, errors) {
                    Expr::Intersection(inner) => out.extend(inner),
                    other => out.push(other),
                }
            }
            Expr::Intersection(out)
        }
        ast::ExprKind::Exclusion { base, sub } => Expr::Exclusion {
            base: Box::new(resolve(tables, scope, base, errors)),
            sub: Box::new(resolve(tables, scope, sub, errors)),
        },
    }
}

fn unknown_member(tables: &Tables<'_>, scope: &Scope<'_>, name: &ast::Ident) -> SchemaError {
    let help = suggest(&name.name, scope.member_names()).map_or_else(
        || {
            format!(
                "define `{}` as a relation or permission on `{}`",
                name.name, scope.def.name.name
            )
        },
        |s| format!("did you mean `{s}`?"),
    );
    tables.error(
        ErrorCode::UnknownMember,
        format!(
            "`{}` is not a relation or permission on `{}`",
            name.name, scope.def.name.name
        ),
        name.span,
        Some(help),
    )
}

fn resolve_arrow(
    tables: &Tables<'_>,
    scope: &Scope<'_>,
    tupleset: &ast::Ident,
    computed: &ast::Ident,
    mode: ast::ArrowMode,
    errors: &mut Vec<SchemaError>,
) -> Expr {
    let Some(info) = scope.members.get(tupleset.name.as_str()) else {
        errors.push(unknown_member(tables, scope, tupleset));
        return placeholder();
    };
    let Some(ast::Member::Relation(rel)) = scope.def.members.get(info.index) else {
        errors.push(
            tables.error(
                ErrorCode::ArrowLeftNotRelation,
                format!(
                    "the left side of an arrow must be a relation, but `{}` is a permission",
                    tupleset.name
                ),
                tupleset.span,
                Some(
                    "an arrow follows stored relationships to related objects; start it from the \
                 relation that links to them, e.g. `parent->view`"
                        .to_owned(),
                ),
            ),
        );
        return placeholder();
    };
    if let Some(wildcard) = rel
        .allowed
        .iter()
        .find(|a| a.kind == ast::AllowedKind::Wildcard)
    {
        errors.push(tables.error(
            ErrorCode::ArrowOverWildcard,
            format!(
                "arrow over `{}`, which allows the wildcard `{}`",
                tupleset.name,
                tables.src.get(wildcard.span.start..wildcard.span.end).unwrap_or_default()
            ),
            tupleset.span,
            Some(format!(
                "an arrow cannot walk a wildcard; move `{0}:*` to a separate relation and \
                 keep `{1}` for concrete `{0}` objects",
                wildcard.type_name.name, tupleset.name
            )),
        ));
        return placeholder();
    }
    let mut targets = BTreeMap::new();
    let mut related: Vec<TypeId> = Vec::new();
    for allowed in &rel.allowed {
        let Some(&ty) = tables.type_ids.get(allowed.type_name.name.as_str()) else {
            continue; // already reported as E_UNKNOWN_TYPE on the relation
        };
        if !related.contains(&ty) {
            related.push(ty);
        }
        if let Some(target) = tables.member(ty, &computed.name) {
            targets.insert(ty, target.rel);
        }
    }
    if targets.is_empty() && !related.is_empty() {
        let names = related
            .iter()
            .map(|&t| format!("`{}`", tables.type_name(t)))
            .collect::<Vec<_>>()
            .join(", ");
        errors.push(tables.error(
            ErrorCode::ArrowTargetMissing,
            format!(
                "no type allowed on `{}` ({names}) has a relation or permission named `{}`",
                tupleset.name, computed.name
            ),
            computed.span,
            Some(format!(
                "define `{}` on at least one of {names}",
                computed.name
            )),
        ));
    }
    Expr::Arrow {
        tupleset: info.rel,
        computed: Arc::from(computed.name.as_str()),
        mode,
        targets,
    }
}

// ---- E_DIRECT_CYCLE ----

/// Permissions that reach themselves through `Computed` references to other permissions
/// on the same type (§5). Relations and arrows break the chain: recursion through them is
/// data-dependent and legal. Reports one error per strongly connected component.
fn check_direct_cycles(tables: &Tables<'_>, scope: &Scope<'_>, errors: &mut Vec<SchemaError>) {
    // Nodes: the first occurrence of each permission name, in source order.
    let perms: Vec<&ast::Permission> = scope
        .def
        .members
        .iter()
        .enumerate()
        .filter_map(|(i, m)| match m {
            ast::Member::Permission(p)
                if scope.members.get(p.name.name.as_str()).map(|m| m.index) == Some(i) =>
            {
                Some(p)
            }
            _ => None,
        })
        .collect();
    let node_of: HashMap<&str, usize> = perms
        .iter()
        .enumerate()
        .map(|(n, p)| (p.name.name.as_str(), n))
        .collect();
    let edges: Vec<Vec<usize>> = perms
        .iter()
        .map(|p| {
            let mut refs = Vec::new();
            computed_refs(&p.expr, &mut refs);
            let mut out: Vec<usize> = refs
                .iter()
                .filter_map(|r| node_of.get(r.name.as_str()).copied())
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect();
    let reach: Vec<Vec<bool>> = (0..perms.len())
        .map(|start| reachable(&edges, start))
        .collect();
    let reaches = |a: usize, b: usize| reach.get(a).and_then(|r| r.get(b)).copied() == Some(true);

    let mut reported = vec![false; perms.len()];
    for (n, perm) in perms.iter().enumerate() {
        if !reaches(n, n) || reported.get(n).copied() == Some(true) {
            continue;
        }
        let component: Vec<usize> = (0..perms.len())
            .filter(|&m| reaches(n, m) && reaches(m, n))
            .collect();
        for &m in &component {
            if let Some(r) = reported.get_mut(m) {
                *r = true;
            }
        }
        let path = shortest_cycle(&edges, n)
            .iter()
            .filter_map(|&m| perms.get(m).map(|p| format!("`{}`", p.name.name)))
            .collect::<Vec<_>>()
            .join(" → ");
        errors.push(
            tables.error(
                ErrorCode::DirectCycle,
                format!("permission `{}` depends on itself: {path}", perm.name.name),
                perm.name.span,
                Some(
                    "recursion is only allowed through a relation or an arrow (e.g. \
                 `parent->view`); rewrite one of these permissions so it does not refer \
                 back to the others"
                        .to_owned(),
                ),
            ),
        );
    }
}

/// `Computed` references anywhere in `expr`; arrows are skipped on purpose.
fn computed_refs<'e>(expr: &'e ast::Expr, out: &mut Vec<&'e ast::Ident>) {
    match &expr.kind {
        ast::ExprKind::Computed(name) => out.push(name),
        ast::ExprKind::Arrow { .. } => {}
        ast::ExprKind::Union(xs) | ast::ExprKind::Intersection(xs) => {
            for x in xs {
                computed_refs(x, out);
            }
        }
        ast::ExprKind::Exclusion { base, sub } => {
            computed_refs(base, out);
            computed_refs(sub, out);
        }
    }
}

/// Nodes reachable from `start` by one or more edges.
fn reachable(edges: &[Vec<usize>], start: usize) -> Vec<bool> {
    let mut seen = vec![false; edges.len()];
    let mut queue: VecDeque<usize> = edges.get(start).into_iter().flatten().copied().collect();
    while let Some(n) = queue.pop_front() {
        if let Some(s) = seen.get_mut(n)
            && !*s
        {
            *s = true;
            queue.extend(edges.get(n).into_iter().flatten().copied());
        }
    }
    seen
}

/// The shortest path `start → … → start`, both ends included.
fn shortest_cycle(edges: &[Vec<usize>], start: usize) -> Vec<usize> {
    let mut parent: Vec<Option<usize>> = vec![None; edges.len()];
    let mut queue = VecDeque::from([start]);
    while let Some(n) = queue.pop_front() {
        for &next in edges.get(n).into_iter().flatten() {
            if next == start {
                // Every BFS-tree path leads back to `start`, which has no parent, so this
                // walk ends there: [start, n, parent(n), …, start], reversed below.
                let mut path = vec![start, n];
                let mut at = n;
                while let Some(p) = parent.get(at).copied().flatten() {
                    path.push(p);
                    at = p;
                }
                path.reverse();
                return path;
            }
            if next != start
                && let Some(slot) = parent.get_mut(next)
                && slot.is_none()
            {
                *slot = Some(n);
                queue.push_back(next);
            }
        }
    }
    vec![start, start]
}

// ---- warnings ----

fn dangling_doc_warning(src: &str, doc: &ast::DocComment) -> SchemaError {
    SchemaError::new(
        ErrorCode::DanglingDocComment,
        "doc comment is not attached to a definition, relation or permission",
        doc.span,
        Some(
            "a doc comment documents the item directly below it; move it there, or use `//` \
             for a plain comment"
                .to_owned(),
        ),
        src,
    )
}

/// Source span of the name of member `rel` on type `ty`. Valid because warnings only run
/// when there are no errors, so IR ids equal AST positions.
fn member_name_span(ast: &ast::Schema, ty: TypeId, rel: RelId) -> Span {
    ast.definitions
        .get(ty.index())
        .and_then(|d| d.members.get(rel.index()))
        .map_or_else(Span::default, |m| member_ident(m).span)
}

fn unused_relations(schema: &CompiledSchema, ast: &ast::Schema, src: &str) -> Vec<SchemaError> {
    let mut used = HashSet::new();
    for t in &schema.types {
        for member in &t.members {
            match member {
                Member::Relation(r) => {
                    for allowed in &r.allowed {
                        if let AllowedSubject::Set(st, sr) = *allowed {
                            used.insert((st, sr));
                        }
                    }
                }
                Member::Permission(p) => p.expr.visit(&mut |e| match e {
                    Expr::Computed(rel) => {
                        used.insert((t.id, *rel));
                    }
                    Expr::Arrow {
                        tupleset, targets, ..
                    } => {
                        used.insert((t.id, *tupleset));
                        used.extend(targets.iter().map(|(&tt, &tr)| (tt, tr)));
                    }
                    _ => {}
                }),
            }
        }
    }
    let mut warnings = Vec::new();
    for t in &schema.types {
        for (i, member) in t.members.iter().enumerate() {
            if let Member::Relation(r) = member
                && !used.contains(&(t.id, RelId(i)))
            {
                warnings.push(SchemaError::new(
                    ErrorCode::UnusedRelation,
                    format!(
                        "relation `{}` is not used by any permission, arrow or subject set",
                        r.name
                    ),
                    member_name_span(ast, t.id, RelId(i)),
                    Some(
                        "it can still be checked directly; remove it if nothing needs it"
                            .to_owned(),
                    ),
                    src,
                ));
            }
        }
    }
    warnings
}

/// `(wildcard subject type, type, relation)` of a relation whose wildcard can reach a
/// member.
type Witness = (TypeId, TypeId, RelId);

/// Permissions named like write access that a wildcard can grant (§5
/// `W_WILDCARD_ON_WRITE_LIKE_PERMISSION`). A wildcard reaches a member through relations
/// (directly or via subject sets), computed references, arrow targets, any union operand,
/// every intersection operand, and an exclusion's base; never through an excluded operand.
fn wildcards_on_write_like(
    schema: &CompiledSchema,
    ast: &ast::Schema,
    src: &str,
) -> Vec<SchemaError> {
    let mut wild: HashMap<(TypeId, RelId), Witness> = HashMap::new();
    // Monotone fixpoint: each pass can only add entries, and there are finitely many.
    let mut changed = true;
    while changed {
        changed = false;
        for t in &schema.types {
            for (i, member) in t.members.iter().enumerate() {
                let key = (t.id, RelId(i));
                if wild.contains_key(&key) {
                    continue;
                }
                let witness = match member {
                    Member::Relation(r) => r.allowed.iter().find_map(|a| match *a {
                        AllowedSubject::Wildcard(wt) => Some((wt, t.id, RelId(i))),
                        AllowedSubject::Set(st, sr) => wild.get(&(st, sr)).copied(),
                        AllowedSubject::Object(_) => None,
                    }),
                    Member::Permission(p) => wildcard_reaching(&p.expr, t.id, &wild),
                };
                if let Some(w) = witness {
                    wild.insert(key, w);
                    changed = true;
                }
            }
        }
    }

    let mut warnings = Vec::new();
    for t in &schema.types {
        for (i, member) in t.members.iter().enumerate() {
            let Member::Permission(p) = member else {
                continue;
            };
            if !WRITE_LIKE.iter().any(|w| p.name.contains(w)) {
                continue;
            }
            let Some(&(wt, holder_t, holder_r)) = wild.get(&(t.id, RelId(i))) else {
                continue;
            };
            let name_of = |ty: TypeId| schema.type_def(ty).map_or("", |d| &*d.name);
            let holder = schema
                .type_def(holder_t)
                .and_then(|d| d.member(holder_r))
                .map_or("", Member::name);
            warnings.push(SchemaError::new(
                ErrorCode::WildcardOnWriteLikePermission,
                format!(
                    "permission `{}` can be granted to every `{}` through the wildcard on `{}#{holder}`",
                    p.name,
                    name_of(wt),
                    name_of(holder_t)
                ),
                member_name_span(ast, t.id, RelId(i)),
                Some(format!(
                    "SpiceDB recommends wildcards only for read-like permissions; check that \
                     `{}` should be open to every `{}`",
                    p.name,
                    name_of(wt)
                )),
                src,
            ));
        }
    }
    warnings
}

fn wildcard_reaching(
    expr: &Expr,
    ty: TypeId,
    wild: &HashMap<(TypeId, RelId), Witness>,
) -> Option<Witness> {
    match expr {
        Expr::Computed(rel) => wild.get(&(ty, *rel)).copied(),
        Expr::Arrow { targets, .. } => targets
            .iter()
            .find_map(|(&tt, &tr)| wild.get(&(tt, tr)).copied()),
        Expr::Union(xs) => xs.iter().find_map(|x| wildcard_reaching(x, ty, wild)),
        Expr::Intersection(xs) => xs
            .iter()
            .map(|x| wildcard_reaching(x, ty, wild))
            .collect::<Option<Vec<_>>>()
            .and_then(|ws| ws.first().copied()),
        Expr::Exclusion { base, .. } => wildcard_reaching(base, ty, wild),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span::default()
    }

    /// The parser never produces a relation without allowed types, so build one by hand.
    #[test]
    fn empty_relation_types() {
        let src = "definition doc {\n    relation viewer: \n}";
        let name = |text: &str, start: usize| ast::Ident {
            name: text.to_owned(),
            span: Span::new(start, start + text.len()),
        };
        let schema = ast::Schema {
            definitions: vec![ast::Definition {
                doc: None,
                name: ast::TypeName {
                    name: "doc".to_owned(),
                    span: Span::new(11, 14),
                },
                members: vec![ast::Member::Relation(ast::Relation {
                    doc: None,
                    name: name("viewer", 30),
                    allowed: Vec::new(),
                    span: span(),
                })],
                span: span(),
            }],
            dangling_docs: Vec::new(),
        };
        let errs = compile_ast(&schema, src).err().map(|e| e.errors);
        let codes: Option<Vec<_>> = errs.as_ref().map(|e| e.iter().map(|e| e.code).collect());
        assert_eq!(codes, Some(vec![ErrorCode::EmptyRelationTypes]));
        let rendered = errs
            .unwrap_or_default()
            .iter()
            .map(ToString::to_string)
            .collect::<String>();
        insta::assert_snapshot!("empty_relation_types", rendered);
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest("usr", ["user", "group"]), Some("user"));
        assert_eq!(
            suggest("viewr", ["owner", "viewer", "view"]),
            Some("viewer")
        );
        assert_eq!(suggest("team", ["user", "group"]), None);
        assert_eq!(suggest("user", ["user"]), None);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn shortest_cycles() {
        // 0 → 1 → 2 → 0, plus 1 → 0
        let edges = vec![vec![1], vec![0, 2], vec![0]];
        assert_eq!(shortest_cycle(&edges, 0), [0, 1, 0]);
        assert_eq!(shortest_cycle(&edges, 2), [2, 0, 1, 2]);
        assert_eq!(shortest_cycle(&[vec![0]], 0), [0, 0]);
    }
}
