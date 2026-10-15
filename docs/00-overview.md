# 00 — Overview

## 1. One-line pitch

An embedded, Zanzibar-style authorisation engine in Rust: relationship-based access
control (ReBAC) that runs inside your process and stores its data in your database —
"SQLite for authorisation".

## 2. Problem

Fine-grained authorisation ("can this user view this document, given folders, groups,
orgs and sharing?") is a graph problem. Google's Zanzibar paper (2019) described a general
model for it: relationship tuples + a schema of rewrite rules. Open-source engines that
implement the model (SpiceDB, OpenFGA, Ory Keto, Permify) are deployed as a **separate
service** the application calls over the network.

That brings three costs for small-to-medium applications:

1. **Operational cost.** Another service to deploy, scale, secure and monitor.
2. **Latency.** Every permission check is a network round trip.
3. **Dual writes.** Relationships live in the authz service, application data lives in the
   application database. Creating a document means writing to both; partial failure leaves
   them inconsistent. Teams build outbox patterns and reconciliation jobs to cope.

## 3. Solution

A Rust library that implements the Zanzibar model in-process:

- Python package first (`pip install`), Node/WASM later.
- Relationships stored in tables owned by the engine but living **inside the
  application's own database** (SQLite now, Postgres later), or in memory for tests.
- Writes can participate in the application's **own database transaction**: the app row
  and the relationship tuple commit or roll back together. Dual writes disappear.
- A schema language that is a **strict subset of SpiceDB's**, so SpiceDB users can try it
  and graduate to SpiceDB later if they outgrow it, and so we can differential-test
  against SpiceDB.

## 4. Who it is for

- A single application (monolith or a handful of services sharing one database) that
  needs Google-Docs-style sharing, nested groups, org/team hierarchies.
- Teams that do not want to run an authz service.
- Local-first and edge apps (later, via WASM/SQLite).

Not for: multi-region platforms with hundreds of services and billions of relationships —
that is what SpiceDB is for, and we say so in the README.

## 5. Scope summary

MVP (0.1): schema DSL subset, `check`, `write`, `read_relationships`,
`lookup_resources`, `lookup_subjects`, in-memory + SQLite stores, transactional writes
through the app's SQLite connection, Python bindings, fixture-file test runner, CLI for
schema validation and fixture runs, differential tests against SpiceDB, benchmarks.

Detailed goals/non-goals: `docs/08-milestones.md` and `CLAUDE.md` §6.

## 6. Glossary

| Term | Meaning |
|---|---|
| **Object type** / definition | A class of objects, e.g. `document`. Declared with `definition`. |
| **Object** | An instance: `document:readme` = type `document`, id `readme`. |
| **Relation** | A named, *stored* edge type on an object type, e.g. `viewer`. Tuples can only be written to relations. |
| **Permission** | A named, *computed* set defined by an expression over relations/permissions. Never stored. |
| **Subject** | The "who" side of a tuple: an object (`user:anne`), a subject set (`group:eng#member`) or a wildcard (`user:*`). |
| **Subject set** / userset | All subjects that have relation/permission R on object O, written `O#R`. |
| **Relationship tuple** | `resource#relation@subject`, e.g. `document:readme#viewer@user:anne`. |
| **Wildcard** | `type:*` as a subject: every object of that type. |
| **Arrow** (`->`) | Walk a relation to the related objects, then evaluate a relation/permission on them. Zanzibar's "tuple-to-userset". |
| **IR** | The compiled expression tree for a permission (`docs/03-schema-language.md` §6). |
| **Revision** | Monotonic `u64` identifying a committed write batch. Every read happens at a revision. |
| **Consistency** | How the caller picks the read revision: `Latest`, `AtLeast(r)`, `Exact(r)`. |
| **Check** | Is subject S in the set defined by `resource#permission`? |
| **LookupResources** | All resources of type T for which subject S has permission P. |
| **LookupSubjects** | All subjects of type T that have permission P on resource R. |
| **Store** | Persistence backend implementing the store traits (`docs/05-storage.md`). |
| **Reference evaluator** | A deliberately naive, obviously-correct evaluator used only in tests as an oracle. |
| **Fixture file** | YAML holding schema + relationships + assertions; mirrors SpiceDB validation files. |

## 7. Prior art (design references, not code sources)

- Zanzibar paper: https://research.google/pubs/zanzibar-googles-consistent-global-authorization-system/
- SpiceDB (Apache-2.0): https://github.com/authzed/spicedb — schema language reference:
  https://authzed.com/docs/spicedb/concepts/schema
- OpenFGA: https://openfga.dev
- Ory Keto: https://github.com/ory/keto
- Permify: https://github.com/Permify/permify
