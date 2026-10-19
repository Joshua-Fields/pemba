"""SpiceDB side of the benchmark comparison (``docs/07-testing.md`` §7).

Loads every ``bench-data`` scenario into a local SpiceDB and times the same operations the
warden benches time, over SpiceDB's HTTP gateway on localhost, one call at a time on one
kept-alive connection. Writes one duration (ns) per call to a JSON file that
``benches/results.py`` turns into the SpiceDB table of ``benches/RESULTS.md``.

End to end, with Docker::

    python benches/spicedb/run.py

starts ``authzed/spicedb:<VERSION> serve-testing --http-enabled`` (each bearer token is its
own empty in-memory datastore, so every scenario is isolated), runs, and removes the
container. Against a server you already run (e.g. SpiceDB on Postgres)::

    python benches/spicedb/run.py --url http://127.0.0.1:8443 --key <preshared key>

where every scenario shares one datastore, so it is emptied before each scenario.

Standard library only. Reads are fully consistent by default, which matches warden's
``Consistency::Latest``; ``--consistency minimize_latency`` measures SpiceDB's cached path.
"""

from __future__ import annotations

import argparse
import http.client
import itertools
import json
import platform
import re
import shutil
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
# Same version as .github/workflows/difftest.yml and KNOWN_DIFFERENCES.md.
VERSION = "v1.56.2"
CONTAINER = "warden-bench-spicedb"
TYPES = ("document", "folder", "group", "user")
TUPLE = re.compile(r"^([^:]+):([^#]+)#([^@]+)@([^:#]+):([^#]+)(?:#(.+))?$")
WRITE_CHUNK = 1000  # SpiceDB's default maximum updates per write


def sh(*cmd: str, check: bool = True) -> str:
    return subprocess.run(cmd, check=check, capture_output=True, text=True).stdout.strip()


def obj(ref: str) -> dict[str, str]:
    t, i = ref.split(":", 1)
    return {"objectType": t, "objectId": i}


def subject(ref: str) -> dict[str, Any]:
    o, _, rel = ref.partition("#")
    s: dict[str, Any] = {"object": obj(o)}
    if rel:
        s["optionalRelation"] = rel
    return s


def relationship(t: str) -> dict[str, Any]:
    m = TUPLE.match(t)
    if not m:
        raise ValueError(f"not a relationship: {t}")
    rt, ri, rel, st, si, srel = m.groups()
    return {
        "resource": {"objectType": rt, "objectId": ri},
        "relation": rel,
        "subject": subject(f"{st}:{si}" + (f"#{srel}" if srel else "")),
    }


class SpiceDb:
    def __init__(self, url: str, key: str, isolated: bool, consistency: str) -> None:
        u = urllib.parse.urlsplit(url)
        self.conn = http.client.HTTPConnection(u.hostname or "127.0.0.1", u.port or 8443,
                                               timeout=120)
        self.key = key
        self.isolated = isolated
        self.token = key
        self.consistency: dict[str, Any] = (
            {"fullyConsistent": True} if consistency == "fully_consistent"
            else {"minimizeLatency": True})

    def use(self, scenario: str) -> None:
        """Point at a fresh, empty datastore for ``scenario``."""
        if self.isolated:
            self.token = f"{self.key}-{scenario}"
        else:
            for t in TYPES:
                self.post("/v1/relationships/delete",
                          {"relationshipFilter": {"resourceType": t}})

    def post(self, path: str, body: dict[str, Any]) -> str:
        self.conn.request("POST", path, json.dumps(body), {
            "Authorization": f"Bearer {self.token}",
            "Content-Type": "application/json",
        })
        resp = self.conn.getresponse()
        text = resp.read().decode()
        if resp.status != 200:
            raise RuntimeError(f"POST {path}: HTTP {resp.status}: {text[:500]}")
        return text

    def write_schema(self, schema: str) -> None:
        self.post("/v1/schema/write", {"schema": schema})

    def touch(self, tuples: list[str]) -> None:
        for i in range(0, len(tuples), WRITE_CHUNK):
            self.post("/v1/relationships/write", {"updates": [
                {"operation": "OPERATION_TOUCH", "relationship": relationship(t)}
                for t in tuples[i:i + WRITE_CHUNK]
            ]})

    def check(self, resource: str, permission: str, subj: str) -> bool:
        r = json.loads(self.post("/v1/permissions/check", {
            "consistency": self.consistency,
            "resource": obj(resource),
            "permission": permission,
            "subject": subject(subj),
        }))
        return bool(r["permissionship"] == "PERMISSIONSHIP_HAS_PERMISSION")

    def lookup(self, rtype: str, permission: str, subj: str, page: int | None) -> int:
        found = 0
        cursor: str | None = None
        consistency = self.consistency
        while True:
            body: dict[str, Any] = {
                "consistency": consistency,
                "resourceObjectType": rtype,
                "permission": permission,
                "subject": subject(subj),
            }
            if page:
                body["optionalLimit"] = page
            if cursor:
                body["optionalCursor"] = {"token": cursor}
            # A server-streamed response: one JSON object per line.
            lines = [json.loads(ln) for ln in self.post(
                "/v1/permissions/resources", body).splitlines() if ln.strip()]
            results = [ln["result"] for ln in lines if "result" in ln]
            found += len(results)
            if not page or len(results) < page:
                return found
            cursor = results[-1]["afterResultCursor"]["token"]
            # Later pages read the first page's snapshot, as warden's do.
            consistency = {"atExactSnapshot": results[0]["lookedUpAt"]}


def start_docker(port: int) -> None:
    if not shutil.which("docker"):
        sys.exit("docker not found: start SpiceDB yourself and pass --url and --key")
    sh("docker", "rm", "-f", CONTAINER, check=False)
    sh("docker", "run", "-d", "--name", CONTAINER, "-p", f"{port}:8443",
       f"authzed/spicedb:{VERSION}", "serve-testing", "--http-enabled")
    for _ in range(60):
        try:
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
            c.request("GET", "/healthz")
            if c.getresponse().status == 200:
                return
        except OSError:
            pass
        time.sleep(1)
    print(sh("docker", "logs", CONTAINER, check=False), file=sys.stderr)
    sys.exit("SpiceDB did not become healthy within 60 s")


def load_data(data: Path) -> tuple[str, list[list[str]]]:
    if not (data / "manifest.tsv").exists():
        subprocess.run(["cargo", "run", "--quiet", "-p", "warden-benches", "--bin",
                        "bench-data", "--", str(data)], cwd=REPO, check=True)
    rows = [ln.split("\t") for ln in (data / "manifest.tsv").read_text().splitlines()]
    return (data / "schema.zed").read_text(), rows


def timed(fn: Any, rounds: int, warmup: int) -> list[int]:
    for _ in range(warmup):
        fn()
    out = []
    for _ in range(rounds):
        t0 = time.perf_counter_ns()
        fn()
        out.append(time.perf_counter_ns() - t0)
    return out


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--url", help="an already running SpiceDB HTTP gateway")
    p.add_argument("--key", default="bench", help="preshared key (token prefix)")
    p.add_argument("--port", type=int, default=8443, help="host port for the Docker server")
    p.add_argument("--consistency", choices=["fully_consistent", "minimize_latency"],
                   default="fully_consistent")
    p.add_argument("--rounds", type=int, default=2000, help="timed checks per scenario")
    p.add_argument("--only", help="regex: run only matching scenarios")
    p.add_argument("--data", type=Path, default=REPO / "target" / "bench-data")
    p.add_argument("--out", type=Path, default=REPO / "target" / "bench-spicedb.json")
    p.add_argument("--keep", action="store_true", help="leave the Docker container running")
    a = p.parse_args()

    started = a.url is None
    if started:
        start_docker(a.port)
    url = a.url or f"http://127.0.0.1:{a.port}"
    try:
        schema, rows = load_data(a.data)
        db = SpiceDb(url, a.key, isolated=started, consistency=a.consistency)
        results: dict[str, list[int]] = {}
        writes = itertools.count()
        for name, kind, x, y, z, expect, page in rows:
            if a.only and not re.search(a.only, name):
                continue
            print(f"spicedb: {name}", flush=True)
            db.use(name)
            db.write_schema(schema)
            db.touch((a.data / f"{name}.tuples").read_text().split())
            if kind == "check":
                assert db.check(x, y, z) is (expect == "true"), name
                ns = timed(lambda: db.check(x, y, z), a.rounds, a.rounds // 10)
            elif kind == "lookup":
                size = None if page == "-" else int(page)
                assert db.lookup(x, y, z, size) == int(expect), name
                ns = timed(lambda: db.lookup(x, y, z, size), max(1, a.rounds // 10), 10)
            else:
                size = int(x)
                ns = []
                for _ in range(max(3, a.rounds // max(1, size // 10))):
                    n = next(writes)
                    batch = [f"document:w{n}_{i}#viewer@user:w{i}" for i in range(size)]
                    t0 = time.perf_counter_ns()
                    db.touch(batch)
                    ns.append(time.perf_counter_ns() - t0)
            results[f"spicedb__{name}"] = ns
        meta = {
            "spicedb_version": VERSION if started else "unknown (--url)",
            "server": f"docker serve-testing (memdb), {url}" if started else url,
            "transport": "HTTP gateway, JSON, one kept-alive connection",
            "consistency": a.consistency,
            "client": f"Python {platform.python_version()}",
            "command": " ".join([Path(sys.executable).name, *sys.argv]),
        }
        a.out.parent.mkdir(parents=True, exist_ok=True)
        a.out.write_text(json.dumps({"meta": meta, "results": results}))
        print(f"wrote {a.out}")
    finally:
        if started and not a.keep:
            sh("docker", "rm", "-f", CONTAINER, check=False)


if __name__ == "__main__":
    main()
