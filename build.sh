#!/bin/sh
# Builds the site into dist/ on Linux, macOS or CI. Run from the project root:  sh build.sh
# Needs: Rust with the wasm32-unknown-unknown target, and wasm-bindgen-cli
# at the same version as the wasm-bindgen crate in Cargo.lock.
set -eu
cd "$(dirname "$0")"

cargo build -p sixtyfour-web --release --target wasm32-unknown-unknown

mkdir -p dist
wasm-bindgen --target web --no-typescript --out-name app --out-dir dist/pkg \
    target/wasm32-unknown-unknown/release/sixtyfour_web.wasm

cp -R static/. dist/
echo 'Built dist/. Serve it with:  cargo run --release -p sixtyfour-server'
