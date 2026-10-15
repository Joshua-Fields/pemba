"""Usage sample that exercises every public method; ``mypy --strict`` must pass on it.

Also run by ``test_typing_sample.py``, so it stays correct at runtime too.
"""

from __future__ import annotations

import tempfile
from pathlib import Path

import warden
from warden import Engine, Precondition, Relationship, Revision

SCHEMA = """\
definition user {}
definition document {
    relation viewer: user | user:*
    relation banned: user
    permission view = viewer - banned
}
"""


def main() -> None:
    diagnostics: list[warden.Diagnostic] = Engine.validate_schema(SCHEMA)
    assert diagnostics == []

    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "authz.db"
        disk: Engine = Engine.sqlite(
            path, reader_pool=2, busy_timeout_ms=1000, auto_migrate=True, max_depth=20
        )
        disk.write_schema(SCHEMA)
        del disk

    engine = Engine.memory(max_depth=50)
    version: int = engine.write_schema(SCHEMA)
    source: str | None = engine.read_schema()
    assert version == 1 and source == SCHEMA

    rel = Relationship("document", "readme", "viewer", "user", "anne")
    rev: Revision = engine.write(
        touch=["document:readme#viewer@user:bob"],
        create=[rel, Relationship.parse("document:spec#viewer@user:anne")],
        delete=(),
        preconditions=[Precondition.must_not_exist(resource_type="document", relation="banned")],
    )
    pre = Precondition.must_exist(resource_type="document", resource_id="readme", subject_relation="")
    engine.write(touch=["document:old#viewer@user:anne"], preconditions=[pre])

    allowed: bool = engine.check("document:readme", "view", "user:anne", at_least=rev)
    one_string: bool = engine.check("document:readme#view@user:bob", at_exactly=rev)
    assert allowed and one_string

    detailed: warden.CheckResult = engine.check_detailed("document:readme", "view", "user:anne")
    reads: int = detailed.store_reads
    assert detailed.allowed and reads >= 1

    bulk: list[bool | warden.WardenError] = engine.check_bulk(
        [("document:readme", "view", "user:anne"), ("document:readme", "nope", "user:anne")]
    )
    for item in bulk:
        if isinstance(item, warden.WardenError):
            code: str = item.code
            assert code == "INVALID_REQUEST"

    page: warden.LookupResult = engine.lookup_resources("document", "view", "user:anne", limit=1)
    ids: list[str] = list(page.ids)
    while page.cursor is not None:
        page = engine.lookup_resources(
            "document", "view", "user:anne", limit=1, cursor=page.cursor, at_exactly=page.revision
        )
        ids += page.ids
    assert ids == list(engine.iter_resources("document", "view", "user:anne"))

    subjects: warden.SubjectsResult = engine.lookup_subjects("document:readme", "view", "user")
    assert "anne" in subjects and not subjects.wildcard and subjects.excluded == []

    rels: list[Relationship] = engine.read_relationships(resource_type="document", subject_id="anne")
    assert {r.resource_id for r in rels} == {"readme", "spec", "old"}

    deleted: warden.DeleteResult = engine.delete_relationships(resource_type="document", resource_id="old")
    assert deleted.deleted == 1 and not deleted.more

    head: Revision = engine.head_revision()
    stats: warden.GcStats = engine.gc(head)
    assert stats.horizon == head

    try:
        engine.write_schema("definition document { relation reader: nope }")
    except warden.SchemaError as e:
        assert [d.code for d in e.diagnostics] == ["E_UNKNOWN_TYPE"]
    try:
        engine.write_schema("definition user {}")
    except warden.SchemaChangeOrphansError as e:
        counts: dict[str, int] = {o.relation: o.count for o in e.orphans}
        assert counts
    except warden.StorageError as e:
        retry: bool = e.retryable or isinstance(e, warden.TransactionConflictError)
        assert not retry


if __name__ == "__main__":
    main()
