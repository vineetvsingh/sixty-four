# Builds the site into dist/. Run from the project root:  .\build.ps1
# Needs: Rust with the wasm32-unknown-unknown target, and wasm-bindgen-cli
# at the same version as the wasm-bindgen crate in Cargo.lock.
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (Test-Path "$env:USERPROFILE\.cargo\bin") { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

cargo build -p sixtyfour-web --release --target wasm32-unknown-unknown
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

New-Item -ItemType Directory -Force dist | Out-Null
wasm-bindgen --target web --no-typescript --out-name app --out-dir dist/pkg `
    target/wasm32-unknown-unknown/release/sixtyfour_web.wasm
if ($LASTEXITCODE -ne 0) { throw 'wasm-bindgen failed' }

Copy-Item static\* dist\ -Recurse -Force
Write-Host 'Built dist/. Serve it with:  cargo run --release -p sixtyfour-server'
