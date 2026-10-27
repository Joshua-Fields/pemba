"""Fails unless the built extension's Postgres TLS is rustls, with no OpenSSL inside
(``docs/10-postgres.md`` §2.1, T5.10).

Wheels must not depend on a system OpenSSL (manylinux images and user machines differ), and
must still speak TLS to Postgres. A statically linked OpenSSL is not visible in the
extension's exported symbols, so, as ``check_no_sqlite.py`` does, scan the bytes: OpenSSL's
API names and library names must be absent, and rustls (with the Postgres connector over it)
present.

Usage: ``python ci/check_tls.py [path/to/extension]`` (default: the installed
``pemba._pemba``). Exit 1 on failure.
"""

from __future__ import annotations

import sys
from pathlib import Path

FORBIDDEN = [b"OPENSSL_init_ssl", b"SSL_CTX_new", b"libssl", b"libcrypto"]
REQUIRED = [b"rustls", b"tokio-postgres-rustls"]


def main() -> int:
    if len(sys.argv) > 1:
        path = Path(sys.argv[1])
    else:
        import pemba._pemba

        path = Path(pemba._pemba.__file__)
    data = path.read_bytes()
    found = [m.decode() for m in FORBIDDEN if m in data]
    missing = [m.decode() for m in REQUIRED if m not in data]
    if found:
        print(f"FAIL: {path} contains OpenSSL ({', '.join(map(repr, found))})")
    if missing:
        print(f"FAIL: {path} lacks rustls ({', '.join(map(repr, missing))}): no Postgres TLS")
    if found or missing:
        return 1
    print(f"ok: {path} ({len(data):,} bytes) has rustls and no OpenSSL")
    return 0


if __name__ == "__main__":
    sys.exit(main())
