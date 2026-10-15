# 02 — Data model

All types in this document live in `warden-core::model` unless stated. Names are fixed.

## 1. Identifiers

### 1.1 Schema identifiers (type names, relation names, permission names)

Follow SpiceDB's identifier rules:

- Regex (one segment): `^[a-z_][a-z0-9_]{1,62}[a-z0-9]$`
  - starts with a lowercase letter or underscore
  - contains lowercase letters, digits, underscores
  - **3 to 64 characters**, must **end with a letter or digit**
- Valid: `user`, `doc`, `_ab`, `parent_folder`, `view2`
- Invalid: `ab` (too short), `_a`, `View` (uppercase), `trailing_`, `1abc`

Object **type** names may additionally carry one or more prefixes separated by `/`, each
segment obeying the rule above: `docs/document`, `iam/group`.
Full type name max length: 128 bytes (our limit; log as open question if SpiceDB differs).

> Note for implementer: SpiceDB's docs state the 3–64 rule for identifiers. Confirm prefix
> segment rules against SpiceDB behaviour in differential tests (§`07` 4) and record any
> difference in `OPEN_QUESTIONS.md`.

### 1.2 Object IDs

- Allowed set: `^[A-Za-z0-9_\-=+/|.]{1,1024}$`
- `@` is **not** allowed: it separates resource from subject in tuple strings. (SpiceDB's
  own ID rules may differ; record differences found in differential tests in
  `OPEN_QUESTIONS.md`.)
- The single character `*` is reserved for wildcard subjects and is **never** a valid
  resource ID.
- IDs are case-sensitive, compared byte-wise.
- `:` and `#` are not allowed (they are tuple-string delimiters).

## 2. Core types (Rust)

```rust
/// Interned or owned type name, e.g. "document" or "docs/document".
pub struct TypeName(Arc<str>);
/// Relation or permission name.
pub struct RelName(Arc<str>);
/// Object id; never "*".
pub struct ObjectId(Arc<str>);

/// A concrete object: `document:readme`.
pub struct ObjectRef {
    pub object_type: TypeName,
    pub object_id: ObjectId,
}

/// The subject side of a tuple or a query.
pub enum SubjectRef {
    /// `user:anne`
    Object(ObjectRef),
    /// `group:eng#member`
    Set { object: ObjectRef, relation: RelName },
    /// `user:*` — only valid inside stored tuples and lookup results, never as the
    /// subject of a `check` call.
    Wildcard { object_type: TypeName },
}

/// A stored relationship tuple.
pub struct Relationship {
    pub resource: ObjectRef,
    pub relation: RelName,      // must be a *relation*, never a permission
    pub subject: SubjectRef,
}

/// Monotonic, store-assigned.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct Revision(pub u64);

pub enum Consistency {
    /// Use the store's head revision at the moment the request starts.
    Latest,
    /// Use head, but fail with RevisionNotYetAvailable if head < r.
    AtLeast(Revision),
    /// Use exactly r. Fails if r > head or r < GC horizon.
    Exact(Revision),
}
```

Implementation notes:
- `TypeName`/`RelName` are cheap to clone. The compiled schema additionally maps them to
  dense `u32` ids (`TypeId`, `RelId`) for the evaluator's hot path.
- All types implement `Debug, Clone, Eq, Hash, Ord` and `Display` in tuple-string syntax.
- `FromStr` is implemented for `TypeName`, `RelName`, `ObjectId`, `ObjectRef`, `SubjectRef`,
  `Relationship` and `Revision`, parsing the canonical string syntax below.
  `Consistency`, `WriteOp` and `RelationshipFilter` (§5) implement `Display` for debugging
  only; that output is not a stable format and cannot be parsed (OQ-009).
- The §1.1 identifier rules live in `warden-schema` (`warden_schema::ident`), shared by the
  schema validator and the tuple parser.

## 3. Canonical string syntax

```
object        := type ":" id
subject       := object | object "#" relation | type ":*"
relationship  := object "#" relation "@" subject
check_string  := object "#" permission_or_relation "@" subject     (used in fixtures)
revision      := "0" | [1-9] [0-9]*                                (fits in u64)
```

Examples:

| String | Parsed |
|---|---|
| `document:readme#viewer@user:anne` | resource `document:readme`, relation `viewer`, subject Object `user:anne` |
| `document:readme#viewer@group:eng#member` | subject Set `group:eng#member` |
| `document:public#viewer@user:*` | subject Wildcard `user` |
| `docs/document:a#parent@docs/folder:root` | prefixed types |

Parsing rules:
- Split resource from subject on the **first** `@`.
- Resource part: split on the **last** `#`, then the object on the **first** `:`.
- Subject part: if it ends in `:*` → wildcard; else split on the first `#` (if any) for a
  subject relation, then the object on the first `:`.
- Every component is validated against §1. Errors: `ParseError { input, position, reason }`.
- `Display` must round-trip: `parse(display(x)) == x` (property-tested).

## 4. Revisions and consistency semantics

- Revision `0` means "empty store, nothing written". The first write batch commits at `1`.
- **Head revision** = highest committed revision.
- A tuple row is **visible at revision R** iff `created_rev <= R && (deleted_rev IS NULL || deleted_rev > R)`.
- A request resolves its `Consistency` to exactly one revision **once**, at the start, and
  uses it for every store read in that request, all through one store snapshot
  (`docs/05-storage.md` §2, "Revision checks"). This is what makes a check internally
  consistent even while writes commit, or another process garbage-collects, concurrently.
- Resolution:
  - `Latest` → `head`.
  - `AtLeast(r)` → if `head >= r` then `head` else `Err(RevisionNotYetAvailable { requested: r, head })`.
  - `Exact(r)` → if `r > head` → `Err(RevisionNotYetAvailable)`; if `r < gc_horizon` →
    `Err(RevisionExpired { requested: r, horizon })`; else `r`.
- Every read API returns the revision it evaluated at (`CheckResponse.revision`, etc.).

## 5. Write operations

```rust
pub enum WriteOp {
    /// Create; fails the whole batch if a visible identical tuple exists.
    Create(Relationship),
    /// Create if absent; no-op if present. Idempotent.
    Touch(Relationship),
    /// Delete if present; no-op if absent. Idempotent.
    Delete(Relationship),
}

pub enum Precondition {
    MustExist(RelationshipFilter),
    MustNotExist(RelationshipFilter),
}

pub struct RelationshipFilter {
    pub resource_type: TypeName,
    pub resource_id: Option<ObjectId>,
    pub relation: Option<RelName>,
    pub subject_type: Option<TypeName>,
    pub subject_id: Option<String>,          // may be "*" to match wildcard subjects
    pub subject_relation: Option<Option<RelName>>, // Some(None) = must have no subject relation
}

pub struct WriteRequest {
    pub ops: Vec<WriteOp>,
    pub preconditions: Vec<Precondition>,
}
```

Batch semantics (all-or-nothing), checked in this order; the first failure aborts the
batch and nothing is written (OQ-016):
1. More than `EngineConfig::max_write_batch` ops (default 1000) → `Err(BatchTooLarge)`.
2. No schema written yet → `Err(NoSchema)`.
3. Validate every op, `Delete` included, against the current schema (§6). The first failure
   → `Err(InvalidRelationship)`. A precondition filter naming a type, relation or subject
   relation the schema does not define, or a permission as its relation, →
   `Err(InvalidRequest)` (such a filter can never match, so a typo would make
   `MustNotExist` pass silently; OQ-017).
4. The same relationship may not appear twice in one batch, whatever the ops →
   `Err(DuplicateInBatch)`.
5. Check preconditions over live tuples at the current head (inside the write
   transaction). The first failure → `Err(PreconditionFailed { index })`.
6. Allocate a new revision `R`.
7. Apply ops: `Create`/`Touch` insert with `created_rev = R`; `Delete` sets `deleted_rev = R`
   on the visible row. `Create` of a live tuple → `Err(AlreadyExists)`.
8. If no op changed anything (all Touch-existing / Delete-absent, or no ops), **still**
   allocate the revision (simpler, monotonic, harmless). Return it.

Steps 1–3 are the engine's; steps 4–8 are the store's (`docs/05-storage.md` §5).

`DeleteByFilter(RelationshipFilter)` is a separate API (`delete_relationships(filter,
limit)`, T2.6): one transaction, one new revision (allocated even if nothing matched, as
rule 8), deleting at most `limit` live matches in storage order (`limit` defaults to and
may not exceed `max_delete_by_filter`, 10 000). It returns how many were deleted and
whether more remain; call again until `more` is false. Not an error when more match.

## 6. Relationship validation against the schema

A `Relationship` is valid iff (checked in this order; the error is
`InvalidRelationship { rel, code, message }` with `rel` in tuple-string syntax):
1. `resource.object_type` is a defined type → else `E_UNKNOWN_TYPE`.
2. `relation` is defined on that type → else `E_UNKNOWN_RELATION`; and is a **relation**
   (not a permission) → else `E_WRITE_TO_PERMISSION`.
3. The subject matches one of the relation's allowed subject types → else
   `E_SUBJECT_TYPE_NOT_ALLOWED` (this includes a subject type or subject relation the schema
   does not define at all):
   - `Object(t:id)` matches allowed `t`.
   - `Set(t:id#r)` matches allowed `t#r` exactly.
   - `Wildcard(t)` matches allowed `t:*`.
4. IDs satisfy §1.2 (guaranteed by construction: `ObjectId` cannot hold an invalid id).

The codes are variants of `warden_schema::ErrorCode`, so schema and write failures share one
code namespace (`expectErrors` in fixtures, `docs/07-testing.md` §3). OQ-017.

## 7. Limits (defaults in `EngineConfig`)

| Setting | Default |
|---|---|
| `max_depth` | 50 |
| `max_write_batch` | 1000 |
| `max_delete_by_filter` | 10 000 |
| `max_lookup_results` (per page) | 1000 |
| `max_lookup_nodes` (lookup_resources phase 1) | 200 000 |
| `max_check_fanout` (tuples read per node before erroring) | 100 000 |
| `max_check_reads` (store reads per `check` / `check_bulk` item before erroring) | 200 000 |
| `gc_window` | none (manual GC only in MVP) |
