"""Fail unless each given wheel or sdist ships the FastAPI helper, in source form."""

from __future__ import annotations

import sys
import tarfile
import zipfile

REQUIRED = ("pemba/contrib/__init__.py", "pemba/contrib/fastapi.py")


def names(path: str) -> list[str]:
    if path.endswith(".whl"):
        with zipfile.ZipFile(path) as archive:
            return archive.namelist()
    with tarfile.open(path) as archive:
        return archive.getnames()


def main(paths: list[str]) -> int:
    if not paths:
        print("no packages given", file=sys.stderr)
        return 1
    status = 0
    for path in paths:
        found = names(path)
        for required in REQUIRED:
            if not any(name.endswith(required) for name in found):
                print(f"{path}: missing {required}", file=sys.stderr)
                status = 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
