#!/usr/bin/env bash
# pm-web full release verification. Runs the native + wasm builds, the workspace
# tests, TypeScript, the production web build, and the headed-Chrome Playwright
# regression. Fails fast with a clear message on the first failing stage.
#
# Headed WebGPU Playwright needs a real GPU + a running preview server, so it is
# a LOCAL release gate (see docs/deployment.md). Skip it in headless CI with
#   SKIP_BROWSER=1 bash scripts/release-check.sh
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
PORT="${PMW_PORT:-5174}"

step() { echo; echo "==== $* ===="; }
fail() { echo "RELEASE-CHECK FAILED: $*" >&2; exit 1; }

step "cargo build (wasm32: pm-web-vj + pm-web-player)"
cargo build --target wasm32-unknown-unknown -p pm-web-vj || fail "wasm32 build (pm-web-vj)"
cargo build --target wasm32-unknown-unknown -p pm-web-player || fail "wasm32 build (pm-web-player)"

step "cargo build --workspace"
cargo build --workspace || fail "workspace build"

step "cargo test --workspace"
cargo test --workspace || fail "workspace tests"

step "TypeScript type check"
( cd web && npx tsc --noEmit ) || fail "tsc"

step "production build (wasm-pack release + vite build)"
( cd web && npm run build ) || fail "npm run build"

if [ "${SKIP_BROWSER:-0}" = "1" ]; then
  echo; echo "SKIP_BROWSER=1 — skipping headed Playwright regression."
  echo "RELEASE-CHECK OK (browser stage skipped)."
  exit 0
fi

step "headed-Chrome Playwright regression (preview on :$PORT)"
( cd web && npx vite preview --port "$PORT" --strictPort >/tmp/pmw-preview.log 2>&1 & echo $! >/tmp/pmw-preview.pid )
sleep 4
set +e
( cd web && PMW_URL="http://localhost:$PORT/" node verify.mjs )
RC=$?
set -e
kill "$(cat /tmp/pmw-preview.pid 2>/dev/null)" 2>/dev/null || true
[ "$RC" -eq 0 ] || fail "Playwright regression (exit $RC)"

echo; echo "RELEASE-CHECK OK."
