#!/usr/bin/env bash
# Builds the engine for the page (docs/11-playground.md §1.3): the `wasm-release` profile,
# then wasm-bindgen's `web` glue into src/wasm/ (ignored by git). Run from anywhere; needs
# the wasm32-unknown-unknown target and the wasm-bindgen CLI at Cargo.lock's version.
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$root"
cargo build --locked --quiet --profile wasm-release --target wasm32-unknown-unknown -p pemba-playground
wasm-bindgen --target web --out-dir playground/web/src/wasm \
  target/wasm32-unknown-unknown/wasm-release/pemba_playground.wasm
