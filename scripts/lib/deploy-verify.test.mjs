// Tests for the Cloudflare Pages deploy verification helpers.
// Run: node --test scripts/lib/deploy-verify.test.mjs   (no external deps)

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  extractMainAsset,
  assertProductionBranch,
  assertAssetMatch,
  assertCoopCoep,
  resolveProvenance,
  parseDeploymentBranch,
} from './deploy-verify.mjs';

// --- hashed asset detection ---
test('extractMainAsset: pulls main-<hash>.js from a script tag', () => {
  const html = '<script type="module" crossorigin src="/assets/main-AbCd1234.js"></script>';
  assert.equal(extractMainAsset(html), 'main-AbCd1234.js');
});

test('extractMainAsset: works without a leading slash', () => {
  assert.equal(extractMainAsset('src="assets/main-XyZ9_-01.js"'), 'main-XyZ9_-01.js');
});

test('extractMainAsset: ignores other hashed assets and picks main', () => {
  const html =
    '<link href="/assets/shader-console-BT7Gv.js"><script src="/assets/main-KfDzo0mk.js"></script>';
  assert.equal(extractMainAsset(html), 'main-KfDzo0mk.js');
});

test('extractMainAsset: throws when no main asset present', () => {
  assert.throws(() => extractMainAsset('<html>no assets here</html>'), /no hashed main asset/);
});

// --- annotated tag: tag object != commit target, provenance uses commit ---
test('resolveProvenance: annotated tag uses dereferenced commit target', () => {
  const p = resolveProvenance('3b6c9cftagobject0000000000000000000000', 'dd14614commit00000000000000000000000000');
  assert.equal(p.annotated, true);
  assert.equal(p.tagObject, '3b6c9cftagobject0000000000000000000000');
  assert.equal(p.commitTarget, 'dd14614commit00000000000000000000000000');
  assert.equal(p.provenanceCommit, p.commitTarget);
  assert.notEqual(p.provenanceCommit, p.tagObject);
});

test('resolveProvenance: lightweight tag / commit (object == target) is not annotated', () => {
  const sha = 'dd14614commit00000000000000000000000000';
  const p = resolveProvenance(sha, sha);
  assert.equal(p.annotated, false);
  assert.equal(p.provenanceCommit, sha);
});

// --- production asset mismatch fails / match passes ---
test('assertAssetMatch: mismatch throws (stale/preview production)', () => {
  assert.throws(() => assertAssetMatch('main-NewHash.js', 'main-OldHash.js'), /mismatch/);
});

test('assertAssetMatch: exact match passes', () => {
  assert.equal(assertAssetMatch('main-KfDzo0mk.js', 'main-KfDzo0mk.js'), true);
});

// --- COOP/COEP verification ---
test('assertCoopCoep: passes when both headers correct', () => {
  const headers = [
    'HTTP/1.1 200 OK',
    'Cross-Origin-Opener-Policy: same-origin',
    'Cross-Origin-Embedder-Policy: require-corp',
    'Content-Type: text/html',
  ].join('\r\n');
  assert.equal(assertCoopCoep(headers), true);
});

test('assertCoopCoep: missing COEP fails', () => {
  const headers = ['HTTP/1.1 200 OK', 'Cross-Origin-Opener-Policy: same-origin'].join('\r\n');
  assert.throws(() => assertCoopCoep(headers), /embedder-policy/);
});

test('assertCoopCoep: wrong COOP value fails', () => {
  const headers = [
    'Cross-Origin-Opener-Policy: unsafe-none',
    'Cross-Origin-Embedder-Policy: require-corp',
  ].join('\n');
  assert.throws(() => assertCoopCoep(headers), /opener-policy/);
});

// --- preview branch result must not be reported as production ---
test('assertProductionBranch: main passes', () => {
  assert.equal(assertProductionBranch('main'), true);
});

test('assertProductionBranch: preview branch "head" fails', () => {
  assert.throws(() => assertProductionBranch('head'), /PREVIEW deployment/);
});

test('assertProductionBranch: empty/undefined fails', () => {
  assert.throws(() => assertProductionBranch(undefined), /PREVIEW deployment/);
});

// --- deployment branch parsing from Pages API shapes ---
test('parseDeploymentBranch: reads latest_deployment branch', () => {
  const api = { result: { latest_deployment: { deployment_trigger: { metadata: { branch: 'main' } } } } };
  assert.equal(parseDeploymentBranch(api), 'main');
});

test('parseDeploymentBranch: reads a bare deployment object (preview)', () => {
  const dep = { deployment_trigger: { metadata: { branch: 'head' } } };
  assert.equal(parseDeploymentBranch(dep), 'head');
});

test('parseDeploymentBranch: accepts a JSON string', () => {
  const s = JSON.stringify({ result: { canonical_deployment: { deployment_trigger: { metadata: { branch: 'main' } } } } });
  assert.equal(parseDeploymentBranch(s), 'main');
});

test('parseDeploymentBranch: throws when branch absent', () => {
  assert.throws(() => parseDeploymentBranch({ result: {} }), /could not find/);
});
