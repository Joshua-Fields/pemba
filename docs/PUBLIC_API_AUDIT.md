# Public API audit for 0.1.0

T7.2 completes the existing T3.9 audit under the maintainer's T7.1 decisions
(OQ-044; `08-milestones.md` T3.9/T7.1/T7.2). This inventory covers every pemba-owned
public struct and enum in every manifest eligible for crates.io publication, including
public modules, macro-generated validated name types and facade reexports.

`Non-exhaustive` means `#[non_exhaustive]`; fields remain public unless already private.
A downstream constructor followed by field assignment replaces a request struct literal.
`Exhaustive, opaque` means the attribute is absent but private fields already prevent
external literal construction and allow implementation fields to grow. These are
intentional opaque values, not a promise that their private layout is frozen.

Extensible requests, results, configuration and diagnostics reserve room for new fields
or cases. Stable value products and closed supported semantic partitions remain
exhaustive for the explicit reasons below. Adding a new operation or semantic partition
to an exhaustive value requires deliberate specification and API revision, rather than
being treated as an incidental compatible change. Existing EngineConfig constructors
and setters are reused unchanged.

## Inventory

Each row names one declaration, using its public module path (not a private source-module
path). Construction describes the supported producer or downstream construction route;
engine-owned results do not need synthetic user constructors. Store-facing result
constructors remain public so third-party store implementations can produce them.

### pemba-schema

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `ast::Schema` | Exhaustive | Fixed source-order list of definitions. | Public fields / struct literal. |
| `ast::DocComment` | Exhaustive | Fixed attached text and source range. | Public fields / struct literal. |
| `ast::Ident` | Exhaustive | Fixed identifier text and source range. | Public fields / struct literal. |
| `ast::TypeName` | Exhaustive | Fixed qualified type-name text and source range. | Public fields / struct literal. |
| `ast::Definition` | Exhaustive | Fixed definition name, documentation, members and range. | Public fields / struct literal. |
| `ast::Relation` | Exhaustive | Fixed relation declaration shape in the supported DSL. | Public fields / struct literal. |
| `ast::AllowedType` | Exhaustive | Fixed allowed subject type, form and range. | Public fields / struct literal. |
| `ast::Permission` | Exhaustive | Fixed permission declaration shape in the supported DSL. | Public fields / struct literal. |
| `ast::Expr` | Exhaustive | Fixed expression kind and source range; operator extension is represented by ExprKind. | Public fields / struct literal. |
| `ast::Member` | Exhaustive | Closed relation-or-permission declaration partition. | Public variants. |
| `ast::AllowedKind` | Exhaustive | Closed object, subject-set and wildcard subject forms. | Public variants. |
| `ast::ArrowMode` | Exhaustive | Closed any/all quantifier pair. | Public variants; also reexported by ir. |
| `ast::ExprKind` | Non-exhaustive (existing) | Parser syntax variants can grow, including recognition of unsupported syntax. | Public variants; downstream matches require a wildcard. |
| `Compiled` | Non-exhaustive (existing) | Compilation may report additional output metadata. | Produced by compile; read public fields. |
| `ErrorCode` | Non-exhaustive (existing) | New diagnostics need new stable codes. | Public variants; wildcard in downstream matches. |
| `SchemaError` | Non-exhaustive (existing) | Diagnostic fields may grow. | Produced by parsing/compilation; read public fields. |
| `SchemaErrors` | Non-exhaustive (existing) | Collected diagnostic metadata may grow. | Produced by parsing/compilation; read public fields. |
| `FormatError` | Non-exhaustive (existing) | Formatting failure cases may grow. | Produced by format; match with wildcard. |
| `ident::IdentErrorKind` | Non-exhaustive (existing) | Identifier validation failure cases may grow. | Public variants; match with wildcard. |
| `ident::IdentError` | Non-exhaustive (existing) | Identifier diagnostic metadata may grow. | Produced by identifier validation; read public fields. |
| `ir::TypeId` | Exhaustive, opaque | Single validated type index; private payload prevents external literals. | Obtained from compiled schema; index() reads it. |
| `ir::RelId` | Exhaustive, opaque | Single validated member index; private payload prevents external literals. | Obtained from compiled schema; index() reads it. |
| `ir::CompiledSchema` | Exhaustive, opaque | Private lookup indexes already permit internal field extension. | Produced by compile. |
| `ir::TypeDef` | Exhaustive, opaque | Private member index already permits internal field extension. | Obtained from CompiledSchema::types. |
| `ir::Member` | Exhaustive | Closed compiled relation-or-permission partition. | Public variants. |
| `ir::RelationDef` | Exhaustive | Frozen compiled relation value: name, documentation and allowed subjects. | Public fields / struct literal. |
| `ir::AllowedSubject` | Exhaustive | Closed supported object, set and wildcard subject forms. | Public variants. |
| `ir::PermissionDef` | Exhaustive | Frozen compiled permission value: name, documentation and expression. | Public fields / struct literal. |
| `ir::Expr` | Exhaustive | Closed evaluator algebra for the supported DSL; adding an operation requires deliberate semantic/API revision. | Public variants; differs intentionally from parser ast::ExprKind. |
| `Span` | Exhaustive | Stable half-open byte range has exactly start and end. | Span::new(start, end), Default or public fields. |

### pemba-core

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `EngineConfig` | Non-exhaustive (existing) | Evaluation limits may grow. | Existing Default and with_* setters; no rework. |
| `EngineBuilder` | Exhaustive, opaque | Private builder state permits internal field extension. | Engine::builder(), store/config/build. |
| `Engine` | Exhaustive, opaque | Private state permits internal field extension. | Engine::builder().store(...).build(). |
| `SchemaWriteResult` | Non-exhaustive (T7.2) | Schema-write outcome metadata may grow. | Produced by Engine::write_schema. |
| `ReadRelationshipsRequest` | Non-exhaustive (T7.2) | Read options may grow. | Existing new(filter); assign public consistency/limit/cursor. |
| `ReadRelationshipsResponse` | Non-exhaustive (T7.2) | Paging outcome metadata may grow. | Produced by Engine::read_relationships. |
| `OrphanReport` | Non-exhaustive (T7.2) | Schema-change diagnostics may grow. | Produced by Error::SchemaChangeOrphans. |
| `CheckRequest` | Non-exhaustive (T7.2) | Per-check options may grow. | new(resource, permission, subject), initially Latest; assign public consistency. |
| `CheckResponse` | Non-exhaustive (T7.2) | Check outcome metadata may grow. | Produced by Engine::check. |
| `CheckItem` | Non-exhaustive (T7.2) | Per-item bulk-check options may grow. | new(resource, permission, subject). |
| `EvalStats` | Non-exhaustive (T7.2) | Evaluation counters may grow. | Existing Default; assign public counters when needed. |
| `LookupResourcesRequest` | Non-exhaustive (T7.2) | Lookup/paging options may grow. | Existing new(resource_type, permission, subject); assign public options. |
| `LookupResourcesResponse` | Non-exhaustive (T7.2) | Lookup/paging outcome metadata may grow. | Produced by Engine::lookup_resources. |
| `LookupSubjectsRequest` | Non-exhaustive (T7.2) | Subject lookup options may grow. | Existing new(resource, permission, subject_type); assign public consistency. |
| `LookupSubjectsResponse` | Non-exhaustive (T7.2) | Subject lookup outcome metadata may grow. | Produced by Engine::lookup_subjects. |
| `Error` | Non-exhaustive (existing) | Runtime failure cases may grow; fail-closed errors stay distinct from denial. | Public variants; match with wildcard. |
| `CheckTrace` | Non-exhaustive (existing) | Trace output metadata may grow. | Produced by Engine::check_with_trace. |
| `TraceNode` | Non-exhaustive (existing) | Trace node metadata may grow. | Produced within CheckTrace. |
| `TraceKind` | Non-exhaustive (existing) | Trace node kinds may grow. | Public variants; match with wildcard. |
| `TraceSource` | Non-exhaustive (existing) | Trace provenance cases may grow. | Public variants; match with wildcard. |
| `TraceRole` | Non-exhaustive (existing) | Trace operand roles may grow. | Public variants; match with wildcard. |
| `model::Component` | Non-exhaustive (existing) | Parse diagnostics may gain cases. | Public variants; match with wildcard. |
| `model::ParseErrorReason` | Non-exhaustive (existing) | Parse diagnostics may gain cases. | Public variants; match with wildcard. |
| `model::ParseError` | Non-exhaustive (existing) | Parse diagnostic metadata may grow. | Produced by FromStr; read input/position/reason. |
| `model::TypeName` | Exhaustive, opaque | Validated type name is one string; private macro-generated payload enforces validation. | FromStr / parse(); as_str() reads it. |
| `model::RelName` | Exhaustive, opaque | Validated relation/permission name is one string; private macro-generated payload enforces validation. | FromStr / parse(); as_str() reads it. |
| `model::ObjectId` | Exhaustive, opaque | Validated object id is one string; private macro-generated payload enforces validation. | FromStr / parse(); as_str() reads it. |
| `model::ObjectRef` | Exhaustive | Stable value is exactly an object type and object id. | FromStr or public fields. |
| `model::SubjectRef` | Exhaustive | Stable closed object, subject-set and wildcard forms. | FromStr or public variants. |
| `model::Relationship` | Exhaustive | Stable tuple is exactly resource, relation and subject. | FromStr or public fields. |
| `model::Revision` | Exhaustive | Stable u64 revision value; numeric representation is intentional. | Revision(n), Default or FromStr. |
| `model::PendingRevision` | Non-exhaustive (existing), opaque | Store-specific pending/committed state may grow; private fields protect resolution semantics. | Produced by Engine::write_in; resolve after caller commit. |
| `model::Consistency` | Exhaustive | Closed Latest, AtLeast and Exact consistency choices are the stable supported semantics. | Public variants. |
| `model::WriteOp` | Exhaustive | Closed Create, Touch and Delete tuple operations are the stable write algebra. | Public variants holding Relationship. |
| `model::RelationshipFilter` | Non-exhaustive (T7.2) | Filtering options may grow. | Existing new(resource_type); assign optional public filters. |
| `model::Precondition` | Exhaustive | Closed existential and not-existential filter requirements. | Public MustExist / MustNotExist variants. |
| `model::WriteRequest` | Non-exhaustive (T7.2) | Batch request options may grow. | new(ops) with no preconditions or existing Default; assign public preconditions. |
| `store::StoreError` | Non-exhaustive (existing) | Backend and transaction failure cases may grow. | Public variants; match with wildcard. |
| `store::SubjectRelMatch` | Exhaustive | Closed absent, exact or unrestricted subject-relation matching. | Public None / Exact / Any variants. |
| `store::ReverseFilter` | Non-exhaustive (T7.2) | Reverse-query restrictions may grow. | new(subject_type, subject_id, subject_relation); resource_type/relation default to None. |
| `store::RelationCount` | Non-exhaustive (T7.2) | Store count/group metadata may grow. | new(resource_type, relation, subject_type, subject_relation, wildcard, count). |
| `store::StoredSchema` | Non-exhaustive (T7.2) | Stored-schema result metadata may grow without changing the storage format. | new(version, source, hash, created_rev). |
| `store::GcStats` | Non-exhaustive (T7.2) | Collection outcome counters may grow. | new(rows_removed, horizon, batches). |
| `store::DeleteResult` | Non-exhaustive (T7.2) | Delete outcome metadata may grow. | new(deleted, more, revision). |
| `store::SqlValue` | Exhaustive | Fixed portable SQL subset is null, i64 and text; changing it requires deliberate driver/API revision. | Public Null / Integer / Text variants. |
| `store::RevisionStrategy` | Non-exhaustive (existing) | Other store revision-assignment strategies may be added. | Public variants; match with wildcard. |
| `store::PlaceholderStyle` | Non-exhaustive (existing) | Other SQL driver placeholder conventions may be added. | Public variants; match with wildcard. |
| `write::Applied` | Non-exhaustive (existing) | Shared write outcomes may support further revision strategies. | Public At/Pending variants; produced by shared write functions. |
| `write::Deleted` | Non-exhaustive (existing) | Shared delete outcome metadata may grow. | Produced by write::delete_matching; at()/into_result() convert it. |
| `write::SchemaWritten` | Non-exhaustive (existing) | Shared schema-write outcome metadata may grow. | Produced by write::write_schema; at()/into_stored() convert it. |

### pemba-store-memory

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `MemoryStore` | Exhaustive, opaque | Private store state permits internal field extension. | new() / Default. |

### pemba-sqlite-common

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `Synchronous` | Non-exhaustive (existing) | SQLite durability configuration choices may grow. | Public variants / Default. |
| `SqliteConfig` | Non-exhaustive (existing) | SQLite connection configuration may grow. | Existing Default and with_* setters. |
| `SqliteDialect` | Exhaustive | Fixed stateless SQLite dialect is an intentional unit value. | Unit value or Default. |

### pemba-store-sqlite

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `SqliteStore` | Exhaustive, opaque | Private connection/store state permits internal field extension. | open() / open_read_only(). |
| `SqliteTxn` | Exhaustive, opaque | Private borrowed transaction state permits internal field extension. | new(&transaction). |

### pemba-store-postgres

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `PostgresConfig` | Non-exhaustive (existing) | Postgres connection configuration may grow. | Existing Default and with_* setters. |
| `PostgresStore` | Exhaustive, opaque | Private pool/store state permits internal field extension. | open(). |
| `PostgresDialect` | Exhaustive, opaque | Private validated schema/quoting state permits internal field extension. | new(schema); validation remains required. |
| `PgTxn` | Exhaustive, opaque | Private borrowed transaction state permits internal field extension. | new(&mut transaction). |

### pemba-fixtures

| Public type | Decision | Reason | Construction / production |
|---|---|---|---|
| `OracleError` | Non-exhaustive (existing) | Oracle failure/decline cases may grow. | Public variants; match with wildcard. |
| `StoreKind` | Non-exhaustive (existing) | Fixture runner backends may grow. | Public variants; ALL enumerates currently supported stores. |
| `FixtureError` | Non-exhaustive (existing) | Fixture loading failures may grow. | Public variants; match with wildcard. |
| `Loaded` | Non-exhaustive (existing) | Fixture loading output may grow. | Produced by load(). |
| `Sections` | Non-exhaustive (existing) | Editor-facing fixture sections may grow. | Produced by parse_str(); existing Default for construction. |
| `Failure` | Non-exhaustive (T7.2) | Failure diagnostics may gain metadata. | new(location, assertion, message). |
| `Report` | Non-exhaustive (T7.2) | Fixture run outcomes/counters may grow. | Produced by run functions or existing Default; fields remain writable. |
| `SubjectsAnswer` | Exhaustive | Fixed fixture value represents explicit ids or wildcard minus excluded ids; no fixture format change. | Existing Default, deserialization or public fields. |

### Facade, binary and upstream reexports

| Crate / path | Coverage decision |
|---|---|
| `pemba` | No structs/enums are declared here. `pub use pemba_core::*` exposes all core root types and its public `model`, `store` and `write` modules; all are covered above. |
| `pemba::MemoryStore` | Feature-gated `memory` reexport; same opaque declaration as `pemba-store-memory::MemoryStore`. |
| `pemba::{SqliteConfig, SqliteStore, SqliteTxn, Synchronous}` | Feature-gated `sqlite` reexports; same decisions as the SQLite/common rows above. |
| Postgres facade coverage | Postgres types are audited in `pemba-store-postgres`; there is no direct Postgres reexport in `pemba` and this task does not add one. |
| `pemba-core::{ErrorCode, SchemaError, SchemaErrors}` | Reexport the same non-exhaustive declarations from `pemba-schema`. |
| `pemba-core::model::IdentErrorKind` | Reexports `pemba-schema::ident::IdentErrorKind`; same non-exhaustive declaration. |
| `pemba-schema::ir::ArrowMode` | Reexports the same exhaustive `ast::ArrowMode`. |
| `pemba-store-sqlite::{SqliteConfig, Synchronous}` | Reexports the same non-exhaustive `pemba-sqlite-common` declarations. |
| `pemba-store-sqlite::rusqlite` | Upstream crate reexport; its types and exhaustive policy are dependency-owned, not redeclared or changed by pemba. |
| `pemba-store-postgres::postgres` | Upstream crate reexport; its types and exhaustive policy are dependency-owned, not redeclared or changed by pemba. |
| `pemba-cli` | Publishable binary package; no public struct/enum declarations or library reexports. |

`pemba-py`, `pemba-store-pysqlite`, `pemba-testkit`, `pemba-difftest` and the playground
crates are explicitly unpublished on crates.io. Their consumers are migrated where the
shared Rust construction API changes; they are not silently counted as publishable Rust
surfaces. The Python extension continues to ship via its existing wheel/sdist pipeline.
Traits, functions, aliases and constants are outside T3.9's struct/enum inventory;
this exclusion does not remove any existing API.

## Construction migration

Newly non-exhaustive input structs use these routes:

| Former literal | Replacement |
|---|---|
| `CheckRequest { ... }` | `CheckRequest::new(resource, permission, subject)` defaults to `Consistency::Latest`; assign `request.consistency` for a different revision. |
| `CheckItem { ... }` | `CheckItem::new(resource, permission, subject)`; `check_bulk` supplies consistency. |
| `WriteRequest { ... }` | `WriteRequest::new(ops)` starts with no preconditions; assign `request.preconditions`, or start from `WriteRequest::default()`. |
| `RelationshipFilter { ... }` | Existing `RelationshipFilter::new(resource_type)` followed by optional public field assignments. |
| `ReadRelationshipsRequest { ... }` | Existing `new(filter)` followed by public consistency/limit/cursor assignments. |
| `LookupResourcesRequest { ... }` | Existing `new(resource_type, permission, subject)` followed by public consistency/limit/cursor assignments. |
| `LookupSubjectsRequest { ... }` | Existing `new(resource, permission, subject_type)` followed by public consistency assignment. |
| `ReverseFilter { ... }` | `new(subject_type, subject_id, subject_relation)` followed by optional resource_type/relation restrictions. |
| Store result literals | `RelationCount::new`, `StoredSchema::new`, `GcStats::new` and `DeleteResult::new` take their current public fields, in declaration order. |
| `Failure { ... }` / `Report { ... }` | `Failure::new(location, assertion, message)` / existing `Report::default()` and public field assignments. |
| `EvalStats { ... }` | Existing `EvalStats::default()` and public field assignments. |

Outside the defining crate, struct-update syntax such as `..Default::default()` does not
permit a literal of a non-exhaustive struct. Use the constructor/default as a value, then
assign fields. Destructuring a non-exhaustive output requires `..`. Existing public fields
and their meanings are retained; engine-generated response values remain readable.

## Approved helper and errors

Only the OQ-044-approved Rust helper is added:

```rust,ignore
engine.check_str("document:readme", "view", "user:anne", Consistency::Latest)?;
```

`Engine::check_str(resource, permission, subject, consistency)` parses the three string
components into the existing typed request and returns the ordinary check's `allowed`
boolean. Parse failures remain `Error::Parse` with the original `ParseError`; the stable
machine code is `PARSE`. Denial returns `Ok(false)` and engine/store errors remain errors.
The typed check API and all consistency and fail-closed semantics are preserved.

Unimplemented Rust `write_str`, `delete_str` and static `Engine::validate_schema` helpers
are removed from the promised specification surface, as approved in T7.1. Typed writes,
filter deletion, `pemba_schema::compile`, CLI validation and Python's existing
`Engine.validate_schema` remain the supported routes. The helper is Rust-only: Python's
existing check signatures and stubs do not acquire a new method. Existing Python parse
failures continue to use built-in `ValueError`; exception/parse-code tests cover that
mapping rather than inventing a new Python exception class.

No on-disk schema, stored tuple, revision, fixture or trace format changes are made.
No version, tag, package publication or deployment is changed by this audit.

## Verification evidence

The focused downstream test is `crates/pemba/tests/public_api.rs`; it exercises
published-facing construction and helper behavior from outside the defining crate.
Command results, exact revisions and applicable local/CI/service checks belong in
[RELEASE-0.1.0.md](RELEASE-0.1.0.md). This inventory is a policy and coverage record,
not a claim that verification has passed.
