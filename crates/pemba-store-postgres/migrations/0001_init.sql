-- Storage version 1 of the Postgres store (docs/10-postgres.md §3). The runner sends this
-- script whole, inside one transaction, after rendering two placeholders:
--   {s}      the store's schema as a quoted identifier, e.g. "app"
--   {s_lit}  the same, with ' doubled, for use inside a string literal
-- The runner records storage_version itself. Every function and operator call is
-- qualified with pg_catalog, so a search_path that lists another schema first cannot
-- redirect it.

CREATE TABLE {s}.pemba_meta (
    key   text COLLATE "C" PRIMARY KEY,
    value text NOT NULL
);

INSERT INTO {s}.pemba_meta (key, value) VALUES
    ('gc_horizon', '0'),
    ('created_at', pg_catalog.to_char(pg_catalog.now() AT TIME ZONE 'UTC',
                                      'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'));

-- Batch ids carry no ordering meaning; revisions are drawn only by the commit trigger.
CREATE SEQUENCE {s}.pemba_batch_seq;
CREATE SEQUENCE {s}.pemba_rev_seq;

CREATE TABLE {s}.pemba_revisions (
    batch_id     bigint      PRIMARY KEY,
    rev          bigint      NOT NULL UNIQUE,
    kind         text        NOT NULL
                 CHECK (kind IN ('base', 'write', 'schema', 'delete_filter')),
    committed_at timestamptz NOT NULL,
    early        boolean     NOT NULL DEFAULT false
);

-- Batch 0 is revision 0: the state before any write, and the target of the GC rebase.
INSERT INTO {s}.pemba_revisions (batch_id, rev, kind, committed_at, early)
    VALUES (0, 0, 'base', pg_catalog.now(), false);

CREATE TABLE {s}.pemba_pending (
    batch_id bigint PRIMARY KEY,
    kind     text   NOT NULL,
    xid      xid8   NOT NULL
);

CREATE TABLE {s}.pemba_relationships (
    resource_type    text COLLATE "C" NOT NULL,
    resource_id      text COLLATE "C" NOT NULL,
    relation         text COLLATE "C" NOT NULL,
    subject_type     text COLLATE "C" NOT NULL,
    subject_id       text COLLATE "C" NOT NULL,
    subject_relation text COLLATE "C" NOT NULL DEFAULT '',
    created_batch    bigint NOT NULL,
    deleted_batch    bigint
);

CREATE UNIQUE INDEX pemba_rel_live
    ON {s}.pemba_relationships (resource_type, resource_id, relation,
                                subject_type, subject_id, subject_relation)
    WHERE deleted_batch IS NULL;

CREATE INDEX pemba_rel_fwd
    ON {s}.pemba_relationships (resource_type, resource_id, relation,
                                subject_type, subject_id, subject_relation,
                                created_batch, deleted_batch);

CREATE INDEX pemba_rel_rev
    ON {s}.pemba_relationships (subject_type, subject_id, subject_relation,
                                resource_type, relation, created_batch, deleted_batch);

CREATE INDEX pemba_rel_deleted
    ON {s}.pemba_relationships (deleted_batch) WHERE deleted_batch IS NOT NULL;

CREATE TABLE {s}.pemba_schema (
    version       bigint PRIMARY KEY,
    source        text   NOT NULL,
    hash          text   NOT NULL,
    created_batch bigint NOT NULL
);

-- Maps one committed batch to the next revision (docs/10-postgres.md §1.6.1). Runs as the
-- committing role; names are baked in, so renaming the schema needs the function
-- regenerated. The lock key's second half is this store's pemba_revisions oid, looked up
-- at run time, so it survives dump/restore and differs between stores in one database.
CREATE FUNCTION {s}.pemba_assign_revision() RETURNS trigger
    LANGUAGE plpgsql
    SECURITY INVOKER
    SET search_path = pg_catalog, pg_temp
AS $pemba_fn$
DECLARE
    q     text := pg_catalog.current_query();
    stmt  text;
    early boolean;
BEGIN
    -- At COMMIT (or END, or PREPARE TRANSACTION) current_query() is that statement. Any
    -- other statement means SET CONSTRAINTS ... IMMEDIATE fired the trigger early
    -- (§1.6.7). A simple query holding several statements shows them all, so look at the
    -- last one.
    stmt := pg_catalog.lower(pg_catalog.btrim(q, E' \t\r\n;'));
    IF stmt !~ '^(commit|end|prepare\s+transaction)(\s|$)' THEN
        stmt := pg_catalog.regexp_replace(stmt, '^.*;\s*', '');
    END IF;
    early := stmt !~ '^(commit|end|prepare\s+transaction)(\s|$)';
    IF early THEN
        RAISE WARNING 'pemba: revision assigned before COMMIT by "%"; the store-wide revision lock is held until this transaction ends', q;
    END IF;
    PERFORM pg_catalog.pg_advisory_xact_lock(
        1885695330,  -- PEMBA_LOCK_CLASS, 0x70656D62 ("pemb")
        '{s_lit}.pemba_revisions'::pg_catalog.regclass::pg_catalog.oid::pg_catalog.int4);
    INSERT INTO {s}.pemba_revisions (batch_id, rev, kind, committed_at, early)
        VALUES (NEW.batch_id,
                pg_catalog.nextval('{s_lit}.pemba_rev_seq'::pg_catalog.regclass),
                NEW.kind, pg_catalog.clock_timestamp(), early);
    RETURN NULL;
END;
$pemba_fn$;

CREATE CONSTRAINT TRIGGER pemba_assign_revision
    AFTER INSERT ON {s}.pemba_pending
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION {s}.pemba_assign_revision();
