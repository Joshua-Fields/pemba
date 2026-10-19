"""Writes the seed corpora under ``fuzz/corpus/`` (``docs/07-testing.md`` §5).

- ``fuzz_schema_parse``: every ``testdata/schemas/*.zed`` and every fixture's ``schema:``
  block, valid and invalid (``schema_errors/``).
- ``fuzz_tuple_parse``: every fixture relationship and assertion, plus edge cases.
- ``fuzz_check_vs_reference``: fixed pseudo-random byte strings (any bytes are a valid
  case; the first byte picks the generator variant).

Idempotent: files are named by content hash, as libFuzzer names the inputs it adds.
Standard library only. Usage: ``python fuzz/seed_corpus.py``
"""

from __future__ import annotations

import hashlib
import random
import re
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CORPUS = REPO / "fuzz" / "corpus"

TUPLE_EDGE_CASES = [
    "document:readme#viewer@user:anne",
    "document:readme#viewer@user:*",
    "document:readme#viewer@group:eng#member",
    "tenant/document:a-b_c#viewer@tenant/user:x|y",
    "document:readme#viewer@user:anne#",
    "document:#viewer@user:anne",
    "document:readme#@user:anne",
    ":readme#viewer@user:anne",
    "document:readme#viewer@user:*#member",
    "document:réadme#viewer@user:anne",
    "document:readme#viewer@@user:anne",
    "document:readme##viewer@user:anne",
    "",
]


def block(text: str, key: str) -> str | None:
    """The ``key: |`` / ``key: |-`` block scalar at the top level of a fixture."""
    m = re.search(rf"^{key}: \|-?\n((?:  .*\n|\n)+)", text, re.M)
    if not m:
        return None
    return "\n".join(ln[2:] for ln in m[1].splitlines()).strip("\n") + "\n"


def put(target: str, data: bytes) -> None:
    d = CORPUS / target
    d.mkdir(parents=True, exist_ok=True)
    (d / hashlib.sha1(data).hexdigest()).write_bytes(data)


def main() -> None:
    fixtures = sorted((REPO / "testdata" / "fixtures").rglob("*.yaml"))
    for f in sorted((REPO / "testdata" / "schemas").glob("*.zed")):
        put("fuzz_schema_parse", f.read_bytes())
    for f in fixtures:
        text = f.read_text(encoding="utf-8")
        schema = block(text, "schema")
        if schema:
            put("fuzz_schema_parse", schema.encode())
        rels = block(text, "relationships") or ""
        tuples = [ln.split("//")[0].strip() for ln in rels.splitlines()]
        tuples += re.findall(r"^\s+- \"?([^\s\"#]+#[^\s\"@]+@[^\s\"]+)\"?", text, re.M)
        for t in tuples:
            if t:
                put("fuzz_tuple_parse", t.encode())
    for t in TUPLE_EDGE_CASES:
        put("fuzz_tuple_parse", t.encode())
    rng = random.Random(0x5EED)
    for i in range(32):
        n = rng.choice([16, 64, 256, 1024, 4096])
        put("fuzz_check_vs_reference", bytes([i % 2]) + rng.randbytes(n))
    for t in sorted(CORPUS.iterdir()):
        print(f"{t.name}: {sum(1 for _ in t.iterdir())} files")


if __name__ == "__main__":
    main()
