# Draft upstream issue: nondeterministic schema validation ("wildcard relations cannot be transitively included")

Status: **draft for a human to file** at https://github.com/authzed/spicedb/issues. Not
filed by the agent. Found by pemba's differential test (T3.4, KD-2 in
`crates/pemba-difftest/KNOWN_DIFFERENCES.md`).

---

**Title:** WriteSchema nondeterministically accepts a schema that includes a wildcard
transitively

**SpiceDB version:** 1.56.2 (`spicedb_1.56.2_linux_amd64` release binary, and the
`authzed/spicedb:v1.56.2` image), datastore `memory`.

**What happens:** writing the same schema repeatedly sometimes succeeds and sometimes
fails with

```
for relation `rel0`: relation/permission `ty3#rel0` includes wildcard type `ty0` via
relation `ty3#rel0`: wildcard relations cannot be transitively included
```

Measured on 2026-10-16: accepted 28 of 200 writes on `serve-testing` (fresh datastore per
write) and 10 of 100 writes on `serve` (the same datastore each time).

**Expected:** the same answer every time. Given the error, that answer is presumably to
refuse: `ty0#rel0` allows `ty3#rel0`, which allows `ty0:*`.

**Schema** (minimised from a generated one; every line is needed for the flakiness):

```
definition ty0 {
    relation rel0: ty3#rel0
    relation rel1: ty2#rel1
}

definition ty1 {
    relation rel0: ty0
}

definition ty2 {
    relation rel1: ty1#rel0
}

definition ty3 {
    relation rel0: ty0:*
}
```

Without the line `relation rel1: ty2#rel1` in `ty0`, the schema was refused in 100 of 100
writes. That relation adds a path `ty0#rel1 → ty2#rel1 → ty1#rel0 → ty0`, back to the
type that holds the offending relation. A guess, not verified against the source: the
transitive-wildcard check walks the type graph from a starting point that depends on map
iteration order. When the walk reaches `ty0` through that path first, it marks `ty0`
visited and never checks `ty0#rel0`.

**Reproduce** (bash, curl, the release binary or the image):

```bash
# Terminal 1
spicedb serve --grpc-preshared-key somekey --http-enabled
# or: docker run --rm -p 8443:8443 authzed/spicedb:v1.56.2 serve --grpc-preshared-key somekey --http-enabled

# Terminal 2
SCHEMA='definition ty0 {\n    relation rel0: ty3#rel0\n    relation rel1: ty2#rel1\n}\n\ndefinition ty1 {\n    relation rel0: ty0\n}\n\ndefinition ty2 {\n    relation rel1: ty1#rel0\n}\n\ndefinition ty3 {\n    relation rel0: ty0:*\n}\n'
ok=0; refused=0
for i in $(seq 1 100); do
  r=$(curl -s -X POST http://127.0.0.1:8443/v1/schema/write \
        -H 'Authorization: Bearer somekey' \
        -d "{\"schema\": \"$SCHEMA\"}")
  case "$r" in
    *writtenAt*) ok=$((ok+1)) ;;
    *transitively*) refused=$((refused+1)) ;;
    *) echo "unexpected: $r" ;;
  esac
done
echo "accepted $ok, refused $refused"
```

Typical output: `accepted 10, refused 90`. Any run with both counts above zero shows the
problem.

**Impact:** a schema can be written once and then fail to write again unchanged, for
example on a redeploy or a migration replay. A client can also hold relationships under
a schema the server would usually refuse.
