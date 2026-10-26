#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== 1. npm install ==="
npm ci

echo "=== 2. Build Tauri app with CLI ==="
npm run tauri -- build --target aarch64-apple-darwin

echo "=== Done ==="
ls -lh target/aarch64-apple-darwin/release/bundle/macos/*.app
ls -lh target/aarch64-apple-darwin/release/bundle/dmg/*.dmg 2>/dev/null || true
