# Schema and relationship guide

Start with the complete [README schema](../../README.md#schema-guide).
A `definition` names an object type; a `relation` declares stored edges and their allowed
subject types; a `permission` computes a subject set. Write relationships such as
`document:readme#viewer@user:anne`, then check the computed `view` permission.
Use Touch for idempotent grants, Create to reject an existing live tuple, and Delete
for removal. Permissions are computed; write tuples to relations.

Supported syntax includes direct subjects (`user`), subject sets (`group#member`),
wildcards (`user:*`), union `+`, intersection `&`, exclusion `-`, and arrows
`parent->view`, `parent.any(view)` and `parent.all(view)`. Type prefixes such as
`tenant/document` and line/block/doc comments are supported. Parenthesise mixed
operators: `a + b & c` is rejected with `E_AMBIGUOUS_OPERATORS`, rather than applying
SpiceDB's union precedence. Arrows cannot chain or walk a wildcard-allowing relation.

Caveats/CEL, expiration, `use` directives, `self`, `nil`, permission type annotations
and functions other than `.any` / `.all` are rejected as unsupported features.
There is no SpiceDB wire API, Watch/changefeed or service mode.

Data cycles are evaluated to a fixed point where defined; some cycles have no defined
answer and fail closed. SpiceDB instead reports a depth error for cyclic data.
A wildcard matches objects, never subject sets. A wildcard on the subtracted side of
an exclusion that could match a subject set produces `AMBIGUOUS_WILDCARD_EXCLUSION`.
Some wildcard schemas accepted here are rejected by SpiceDB. Consult
[known differences and repros](../../crates/pemba-difftest/KNOWN_DIFFERENCES.md)
before migrating a model to or from SpiceDB; compatibility is not universal.

Validate before deploying with `pemba schema validate path/to/schema.zed`, format with
`pemba schema fmt path/to/schema.zed`, or use Python `Engine.validate_schema`.
Rust can validate without a store using `pemba_schema::compile`.
`write_schema` validates and rejects changes that orphan live relationships, returning
an orphan report (`SchemaChangeOrphansError` in Python). Remove affected relationships
first; do not edit internal tables to bypass validation.

Schema versions are not a historical authorisation log: an exact-revision check uses
**today's schema** over historical relationships. Plan schema rollout alongside app
changes and retention; see [consistency](consistency.md).
For concrete models, use the [agent example](../../examples/agent_authz.py) and
[fixture collection](../../testdata/fixtures/). The
[browser playground](https://joshua-fields.github.io/pemba/) uses the real engine and a
memory store: it helps explore models and explanations, but cannot demonstrate a real
database transaction. [Limits and troubleshooting](troubleshooting.md) explain failures.
