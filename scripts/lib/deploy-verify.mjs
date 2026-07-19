// Pure verification/parsing helpers for the Cloudflare Pages production deploy
// script (scripts/deploy-cloudflare-pages.sh). Kept dependency-free and pure so
// the provenance/asset/header logic can be unit-tested with `node --test`
// (see deploy-verify.test.mjs) without touching git, the network, or Wrangler.
//
// Also usable as a small CLI so the bash orchestrator can call one subcommand
// per check:
//   node deploy-verify.mjs extract-asset      <index.html>
//   node deploy-verify.mjs assert-asset        <expected> <actual>
//   node deploy-verify.mjs assert-branch       <branch>
//   node deploy-verify.mjs verify-headers      <headers-file>
//   node deploy-verify.mjs parse-deploy-branch <pages-api.json>
// Each exits 0 on success and non-zero (with a message on stderr) on failure.

import { readFileSync } from 'node:fs';

export const PRODUCTION_BRANCH = 'main';
export const REQUIRED_HEADERS = {
  'cross-origin-opener-policy': 'same-origin',
  'cross-origin-embedder-policy': 'require-corp',
};

/**
 * Extract the hashed main application asset (e.g. `main-AbCd1234.js`) that a
 * built index.html references. Matches both `/assets/main-*.js` and
 * `assets/main-*.js`. Throws if no such reference is present.
 */
export function extractMainAsset(html) {
  if (typeof html !== 'string') throw new TypeError('html must be a string');
  const m = html.match(/assets\/(main-[A-Za-z0-9_-]+\.js)/);
  if (!m) throw new Error('no hashed main asset (assets/main-<hash>.js) found in HTML');
  return m[1];
}

/**
 * Cloudflare Pages only updates the production alias for deployments on the
 * project's production branch. Fail loudly for anything else so a preview
 * deployment is never mistaken for a production release.
 */
export function assertProductionBranch(branch) {
  if (branch !== PRODUCTION_BRANCH) {
    throw new Error(
      `deployment branch is '${branch}', expected '${PRODUCTION_BRANCH}' — ` +
        `this is a PREVIEW deployment, not a production release`,
    );
  }
  return true;
}

/**
 * The canonical production alias must serve the exact artifact we built. A
 * matching HTTP 200 or matching headers are NOT sufficient — the hashed asset
 * must match byte-for-name.
 */
export function assertAssetMatch(expected, actual) {
  if (!expected) throw new Error('expected asset is empty');
  if (expected !== actual) {
    throw new Error(
      `production asset mismatch: expected '${expected}', production serves '${actual}' ` +
        `(edge cache may be stale, or the deploy did not update the production alias)`,
    );
  }
  return true;
}

/**
 * Verify COOP/COEP are present with the exact values cross-origin isolation
 * (SharedArrayBuffer audio) requires. `headerText` is raw header output such as
 * from `curl -sI`. Case-insensitive on names; value-exact.
 */
export function assertCoopCoep(headerText) {
  if (typeof headerText !== 'string') throw new TypeError('headerText must be a string');
  const seen = {};
  for (const line of headerText.split(/\r?\n/)) {
    const idx = line.indexOf(':');
    if (idx === -1) continue;
    const name = line.slice(0, idx).trim().toLowerCase();
    const value = line.slice(idx + 1).trim().toLowerCase();
    if (name in REQUIRED_HEADERS) seen[name] = value;
  }
  const missing = [];
  for (const [name, want] of Object.entries(REQUIRED_HEADERS)) {
    if (seen[name] !== want) missing.push(`${name}: ${want} (got: ${seen[name] ?? 'absent'})`);
  }
  if (missing.length) throw new Error('required headers missing/incorrect — ' + missing.join('; '));
  return true;
}

/**
 * Classify a resolved release reference. `refObject` is `git rev-parse <ref>`
 * (the annotated tag object for annotated tags); `commitTarget` is
 * `git rev-parse <ref>^{commit}`. Provenance always uses the dereferenced
 * commit target, never the tag object.
 */
export function resolveProvenance(refObject, commitTarget) {
  if (!refObject || !commitTarget) throw new Error('both refObject and commitTarget are required');
  const annotated = refObject !== commitTarget;
  return { tagObject: refObject, commitTarget, annotated, provenanceCommit: commitTarget };
}

/**
 * Extract the branch a Cloudflare Pages deployment was created on, from either a
 * project response (`result.latest_deployment` / `result.canonical_deployment`)
 * or a bare deployment object. Throws if no branch can be found.
 */
export function parseDeploymentBranch(apiResponse) {
  const obj = typeof apiResponse === 'string' ? JSON.parse(apiResponse) : apiResponse;
  const candidates = [
    obj?.result?.latest_deployment,
    obj?.result?.canonical_deployment,
    obj?.result,
    obj,
  ];
  for (const d of candidates) {
    const branch = d?.deployment_trigger?.metadata?.branch;
    if (typeof branch === 'string' && branch.length) return branch;
  }
  throw new Error('could not find deployment_trigger.metadata.branch in API response');
}

// --- CLI dispatch -----------------------------------------------------------

function main(argv) {
  const [cmd, ...rest] = argv;
  try {
    switch (cmd) {
      case 'extract-asset':
        process.stdout.write(extractMainAsset(readFileSync(rest[0], 'utf8')) + '\n');
        return 0;
      case 'assert-asset':
        assertAssetMatch(rest[0], rest[1]);
        return 0;
      case 'assert-branch':
        assertProductionBranch(rest[0]);
        return 0;
      case 'verify-headers':
        assertCoopCoep(readFileSync(rest[0], 'utf8'));
        return 0;
      case 'parse-deploy-branch':
        process.stdout.write(parseDeploymentBranch(readFileSync(rest[0], 'utf8')) + '\n');
        return 0;
      default:
        process.stderr.write(`unknown subcommand: ${cmd ?? '(none)'}\n`);
        return 2;
    }
  } catch (e) {
    process.stderr.write(`deploy-verify ${cmd}: ${e.message}\n`);
    return 1;
  }
}

// Only run the CLI when executed directly (not when imported by the test file).
if (import.meta.url === `file://${process.argv[1]}` || process.argv[1]?.endsWith('deploy-verify.mjs')) {
  process.exit(main(process.argv.slice(2)));
}
