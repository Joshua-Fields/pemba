"""Runs the store conformance suite against the option A prototype (OQ-032)."""

import sys
import tempfile

import warden_pystore_prototype as proto

with tempfile.TemporaryDirectory() as d:
    failure = proto.run_conformance(d)
print("conformance: PASS" if failure is None else f"conformance: FAIL\n{failure}")
sys.exit(0 if failure is None else 1)
