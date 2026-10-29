# Revisions, consistency and history

Each read resolves one snapshot and reports its revision. Revisions are local to a store;
do not use a token from another database/store as a global clock.

| Python | Rust | Contract |
|---|---|---|
| default | `Consistency::Latest` | newest committed revision |
| `at_least=rev` | `Consistency::AtLeast(rev)` | newest revision, error if below the bound |
| `at_exactly=rev` | `Consistency::Exact(rev)` | exact revision, error if future or expired |

`AtLeast` is a bound check, not an indefinite wait for commit. A native write commits
before returning `Revision`. A foreign write returns `PendingRevision`; commit first,
then resolve it or pass it as Python `at_least=pending`. Rust calls
`engine.resolve(&pending)` and uses `Consistency::AtLeast(rev)`. All reads use engine
connections; there is no read-your-uncommitted-write promise. On SQLite the pending
number is already known, but it is unreadable before commit. After rollback it is void
and may be reused: resolution cannot detect SQLite rollback. On Postgres resolution
raises `RevisionPending` before commit and `WriteRolledBack` after rollback.

Postgres allocates revisions in commit order; failed commits can leave gaps. Reading an
exact skipped number sees the state at the preceding revision. One write call gets one
revision. Multiple write calls in one transaction get consecutive revisions; an exact
read at an intermediate revision may show only part of that transaction. Put related
updates in one write call when historical atomicity matters (within the batch limit).

For `lookup_resources`, pin later pages to the first page's revision using `at_exactly`.
Python `iter_resources` does this for you. Finish on a null/absent cursor (including an
empty final page). Do not switch snapshots between pages. GC may expire a cursor's
revision; handle that error and restart the traversal deliberately.

`gc(keep_from)` advances the retention horizon and removes older history. Reads below
the horizon fail with `ConsistencyError` / `REVISION_EXPIRED`. Schedule GC to suit your
retention policy; pemba does not retain history forever automatically once you run it.
On Postgres GC uses short transactions; autovacuum handles dead rows. After a first
large backlog, a DBA may run `VACUUM (ANALYZE)` on the relationship table. Long-running
transactions can hold vacuum back.

**Exact is not an audit log:** the current schema is applied to the selected historical
relationships. A schema edit may change an old revision's answer. Retaining relationship
history alone does not preserve the old authorisation decision.

## Runnable verification

This complete example exercises denial, an invalid permission and expired history;
errors remain distinct from a successful denial:

```python
from pemba import Engine, PembaError, ConsistencyError

engine = Engine.memory()
engine.write_schema("definition user {} definition document { relation viewer: user permission view = viewer }")
first = engine.write(touch=["document:readme#viewer@user:anne"])
assert engine.check("document:readme#view@user:anne", at_least=first)
assert not engine.check("document:readme#view@user:bob")
try:
    engine.check("document:readme#missing@user:anne")
except PembaError:
    pass  # An evaluation/request error; do not treat it as an ordinary denial.
else:
    raise AssertionError("unknown permission must fail closed")
second = engine.write(delete=["document:readme#viewer@user:anne"])
assert engine.check("document:readme#view@user:anne", at_exactly=first)
assert not engine.check("document:readme#view@user:anne", at_least=second)
engine.gc(second)
try:
    engine.check("document:readme#view@user:anne", at_exactly=first)
except ConsistencyError as error:
    assert error.code == "REVISION_EXPIRED"
else:
    raise AssertionError("GC must expire the old revision")
```

See [transactions](transactions.md) for commit ownership and retry rules, and
[troubleshooting](troubleshooting.md) for resource bounds and error handling.
