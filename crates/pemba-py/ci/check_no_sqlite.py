"""Fails if the built extension contains a SQLite library (ADR-006, OQ-032).

pemba-py must share CPython's ``sqlite3``: a second SQLite copy in the process silently
breaks locks on database files the app also opens. A bundled SQLite is not visible in the
extension's exported symbols (a cdylib exports only the module init), so scan the bytes
for markers every SQLite build contains: the database header magic, which is data and
survives stripping, and the API names in the symbol table and error strings.

Usage: ``python ci/check_no_sqlite.py [path/to/extension]`` (default: the installed
``pemba._pemba``). Exit 1 if any marker is found.
"""

from __future__ import annotations

import sys
from pathlib import Path

MARKERS = [b"SQLite format 3\x00", b"sqlite3_open", b"sqlite3_prepare_v2"]


def main() -> int:
    if len(sys.argv) > 1:
        path = Path(sys.argv[1])
    else:
        import pemba._pemba

        path = Path(pemba._pemba.__file__)
    data = path.read_bytes()
    found = [m.decode(errors="replace") for m in MARKERS if m in data]
    if found:
        print(f"FAIL: {path} contains SQLite ({', '.join(map(repr, found))})")
        return 1
    print(f"ok: {path} ({len(data):,} bytes) contains no SQLite")
    return 0


if __name__ == "__main__":
    sys.exit(main())
