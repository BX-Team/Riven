#!/usr/bin/env bash
# Builds the browser demo into crates/riven-demo/dist: index.html, riven_demo.js and the wasm.
# Needs a nightly toolchain with the wasm32-unknown-unknown target (gpui's web backend
# uses unstable wasm features) and wasm-bindgen-cli at the version in Cargo.lock.
set -euo pipefail
cd "$(dirname "$0")"
root=../..
out=dist

cargo build -p riven-demo --lib --target wasm32-unknown-unknown --release \
  --target-dir "$root/target/demo-web"
rm -rf "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  "$root/target/demo-web/wasm32-unknown-unknown/release/riven_demo.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-threads \
    "$out/riven_demo_bg.wasm" -o "$out/riven_demo_bg.wasm"
fi
cp web/index.html "$out/"
