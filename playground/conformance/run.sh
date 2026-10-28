#!/usr/bin/env bash
# The playground conformance check (docs/11-playground.md §6 layers 1-2, T6.3), from the
# repository root:
#   1. generate 1 000 cases as fixtures, each checked against the reference evaluator;
#   2. write the transcript of them plus testdata/fixtures natively;
#   3. build the release .wasm, write the same transcript from it under Node;
#   4. fail on any byte of difference.
# Needs: the wasm32-unknown-unknown target, wasm-bindgen (the CLI at Cargo.lock's
# version) and Node. CI: .github/workflows/playground.yml.
#
# Negative control (local only):
#   CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="--cfg pemba_conformance_sabotage" makes the
# .wasm drop one id from multi-id lookup_resources answers; this script must then fail.
set -euo pipefail

out=target/conformance
rm -rf "$out"
mkdir -p "$out"

tool() { cargo run --locked --quiet --release -p pemba-playground-conformance -- "$@"; }
tool generate "$out/generated"
tool list "$out/manifest.txt" testdata/fixtures "$out/generated"
tool transcript "$out/manifest.txt" "$out/native.txt"

cargo build --locked --quiet --profile wasm-release --target wasm32-unknown-unknown -p pemba-playground
wasm-bindgen --target nodejs --out-dir "$out/pkg" \
  target/wasm32-unknown-unknown/wasm-release/pemba_playground.wasm
node playground/conformance/transcript.mjs "$out/pkg" "$out/manifest.txt" "$out/wasm.txt"

if cmp -s "$out/native.txt" "$out/wasm.txt"; then
  echo "native and WASM transcripts are identical ($(wc -l < "$out/native.txt") lines)"
else
  echo "native and WASM transcripts differ:" >&2
  diff "$out/native.txt" "$out/wasm.txt" | cut -c1-300 | head -40 >&2 || true
  exit 1
fi
