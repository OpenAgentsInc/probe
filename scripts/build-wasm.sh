#!/usr/bin/env bash
# Build the probe wasm core and generate the @openagentsinc/probe bindings.
# Uses wasm-bindgen directly (no wasm-pack dependency). Output lands in
# packages/probe/wasm/ and is consumed by packages/probe/src.
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${1:-release}"
TARGET_DIR="target/wasm32-unknown-unknown/${PROFILE}"
OUT="packages/probe/wasm"

if [ "$PROFILE" = "release" ]; then
  cargo build -p probe-wasm --target wasm32-unknown-unknown --release
else
  cargo build -p probe-wasm --target wasm32-unknown-unknown
fi

rm -rf "$OUT"
mkdir -p "$OUT"
# --target nodejs emits a CommonJS + require() loader that Node and Bun both
# run with no flags; the async ABI never needs JSPI.
wasm-bindgen "${TARGET_DIR}/probe_wasm.wasm" \
  --out-dir "$OUT" \
  --target nodejs \
  --omit-default-module-path

# The nodejs bindings are CommonJS; under the package's type:module, rename
# to .cjs so require() loads them correctly on Node and Bun.
mv "${OUT}/probe_wasm.js" "${OUT}/probe_wasm.cjs"

# wasm-opt if available (size discipline: opt-level z is set in Cargo).
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz "${OUT}/probe_wasm_bg.wasm" -o "${OUT}/probe_wasm_bg.wasm"
fi

echo "wasm bindings written to ${OUT}"
ls -la "${OUT}"
