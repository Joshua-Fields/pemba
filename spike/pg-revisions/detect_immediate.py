"""Can the commit trigger tell that it fired before COMMIT (SET CONSTRAINTS ... IMMEDIATE)?
Records current_query() inside the batchmap trigger for each way it can fire."""
import json
import psycopg
import spike
from spike import DSN, ddl, write

S = "batchmap"
with psycopg.connect(DSN, autocommit=True) as c:
    c.execute(ddl(S))
    c.execute(f"CREATE TABLE {S}.fired (batch_id bigint, q text)")
    c.execute(f"""CREATE OR REPLACE FUNCTION {S}.assign_rev() RETURNS trigger LANGUAGE plpgsql
        SET search_path = pg_catalog, pg_temp AS $$
    BEGIN
        PERFORM pg_advisory_xact_lock(70006, 0);
        INSERT INTO {S}.revisions (batch_id, rev) VALUES (NEW.batch_id, nextval('{S}.rev_seq'));
        INSERT INTO {S}.fired VALUES (NEW.batch_id, current_query());
        RETURN NULL;
    END $$""")
out = {}
with psycopg.connect(DSN) as c:
    c.execute("SELECT 1"); b = write(S, c, [("d:1", "u")], []); c.commit()
    out["at_commit"] = b
    c.execute("SELECT 1"); b = write(S, c, [("d:2", "u")], []); c.execute("SET CONSTRAINTS ALL IMMEDIATE"); c.commit()
    out["set_constraints_after_write"] = b
    c.execute("SELECT 1"); c.execute("SET CONSTRAINTS ALL IMMEDIATE"); b = write(S, c, [("d:3", "u")], []); c.commit()
    out["set_constraints_before_write"] = b
    c.execute("SELECT 1"); b = write(S, c, [("d:4", "u")], []); c.execute(f"SET CONSTRAINTS {S}.assign_rev IMMEDIATE"); c.commit()
    out["set_constraints_named"] = b
with psycopg.connect(DSN, autocommit=True) as c:
    c.execute("BEGIN"); b = write(S, c, [("d:5", "u")], []); c.execute("END")
    out["end_statement"] = b
    q = dict(c.execute(f"SELECT batch_id, q FROM {S}.fired").fetchall())
print(json.dumps({k: q.get(v) for k, v in out.items()}))
