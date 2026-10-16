# 03 — Schema language

Crate: `warden-schema`. No I/O, no dependency on `warden-core` (core depends on schema).

Reference for syntax: https://authzed.com/docs/spicedb/concepts/schema — we implement a
**strict subset** (ADR-004). File extension convention: `.zed`.

## 1. Example accepted schema

```zed
/** a person */
definition user {}

definition group {
    relation member: user | group#member
}

definition folder {
    relation parent: folder
    relation owner: user
    relation viewer: user | user:* | group#member

    permission edit = owner + parent->edit
    permission view = viewer + edit + parent->view
}

definition document {
    relation folder: folder
    relation owner: user
    relation viewer: user | user:* | group#member
    relation banned: user

    permission edit = owner + folder->edit
    permission view = (viewer + edit + folder->view) - banned
    permission view_if_in_all_groups = folder.all(view)
}
```

## 2. Lexical structure

| Token | Pattern / text |
|---|---|
| whitespace | spaces, tabs, `\r`, `\n` — insignificant except as separators |
| line comment | `//` to end of line — discarded |
| block comment | `/* ... */` (not starting `/**`) — discarded, no nesting |
| doc comment | `/** ... */` — kept, attached to the next definition/relation/permission |
| keywords | `definition` `relation` `permission` `caveat` `use` `with` `self` `nil` |
| identifier | `[a-z_][a-z0-9_]*` (length/termination rules checked by the validator, not the lexer, so errors are friendlier) |
| `/` | type-prefix separator, only inside type references |
| punctuation | `{ } ( ) : = | # , .` |
| operators | `+` `&` `-` `->` |
| wildcard | `*` (only valid after `:` in a relation type) |

- Lexer is hand-written (or `logos`), producing tokens with byte spans.
- Unterminated block comment → `E_UNTERMINATED_COMMENT`.
- Any other character → `E_UNEXPECTED_CHAR`.
- Doc comments: `/**/` is an empty plain comment, not a doc comment. Consecutive doc
  comments before one node are joined with `\n`. A doc comment with nothing to attach to
  (before `}`, at end of file, or inside a member) is kept out of the tree and reported as
  the warning `W_DANGLING_DOC_COMMENT` (OQ-010).
- Type names and type references are written without whitespace or comments around `/`,
  `#` and `:` (`docs/document`, `group#member`, `user:*`). `docs / document`,
  `group # member` and `user : *` are `E_UNEXPECTED_TOKEN` (OQ-011). The `:` after a
  relation name is not part of a type reference and may be spaced freely.

## 3. Grammar (EBNF)

```
schema          = { top_level } EOF ;
top_level       = [ doc_comment ] ( definition | unsupported_top ) ;
unsupported_top = "caveat" ... | "use" ... ;              (* → E_UNSUPPORTED_FEATURE *)

definition      = "definition" type_name "{" { member } "}" ;
type_name       = ident { "/" ident } ;
member          = [ doc_comment ] ( relation | permission ) ;

relation        = "relation" ident ":" allowed_type { "|" allowed_type } ;
allowed_type    = type_name [ "#" ident ]                   (* subject set *)
                | type_name ":" "*"                         (* wildcard *)
                ;
(* "with <caveat>" / "with expiration" after allowed_type → E_UNSUPPORTED_FEATURE *)

permission      = "permission" ident [ ":" type_list ] "=" expr ;
(* a type annotation (": user | team") → E_UNSUPPORTED_FEATURE (typechecking) *)

expr            = term { binop term } ;                     (* see §4.3 mixing rule *)
binop           = "+" | "&" | "-" ;
term            = "(" expr ")"
                | ident "->" ident                          (* arrow *)
                | ident "." "any" "(" ident ")"            (* arrow alias *)
                | ident "." "all" "(" ident ")"            (* intersection arrow *)
                | ident                                     (* computed userset *)
                | "self"                                    (* → E_UNSUPPORTED_FEATURE *)
                | "nil"                                     (* → E_UNSUPPORTED_FEATURE *)
                ;
```

Notes:
- Members are separated by whitespace/newlines only. A `;` is rejected with
  `E_UNEXPECTED_TOKEN` and the hint "remove ';' — the schema language does not use
  semicolons".
- Relations and permissions may appear in any order and may reference later members.
- An empty definition body `{}` is valid.
- `->` chains (`a->b->c`) are **not** valid: the right side of an arrow must be a single
  identifier on the related type. Reject with `E_ARROW_CHAIN` and suggest an intermediate
  permission.

## 4. Expressions

### 4.1 Meaning

| Syntax | IR node | Meaning (set of subjects) |
|---|---|---|
| `r` | `Computed(r)` | subjects of relation/permission `r` on **this** object |
| `a + b` | `Union[a, b]` | a ∪ b |
| `a & b` | `Intersection[a, b]` | a ∩ b |
| `a - b` | `Exclusion{base: a, sub: b}` | a \ b |
| `t->p` / `t.any(p)` | `Arrow{tupleset: t, computed: p, mode: Any}` | ∪ over objects O related via relation `t`: subjects of `p` on O |
| `t.all(p)` | `Arrow{tupleset: t, computed: p, mode: All}` | ∩ over objects O related via `t` (empty → ∅, ADR-013) |

Direct (stored) membership is the implicit meaning of a **relation**; a relation's IR is
`Direct(rel)`.

### 4.2 Arrow semantics details

- The left side `t` **must be a relation** (not a permission) on the same type → else
  `E_ARROW_LEFT_NOT_RELATION`.
- The arrow walks the **object** of each subject stored on `t`. If a stored subject is a
  subject set (`group:eng#member`), its `#member` is **ignored** and the arrow continues
  from `group:eng` (this is SpiceDB's documented behaviour).
- Wildcard subjects stored on `t` are **skipped** by the arrow. Additionally, schema
  validation **rejects** an arrow whose left relation allows a wildcard type
  (`E_ARROW_OVER_WILDCARD`) — conservative; record in OPEN_QUESTIONS if SpiceDB accepts it.
- The right side `p` must exist (as relation or permission) on **at least one** of the
  object types allowed on `t` → else `E_ARROW_TARGET_MISSING`. At runtime, related objects
  whose type lacks `p` contribute nothing (∅).

### 4.3 Precedence and the mixing rule (ADR-004)

SpiceDB, for historical reasons, gives union higher precedence than intersection and
exclusion: `a + b & c` is `(a + b) & c`. To avoid silent surprises we enforce:

> **Within a single parenthesised level, all binary operators must be the same.**

- `a + b + c` ✅ → `Union[a, b, c]` (flattened)
- `a & b & c` ✅ → `Intersection[a, b, c]`
- `a - b - c` ✅ → left-assoc: `Exclusion{Exclusion{a, b}, c}`
- `a + b & c` ❌ → `E_AMBIGUOUS_OPERATORS` with a fix-it showing both parenthesisations
- `(a + b) & c` ✅
- `a + (b & c)` ✅
- Arrows/`.any`/`.all` are terms, so `a + b->c` ✅.

The `E_AMBIGUOUS_OPERATORS` diagnostic carries a fix-it with both parenthesisations. When
one of the mixed operators is `+`, it also carries a note saying how SpiceDB would read the
input: SpiceDB documents union as binding tighter than `&` and `-`, so `a + b & c` is
`(a + b) & c` there. The relative precedence of `&` and `-` is not documented, so a
`&`/`-` mix gets no note.

Flatten nested same-operator unions/intersections in the IR (`Union[Union[a,b],c]` →
`Union[a,b,c]`). Never flatten exclusion.

## 5. Validation rules (semantic pass)

Run after parsing; collect **all** errors (do not stop at the first), sorted by span.

| Code | Rule |
|---|---|
| `E_IDENT_FORMAT` | identifier violates `docs/02-data-model.md` §1.1 |
| `E_DUPLICATE_TYPE` | two definitions with the same full type name |
| `E_DUPLICATE_MEMBER` | relation/permission name repeated within a definition |
| `E_UNKNOWN_TYPE` | allowed type references an undefined type |
| `E_UNKNOWN_SUBJECT_RELATION` | `t#r` where `r` is not defined on `t` |
| `E_UNKNOWN_MEMBER` | expression references a name not defined on this type |
| `E_ARROW_LEFT_NOT_RELATION` | arrow left side is a permission |
| `E_ARROW_OVER_WILDCARD` | arrow left relation allows a wildcard |
| `E_ARROW_TARGET_MISSING` | arrow right side exists on none of the left relation's types |
| `E_ARROW_CHAIN` | `a->b->c` |
| `E_DIRECT_CYCLE` | permission refers to itself through computed usersets only (no arrow and no relation in between), e.g. `permission a = b`, `permission b = a + x` |
| `E_AMBIGUOUS_OPERATORS` | mixing rule §4.3 |
| `E_EMPTY_RELATION_TYPES` | relation with no allowed types (parser normally prevents) |
| `E_DUPLICATE_ALLOWED_TYPE` | `relation r: user | user` |
| `E_UNSUPPORTED_FEATURE` | caveats, `with`, `use`, `self`, `nil`, type annotations |
| `E_RESERVED_NAME` | member named like a keyword (`definition`, `relation`, `permission`, `self`, `nil`, `caveat`, `use`, `with`, `any`, `all`) |

Warnings (returned, not fatal):
- `W_WILDCARD_ON_WRITE_LIKE_PERMISSION`: a wildcard relation flows into a permission whose
  name matches `edit|write|delete|admin|manage|owner` (heuristic; mirrors SpiceDB's advice
  to only use wildcards for read permissions). "Matches" is a substring match (`can_edit`,
  `editor`). A wildcard flows through relations (directly or via a subject set
  `t#r` whose `r` it reaches), computed references, arrow targets, any operand of a union,
  every operand of an intersection, and the base of an exclusion; never through an excluded
  operand.
- `W_UNUSED_RELATION`: relation referenced by no permission, no arrow (as tupleset or
  target) and no subject set `t#r` (informational).
- `W_DANGLING_DOC_COMMENT`: a doc comment with nothing to attach to (§2).

Semantic warnings (`W_WILDCARD_...`, `W_UNUSED_...`) are computed on the IR, so they are
only reported when compilation succeeds; `W_DANGLING_DOC_COMMENT` is also returned
alongside errors.

Cycle detection for `E_DIRECT_CYCLE`: build a graph whose nodes are `(type, permission)` and
whose edges are `Computed(x)` references to *permissions* on the same type; any cycle is an
error. Edges through relations or arrows are not included (data-dependent recursion such as
`parent->view` is legal and common).

## 6. Compiled IR

```rust
pub struct CompiledSchema {
    pub types: Vec<TypeDef>,                      // indexed by TypeId
    type_by_name: HashMap<Arc<str>, TypeId>,
    pub source: Arc<str>,                         // original text, for storage/round-trip
    pub hash: [u8; 32],                           // sha-256 of normalised source (T0.5)
    used_by: HashMap<(TypeId, RelId), Vec<(TypeId, RelId)>>,  // derived indexes, below
    arrows_into: HashMap<(TypeId, RelId), Vec<(TypeId, RelId, RelId)>>,
    subject_set_uses: HashMap<(TypeId, RelId), Vec<(TypeId, RelId)>>,
}
// Accessors: type_id(&str) -> Option<TypeId>, type_def(TypeId) -> Option<&TypeDef>,
// used_by / arrows_into / subject_set_uses(TypeId, RelId) -> &[...] (empty if none).
// TypeDef: member_id(&str) -> Option<RelId>, member(RelId) -> Option<&Member>.
pub struct TypeId(usize);   // .index(); only produced by compile
pub struct RelId(usize);    // .index(); position in its type's members

pub struct TypeDef {
    pub id: TypeId,
    pub name: Arc<str>,
    pub doc: Option<String>,
    pub members: Vec<Member>,                     // indexed by RelId local to the type
    member_by_name: HashMap<Arc<str>, RelId>,
}

pub enum Member {
    Relation(RelationDef),
    Permission(PermissionDef),
}

pub struct RelationDef {
    pub name: Arc<str>,
    pub doc: Option<String>,
    pub allowed: Vec<AllowedSubject>,
}

pub enum AllowedSubject {
    Object(TypeId),
    Set(TypeId, RelId),
    Wildcard(TypeId),
}

pub struct PermissionDef {
    pub name: Arc<str>,
    pub doc: Option<String>,
    pub expr: Expr,
}

pub enum Expr {
    Computed(RelId),                               // relation or permission on same type
    Arrow { tupleset: RelId, computed: Arc<str>, mode: ArrowMode,
            targets: BTreeMap<TypeId, RelId> },    // per-arrow target map, see below
    Union(Vec<Expr>),
    Intersection(Vec<Expr>),
    Exclusion { base: Box<Expr>, sub: Box<Expr> },
}
pub enum ArrowMode { Any, All }
```

`Arrow.computed` is a name (not a `RelId`) because it resolves per related type; the
per-arrow `targets` map precomputes it for the hot path: one entry per type allowed on the
tupleset (as object or subject set) that has `computed`. Related objects of other types
contribute ∅. (`BTreeMap` rather than `HashMap` so the IR has a deterministic `Debug`
form; lookups are on a handful of entries.)

Index lists are sorted and deduplicated. The `hash` field is produced by the pretty printer's
normalisation (§9).

Derived indexes computed at compile time (used by lookups, `docs/04-evaluation.md` §5):
- `used_by[(type, rel)]` → list of `(type, permission)` whose expression contains
  `Computed(rel)` anywhere.
- `arrows_into[(target_type, target_rel)]` → list of `(source_type, tupleset_rel, permission_containing_arrow)`
  for every arrow `tupleset->target_rel` where `target_type` is allowed on `tupleset`.
- `subject_set_uses[(type, rel)]` → list of `(resource_type, relation)` whose allowed
  subjects include `type#rel`.

## 7. Unsupported SpiceDB features → `E_UNSUPPORTED_FEATURE`

Each must produce a specific message naming the feature:
`caveat` definitions; `with <caveat>` and `with expiration` on allowed types;
`use typechecking`, `use self`, any `use`; the `self` keyword; `nil`; permission type
annotations; any `.function(...)` other than `.any` / `.all`.

## 8. Errors

```rust
pub struct SchemaError {
    pub code: ErrorCode,          // enum, Display = "E_..."
    pub message: String,          // human sentence
    pub span: Span,               // byte range
    pub line: u32, pub col: u32,  // 1-based, computed from span (col counts characters)
    pub source_line: String,      // text of line `line`, so Display needs no source
    pub note: Option<String>,     // extra context, e.g. how SpiceDB would read it (§4.3)
    pub help: Option<String>,     // fix-it hint
}

pub struct SchemaErrors {         // returned by compile: errors (never empty) + warnings,
    pub errors: Vec<SchemaError>, // each sorted by span
    pub warnings: Vec<SchemaError>,
}

pub fn compile(source: &str) -> Result<Compiled, SchemaErrors>;
pub struct Compiled { pub schema: CompiledSchema, pub warnings: Vec<SchemaError> }
```
Warnings use the same `SchemaError` type; `ErrorCode::is_warning()` is true for `W_` codes,
and `Display` then says `warning[W_...]` instead of `error[E_...]`.
`Display` renders rustc-style with a source excerpt and caret.
`SchemaError::display_with_path(path)` renders the same with the file in the location
line (`--> schema.zed:3:5`), for tools such as the CLI. Tests snapshot the rendered
output (`insta`).

The parser (`warden_schema::parse`, source → AST with byte spans) stops at the first lexical
or syntax error: `E_UNEXPECTED_CHAR`, `E_UNTERMINATED_COMMENT`, `E_UNEXPECTED_TOKEN`,
`E_AMBIGUOUS_OPERATORS`, `E_ARROW_CHAIN`, `E_UNSUPPORTED_FEATURE`. Collecting all errors
(§5) applies to the semantic pass. The parser accepts keywords as relation/permission names
(`relation nil: user`) so the validator can report `E_RESERVED_NAME`.

## 9. Pretty printer

`CompiledSchema::to_canonical_string()` emits normalised source: 4-space indent, one blank
line between definitions, relations before permissions in original relative order, doc
comments preserved, redundant parentheses removed only where the mixing rule still holds.
Round-trip property (OQ-012). Printing moves relations ahead of permissions, which
renumbers member `RelId`s when the input interleaves them, and a recompiled schema's
`source` is the printed text, so literal IR equality `compile(print(compile(s))) ==
compile(s)` does not hold in general. Instead, for every valid `s`, with `a = compile(s)`
and `b = compile(print(a))`:
1. **Same schema by name**: `a` and `b` are equal once every `TypeId`/`RelId` is replaced by
   the name it stands for, and `a.hash == b.hash`.
2. **Fixed point**: `print(b) == print(a)`, so `compile(print(b)) == b` exactly (the whole
   `CompiledSchema`, `source` included): the IR round-trips exactly once it is canonical.
3. **Stable ids for canonical order**: if `s` already lists relations before permissions in
   every definition, `a.types == b.types`.

Details fixed in T0.5:
- A definition with no members prints as `definition name {}`. Inside a body, one blank line
  separates the relation group from the permission group; no other blank lines.
- Allowed types are joined with ` | `; binary operators are surrounded by single spaces.
- `t.any(p)` prints as `t->p` (alias); `t.all(p)` is kept.
- An operand is parenthesised iff it contains a binary operator, except the left operand of
  `-` when it is itself an exclusion (`a - b - c`).
- A doc comment prints as `/** text */` when the text is one line that is non-empty and does
  not start with whitespace or `*`; otherwise as a `/**` … ` * line` … ` */` block, so the
  parser reads back exactly the same text. Plain comments and dangling doc comments are
  not printed. Output ends with a newline (an empty schema prints as the empty string).

`CompiledSchema::hash` is the SHA-256 of `to_canonical_string()`, so it ignores formatting,
plain comments and relation/permission interleaving, but changes with doc comments.

### 9.1 Comment-preserving formatter (T3.8)

`warden_schema::format(source) -> Result<String, FormatError>` (used by
`warden schema fmt`) prints the canonical form above with every plain comment (`//`,
`/* */`) and every dangling doc comment kept, verbatim except that a `//` comment loses
trailing whitespace and line breaks become `\n`. Without such comments its output is
exactly `to_canonical_string()`. Placement:
- A comment after code on the same line stays at the end of that line: after a member,
  after `definition x {`, or after `}`. Several are joined with single spaces.
- A comment on its own line goes before the next member (inside a body) or definition (at
  top level), before its doc comment; if nothing follows, before the body's `}` or at the
  end of the file. Comments follow their member when canonical order moves it.
- A comment inside a member or a definition header (between tokens of `relation x: ...`,
  a permission expression, or `definition x {`) moves to its own line before that member
  or definition.
- A dangling doc comment inside a definition always goes just before `}`, since printed
  before a member it would attach to it.
- Blank lines: as in §9 inside bodies; outside definitions a blank line is kept before and
  after a comment where the source had one (at least one).
- Indentation: own-line comments inside a body get 4 spaces; continuation lines of a
  multi-line `/* */` are kept as written.

`format` is a fixed point (`format(format(s)) == format(s)`) and its output compiles to a
schema with the same hash. It checks both before returning: the output must compile to
the same canonical form and contain the same comments, else `FormatError::NotPreserved`
and no output. A schema that does not compile is `FormatError::Schema`.

## 10. Schema changes against existing data

`Engine::write_schema(new)` must refuse (`SCHEMA_CHANGE_ORPHANS`: the
`Error::SchemaChangeOrphans` variant; runtime error codes from `Error::code()` carry no
`E_` prefix, which is reserved for schema diagnostics, `06` §1.1, OQ-030) when, at head, any live tuple would fail write validation
(`02` §6) under the new schema. That includes:
- a relation with live tuples removed, or turned into a permission of the same name;
- an allowed subject type, subject set or wildcard with live tuples using it removed from a
  relation (including a subject set `x#p` removed because `p` was deleted from `x`;
  wildcards are counted apart from objects);
- a type with live tuples (as resource or subject) removed.
The error lists each offending group (relation, subject form, code) with a count of
tuples. Removing a permission is allowed whenever no live tuple uses it as a subject
relation. See `docs/05-storage.md` §7.
