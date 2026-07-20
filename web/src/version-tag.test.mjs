// Tests for the release-tag parser + display-version resolution.
// Run: node --test web/src/version-tag.test.mjs   (no deps)
//
// Imports the compiled-away TS via a tiny re-implementation guard: since this is
// a .mjs test and version-tag.ts is TS, we import the source through the same
// pure logic by requiring a JS mirror. To keep it dependency-free we duplicate
// the two small regexes here and assert parity of behaviour would be brittle —
// instead we import the TS directly using Node's type-stripping (Node 22.6+),
// falling back to skip if unavailable.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseReleaseTag, resolveDisplayVersion } from './version-tag.ts';

// --- canonical beta ---
test('parses v0.10.0-beta.1 → 0.10.0-beta.1', () => {
  const p = parseReleaseTag('v0.10.0-beta.1');
  assert.equal(p?.version, '0.10.0-beta.1');
  assert.equal(p?.base, '0.10.0');
  assert.equal(p?.kind, 'beta');
  assert.equal(p?.legacy, false);
  assert.equal(p?.beta, 1);
});

// --- canonical stable ---
test('parses v0.10.0 → 0.10.0 (stable)', () => {
  const p = parseReleaseTag('v0.10.0');
  assert.equal(p?.version, '0.10.0');
  assert.equal(p?.kind, 'stable');
  assert.equal(p?.legacy, false);
  assert.equal(p?.beta, undefined);
});

// --- legacy line preserved verbatim, NOT reinterpreted ---
test('legacy v0.0.3-web-beta.4 stays historical (not 0.10.0-beta.4)', () => {
  const p = parseReleaseTag('v0.0.3-web-beta.4');
  assert.equal(p?.version, '0.0.3-web-beta.4');
  assert.equal(p?.base, '0.0.3');
  assert.equal(p?.kind, 'legacy');
  assert.equal(p?.legacy, true);
  assert.equal(p?.beta, 4);
});

// --- malformed tags are rejected (not silently accepted) ---
test('malformed tags → null', () => {
  for (const bad of ['', '0.10.0', 'v0.10', 'v0.10.0-beta', 'v0.10.0-rc.1', 'vX.Y.Z', 'release-1']) {
    assert.equal(parseReleaseTag(bad), null, `expected null for ${bad}`);
  }
});

// --- dev build never fabricates a beta ---
test('dev build resolves to <base>-dev', () => {
  const r = resolveDisplayVersion({ releaseTag: '', productBase: '0.10.0', isDev: true });
  assert.equal(r.version, '0.10.0-dev');
  assert.equal(r.kind, 'dev');
});

test('tagged build resolves to the release version', () => {
  assert.equal(resolveDisplayVersion({ releaseTag: 'v0.10.0-beta.1', productBase: '0.10.0', isDev: false }).version, '0.10.0-beta.1');
  assert.equal(resolveDisplayVersion({ releaseTag: 'v0.10.0', productBase: '0.10.0', isDev: false }).version, '0.10.0');
  // a malformed tag falls back to dev honestly rather than pretending
  assert.equal(resolveDisplayVersion({ releaseTag: 'garbage', productBase: '0.10.0', isDev: false }).version, '0.10.0-dev');
});
