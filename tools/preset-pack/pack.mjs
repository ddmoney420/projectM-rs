#!/usr/bin/env node
// pack.mjs — pack .milk preset packs into sharded, gzipped bundles + a manifest.
//
// Output layout (suitable for copying to vibrdrome-web/public/presets/):
//   out/presets/manifest.json          — index of shards + totals + per-preset list
//   out/presets/<category>.ndjson.gz   — one shard per top-level category
//
// Each shard is gzipped NDJSON: one JSON object per line,
//   {"name": "...", "path": "<pack>/<relpath>", "text": "<full .milk source>"}
// so a consumer can stream-decompress and store presets one at a time.
//
// Usage:
//   node pack.mjs [--out DIR] [--pack name=DIR ...] [--verify-only]
// Defaults to the two official packs at their conventional clone locations.

import { promises as fs } from 'node:fs';
import { createReadStream } from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import zlib from 'node:zlib';

const HOME = os.homedir();

// --- args ---
const args = process.argv.slice(2);
let outDir = path.join(path.dirname(new URL(import.meta.url).pathname), 'out');
let verifyOnly = false;
const packs = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--out') outDir = path.resolve(args[++i]);
  else if (args[i] === '--verify-only') verifyOnly = true;
  else if (args[i] === '--pack') {
    const [name, dir] = args[++i].split('=');
    packs.push({ name, dir: path.resolve(dir) });
  }
}
if (packs.length === 0) {
  packs.push({ name: 'cream-of-the-crop', dir: path.join(HOME, 'milk-presets') });
  packs.push({ name: 'milkdrop-original', dir: path.join(HOME, 'milkdrop-original-presets') });
}

const presetsDir = path.join(outDir, 'presets');
const manifestPath = path.join(presetsDir, 'manifest.json');
const safe = (s) => s.replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^_+|_+$/g, '') || 'Uncategorized';

// --- walk a directory for *.milk files ---
async function walk(dir) {
  const out = [];
  async function rec(d) {
    let entries;
    try { entries = await fs.readdir(d, { withFileTypes: true }); } catch { return; }
    for (const e of entries) {
      const full = path.join(d, e.name);
      if (e.isDirectory()) await rec(full);
      else if (e.isFile() && e.name.toLowerCase().endsWith('.milk')) out.push(full);
    }
  }
  await rec(dir);
  return out;
}

async function build() {
  // category -> array of {name, path, text}
  const byCategory = new Map();
  let totalPresets = 0;
  let totalRawBytes = 0;

  for (const pack of packs) {
    const files = await walk(pack.dir);
    if (files.length === 0) {
      console.warn(`  ! no .milk files under ${pack.dir} (pack "${pack.name}") — skipped`);
      continue;
    }
    for (const file of files) {
      const rel = path.relative(pack.dir, file);
      const segs = rel.split(path.sep);
      // category = first path segment under the pack root, else the pack name.
      const category = segs.length > 1 ? segs[0] : pack.name;
      const text = await fs.readFile(file, 'utf8');
      const name = path.basename(file, path.extname(file));
      const entry = { name, path: `${pack.name}/${rel}`, text };
      if (!byCategory.has(category)) byCategory.set(category, []);
      byCategory.get(category).push(entry);
      totalPresets++;
      totalRawBytes += Buffer.byteLength(text, 'utf8');
    }
    console.log(`  ${pack.name}: ${files.length} presets from ${pack.dir}`);
  }

  await fs.mkdir(presetsDir, { recursive: true });

  // Write one gzipped NDJSON shard per category.
  const shards = [];
  const index = []; // flat {name, path, category, shard} for browsing without downloads
  for (const [category, entries] of [...byCategory].sort((a, b) => a[0].localeCompare(b[0]))) {
    entries.sort((a, b) => a.name.localeCompare(b.name));
    const file = `${safe(category)}.ndjson.gz`;
    const ndjson = entries.map((e) => JSON.stringify(e)).join('\n') + '\n';
    const gz = zlib.gzipSync(Buffer.from(ndjson, 'utf8'), { level: 9 });
    await fs.writeFile(path.join(presetsDir, file), gz);
    const rawBytes = Buffer.byteLength(ndjson, 'utf8');
    shards.push({ file, category, count: entries.length, rawBytes, gzBytes: gz.length });
    for (const e of entries) index.push({ name: e.name, path: e.path, category, shard: file });
    console.log(`  shard ${file}: ${entries.length} presets, ${(rawBytes / 1048576).toFixed(1)} MB → ${(gz.length / 1048576).toFixed(2)} MB gz`);
  }

  const manifest = {
    version: 1,
    sources: packs.map((p) => p.name),
    totalPresets,
    totalRawBytes,
    totalGzBytes: shards.reduce((s, x) => s + x.gzBytes, 0),
    shards,
    presets: index, // flat index for the app to list/search before fetching shards
  };
  await fs.writeFile(manifestPath, JSON.stringify(manifest, null, 0));

  console.log(`\n  manifest: ${manifest.totalPresets} presets, ${shards.length} shards`);
  console.log(`  raw ${(totalRawBytes / 1048576).toFixed(1)} MB → gz ${(manifest.totalGzBytes / 1048576).toFixed(1)} MB (manifest.json ${(Buffer.byteLength(JSON.stringify(manifest)) / 1048576).toFixed(1)} MB)`);
  return manifest;
}

// --- verify: re-read every shard, decompress, count lines, compare to manifest ---
async function verify() {
  const manifest = JSON.parse(await fs.readFile(manifestPath, 'utf8'));
  let counted = 0;
  let sampleOk = 0, sampleBad = 0;
  for (const shard of manifest.shards) {
    const gz = await fs.readFile(path.join(presetsDir, shard.file));
    const text = zlib.gunzipSync(gz).toString('utf8');
    const lines = text.split('\n').filter((l) => l.length > 0);
    if (lines.length !== shard.count) {
      console.error(`  ✗ ${shard.file}: manifest says ${shard.count}, decompressed ${lines.length}`);
    }
    counted += lines.length;
    // sample-check the first preset of each shard round-trips to non-empty .milk
    try {
      const first = JSON.parse(lines[0]);
      if (first.text && first.text.length > 0) sampleOk++; else sampleBad++;
    } catch { sampleBad++; }
  }
  const ok = counted === manifest.totalPresets && sampleBad === 0;
  console.log(`  verify: decompressed ${counted}/${manifest.totalPresets} presets across ${manifest.shards.length} shards; sample-parse ok=${sampleOk} bad=${sampleBad}`);
  console.log(ok ? '  ✓ VERIFY PASSED' : '  ✗ VERIFY FAILED');
  return ok;
}

if (verifyOnly) {
  const ok = await verify();
  process.exit(ok ? 0 : 1);
} else {
  console.log('Packing preset bundles…');
  await build();
  console.log('\nVerifying…');
  const ok = await verify();
  process.exit(ok ? 0 : 1);
}
