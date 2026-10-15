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
  comments before one node are joined with `\n`. A doc comment followed by nothing it can
  attach to (before `}` or end of file), or one inside a member, is discarded like a plain
  comment (OQ-010).
- Type names are written without whitespace around `/` (`docs/document`); `docs / document`
  is `E_UNEXPECTED_TOKEN` (OQ-011).

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
  to only use wildcards for read permissions).
- `W_UNUSED_RELATION`: relation referenced by no permission and no arrow (informational).

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
    pub hash: [u8; 32],                           // sha-256 of normalised source
}

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
    Arrow { tupleset: RelId, computed: Arc<str>, mode: ArrowMode },
    Union(Vec<Expr>),
    Intersection(Vec<Expr>),
    Exclusion { base: Box<Expr>, sub: Box<Expr> },
}
pub enum ArrowMode { Any, All }
```

`Arrow.computed` is a name (not a `RelId`) because it resolves per related type; precompute
a per-arrow `HashMap<TypeId, RelId>` for the hot path.

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
    pub help: Option<String>,     // fix-it hint
}
```
`Display` renders rustc-style with a source excerpt and caret. Tests snapshot the rendered
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
Property: `compile(print(compile(s))) == compile(s)` (IR equality).

## 10. Schema changes against existing data

`Engine::write_schema(new)` must refuse (`E_SCHEMA_CHANGE_ORPHANS`) when, at head:
- a relation with visible tuples would be removed, or
- an allowed subject type with visible tuples using it would be removed from a relation, or
- a type with visible tuples (as resource or subject) would be removed.
The error lists each offending relation and a count of tuples. Removing permissions is
always allowed. See `docs/05-storage.md` §7 for the queries.
