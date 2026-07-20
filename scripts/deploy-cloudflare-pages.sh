#!/usr/bin/env bash
# Deterministic Cloudflare Pages PRODUCTION deploy for the pm-web beta site.
#
# Motivation (see memory/cloudflare-pages-prod-deploy.md): a successful Wrangler
# upload does NOT prove the production alias was updated. Deploying from a
# detached-HEAD tag checkout makes Wrangler label the deployment branch `head`,
# creating a PREVIEW deployment that never touches the production alias — the
# per-deployment URL serves the new build while production keeps the old one.
#
# This script makes production deploys deterministic:
#   1. builds from an EXACT immutable release ref (tag or commit),
#   2. verifies the release ref exists and resolves the dereferenced commit,
#   3. builds in an isolated temporary git worktree (your checkout is untouched),
#   4. records the exact hashed main asset the build produced,
#   5. writes/verifies the COOP/COEP `_headers` in the artifact,
#   6. deploys EXPLICITLY to the production branch (--branch main),
#   7. verifies Cloudflare reports the deployment on branch `main` (not preview),
#   8. polls the canonical production alias until it serves the expected asset,
#   9. verifies COOP/COEP on the live production alias.
# Any provenance check that does not match fails loudly.
#
# Usage:
#   scripts/deploy-cloudflare-pages.sh <tag-or-commit> [--dry-run|--verify-only]
#
#   --dry-run      resolve + build + detect asset + generate/verify _headers,
#                  then STOP before any Cloudflare operation. Publishes nothing.
#   --verify-only  build the release, then verify the ALREADY-LIVE production
#                  alias serves exactly that build (asset match + COOP/COEP).
#                  Publishes nothing; needs no credentials (reads the public
#                  alias). Use it to confirm what is currently in production.
#
# Credentials (real deploy only) come from the environment Wrangler expects:
#   CLOUDFLARE_API_TOKEN   (Pages:Edit)     — never printed
#   CLOUDFLARE_ACCOUNT_ID
# If either is absent the script stops without deploying. Tokens are never
# accepted as CLI args, echoed, or written to the repo.
set -euo pipefail

PROJECT="projectm-rs-web-beta"
PROD_BRANCH="main"
PROD_URL="https://projectm-rs-web-beta.pages.dev"
POLL_ATTEMPTS="${DEPLOY_POLL_ATTEMPTS:-10}"
POLL_INTERVAL="${DEPLOY_POLL_INTERVAL:-12}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB="$ROOT/scripts/lib/deploy-verify.mjs"

usage() {
  sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'
}
log()  { echo; echo "==== $* ===="; }
fail() { echo "DEPLOY FAILED: $*" >&2; exit 1; }

# Verify the LIVE canonical production alias serves the exact build we produced
# (bounded edge-cache poll — the retry semantics are unit-tested via
# pollForAssetMatch in scripts/lib/deploy-verify.test.mjs) with the required
# COOP/COEP headers. Shared by the real deploy path and --verify-only. Reads the
# public alias only; requires $EXPECTED_ASSET to be set.
verify_production() {
  log "verify canonical production alias ($PROD_URL)"
  node "$LIB" poll-asset "$PROD_URL/" "$EXPECTED_ASSET" "$POLL_ATTEMPTS" "$((POLL_INTERVAL * 1000))" \
    || fail "production alias never served $EXPECTED_ASSET within $POLL_ATTEMPTS attempts (stale edge cache, or production was not updated)"
  log "verify production headers"
  local hdrs; hdrs="$(mktemp)"
  curl -sI "$PROD_URL/" >"$hdrs"
  node "$LIB" verify-headers "$hdrs" || { rm -f "$hdrs"; fail "production alias missing/incorrect COOP/COEP"; }
  rm -f "$hdrs"
  echo "production alias verified: serving $EXPECTED_ASSET with COOP/COEP"
}

# --- arg parsing ---
RELEASE=""
DRY_RUN=0
VERIFY_ONLY=0
for a in "$@"; do
  case "$a" in
    --dry-run) DRY_RUN=1 ;;
    --verify-only) VERIFY_ONLY=1 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "unknown option: $a" >&2; exit 2 ;;
    *) if [ -z "$RELEASE" ]; then RELEASE="$a"; else echo "unexpected extra arg: $a" >&2; exit 2; fi ;;
  esac
done
[ -n "$RELEASE" ] || { echo "usage: scripts/deploy-cloudflare-pages.sh <tag-or-commit> [--dry-run|--verify-only]" >&2; exit 2; }
[ "$DRY_RUN" = "1" ] && [ "$VERIFY_ONLY" = "1" ] && { echo "--dry-run and --verify-only are mutually exclusive" >&2; exit 2; }

# --- 1/2. resolve + verify release provenance (never create/move tags) ---
log "resolve release provenance"
git -C "$ROOT" rev-parse --verify --quiet "${RELEASE}^{commit}" >/dev/null \
  || fail "release ref '$RELEASE' does not resolve to a commit (it must already exist)"
REF_OBJ="$(git -C "$ROOT" rev-parse "$RELEASE")"
COMMIT="$(git -C "$ROOT" rev-parse "${RELEASE}^{commit}")"
echo "release ref     : $RELEASE"
echo "tag-object SHA  : $REF_OBJ"
echo "commit target   : $COMMIT"
if [ "$REF_OBJ" != "$COMMIT" ]; then
  echo "annotated tag detected → using dereferenced commit target for provenance"
fi

# --- 3. isolated temporary worktree at the exact commit ---
log "checkout release commit in an isolated worktree"
WT_PARENT="$(mktemp -d)"
WORKTREE="$WT_PARENT/wt"
cleanup() {
  git -C "$ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1 || true
  rm -rf "$WT_PARENT" >/dev/null 2>&1 || true
}
trap cleanup EXIT
git -C "$ROOT" worktree add --detach "$WORKTREE" "$COMMIT" >/dev/null
WT_HEAD="$(git -C "$WORKTREE" rev-parse HEAD)"
echo "worktree HEAD   : $WT_HEAD"
[ "$WT_HEAD" = "$COMMIT" ] || fail "worktree HEAD ($WT_HEAD) != release commit ($COMMIT)"

# --- 4a. clean production build (embed the EXACT release identity) ---
# The build receives the requested release tag + its dereferenced commit — NOT
# current main — so the artifact's About panel shows the release it actually is.
# For a bare commit ref (no tag), APP_RELEASE_TAG stays empty → the app honestly
# shows "<base>-dev". Legacy tags (v0.0.3-web-beta.N) are passed verbatim and the
# app parses them on their legacy path.
export APP_GIT_COMMIT="$COMMIT"
if [ "$REF_OBJ" != "$COMMIT" ] || printf '%s' "$RELEASE" | grep -qE '^v[0-9]'; then
  export APP_RELEASE_TAG="$RELEASE"
else
  export APP_RELEASE_TAG=""
fi
log "clean build (npm ci + npm run build) — release '${APP_RELEASE_TAG:-<none/dev>}' @ $COMMIT"
( cd "$WORKTREE/web" && APP_RELEASE_TAG="$APP_RELEASE_TAG" APP_GIT_COMMIT="$APP_GIT_COMMIT" npm ci && \
    APP_RELEASE_TAG="$APP_RELEASE_TAG" APP_GIT_COMMIT="$APP_GIT_COMMIT" npm run build ) || fail "production build failed"
DIST="$WORKTREE/web/dist"
[ -f "$DIST/index.html" ] || fail "no dist/index.html produced"

# --- 4b. detect the hashed main asset this build produced ---
log "detect build artifact identity"
EXPECTED_ASSET="$(node "$LIB" extract-asset "$DIST/index.html")" || fail "could not detect hashed main asset"
WASM_ASSET="$(ls "$DIST/assets" 2>/dev/null | grep -E '^pm_web_bg-.*\.wasm$' | head -1 || true)"
OUTPUT_HTML="$([ -f "$DIST/output.html" ] && echo output.html || echo '<none>')"
echo "expected main   : $EXPECTED_ASSET"
echo "wasm asset      : ${WASM_ASSET:-<none>}"
echo "primary html    : index.html   output page: $OUTPUT_HTML"

# --- 5. write + verify deployment-only COOP/COEP headers ---
log "generate + verify _headers"
printf '/*\n  Cross-Origin-Opener-Policy: same-origin\n  Cross-Origin-Embedder-Policy: require-corp\n' > "$DIST/_headers"
[ -f "$DIST/_headers" ] || fail "_headers not created"
node "$LIB" verify-headers "$DIST/_headers" || fail "generated _headers is missing COOP/COEP"
echo "_headers present with COOP/COEP"

# --- dry run stops here (no deploy, no live verification) ---
if [ "$DRY_RUN" = "1" ]; then
  log "DRY RUN OK"
  echo "release          : $RELEASE ($COMMIT)"
  echo "expected asset   : $EXPECTED_ASSET"
  echo "artifact         : $DIST (with verified _headers)"
  echo "intended project : $PROJECT   intended branch: $PROD_BRANCH"
  echo "NOT published — skipped Wrangler deploy + production alias verification."
  exit 0
fi

# --- verify-only: compare the ALREADY-LIVE production alias to this build ---
# No deploy, no credentials — reads the public alias. Confirms what is currently
# in production actually matches the given release.
if [ "$VERIFY_ONLY" = "1" ]; then
  verify_production
  log "VERIFY-ONLY OK"
  echo "release        : $RELEASE ($COMMIT)"
  echo "production url  : $PROD_URL"
  echo "serving asset  : $EXPECTED_ASSET (matches this release build)"
  echo "NOT deployed — verification only."
  exit 0
fi

# --- 9. credential safety (real deploy only) ---
log "check credentials"
[ -n "${CLOUDFLARE_API_TOKEN:-}" ]  || fail "CLOUDFLARE_API_TOKEN is not set — refusing to deploy (see docs/deployment.md)"
[ -n "${CLOUDFLARE_ACCOUNT_ID:-}" ] || fail "CLOUDFLARE_ACCOUNT_ID is not set — refusing to deploy"
echo "credentials present (values not shown)"

# --- 6/8. explicit production-branch deploy ---
log "wrangler pages deploy --branch $PROD_BRANCH"
DEPLOY_LOG="$(mktemp)"
set +e
( cd "$DIST/.." && CLOUDFLARE_API_TOKEN="$CLOUDFLARE_API_TOKEN" CLOUDFLARE_ACCOUNT_ID="$CLOUDFLARE_ACCOUNT_ID" \
    npx wrangler pages deploy dist --project-name "$PROJECT" --branch "$PROD_BRANCH" --commit-dirty=true ) >"$DEPLOY_LOG" 2>&1
RC=$?
set -e
# Redacted echo — drop any line that could contain a token/credential header.
grep -viE 'cfut_|Bearer|Authorization|CLOUDFLARE_API_TOKEN' "$DEPLOY_LOG" || true
rm -f "$DEPLOY_LOG"
[ "$RC" -eq 0 ] || fail "wrangler deploy exited $RC"

# --- 10. verify Cloudflare reports this as a PRODUCTION (branch=main) deploy ---
log "verify deployment branch via Cloudflare API"
PROJ_JSON="$(mktemp)"
curl -s -H "Authorization: Bearer ${CLOUDFLARE_API_TOKEN}" \
  "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/pages/projects/${PROJECT}" >"$PROJ_JSON"
DEPLOY_ID="$(node -e 'const o=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));process.stdout.write((o.result&&o.result.latest_deployment&&o.result.latest_deployment.id)||"unknown")' "$PROJ_JSON" 2>/dev/null || echo unknown)"
DEPLOY_BRANCH="$(node "$LIB" parse-deploy-branch "$PROJ_JSON")" || { rm -f "$PROJ_JSON"; fail "could not read deployment branch from API"; }
rm -f "$PROJ_JSON"
echo "deployment id     : $DEPLOY_ID"
echo "deployment branch : $DEPLOY_BRANCH"
node "$LIB" assert-branch "$DEPLOY_BRANCH" \
  || fail "deployment is on branch '$DEPLOY_BRANCH', not production '$PROD_BRANCH' — this is a PREVIEW, not a production release"

# --- 11/12. verify the canonical alias serves the EXACT asset + COOP/COEP ---
verify_production

log "PRODUCTION DEPLOY VERIFIED"
echo "release        : $RELEASE ($COMMIT)"
echo "deployment id  : $DEPLOY_ID (branch $DEPLOY_BRANCH)"
echo "production url  : $PROD_URL"
echo "serving asset  : $EXPECTED_ASSET"
echo "COOP/COEP      : verified"
