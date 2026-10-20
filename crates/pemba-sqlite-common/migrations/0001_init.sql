-- Storage version 1 (docs/05-storage.md §4). Run by the migration runner inside one
-- transaction; the runner records storage_version itself.

CREATE TABLE IF NOT EXISTS pemba_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO pemba_meta (key, value) VALUES
    ('gc_horizon', '0'),
    ('created_at', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE IF NOT EXISTS pemba_revisions (
    rev          INTEGER PRIMARY KEY AUTOINCREMENT,
    committed_at TEXT NOT NULL,
    kind         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pemba_relationships (
    resource_type    TEXT    NOT NULL,
    resource_id      TEXT    NOT NULL,
    relation         TEXT    NOT NULL,
    subject_type     TEXT    NOT NULL,
    subject_id       TEXT    NOT NULL,
    subject_relation TEXT    NOT NULL DEFAULT '',
    created_rev      INTEGER NOT NULL,
    deleted_rev      INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS pemba_rel_live
    ON pemba_relationships (resource_type, resource_id, relation,
                             subject_type, subject_id, subject_relation)
    WHERE deleted_rev IS NULL;

CREATE INDEX IF NOT EXISTS pemba_rel_fwd
    ON pemba_relationships (resource_type, resource_id, relation,
                             subject_type, subject_id, subject_relation,
                             created_rev, deleted_rev);

CREATE INDEX IF NOT EXISTS pemba_rel_rev
    ON pemba_relationships (subject_type, subject_id, subject_relation,
                             resource_type, relation, created_rev);

CREATE INDEX IF NOT EXISTS pemba_rel_deleted
    ON pemba_relationships (deleted_rev) WHERE deleted_rev IS NOT NULL;

CREATE TABLE IF NOT EXISTS pemba_schema (
    version     INTEGER PRIMARY KEY AUTOINCREMENT,
    source      TEXT    NOT NULL,
    hash        TEXT    NOT NULL,
    created_rev INTEGER NOT NULL
);
