# preset-pack

Packs Milkdrop `.milk` preset packs into **sharded, gzipped bundles + a manifest**
for the web visualizer (`pm-web`). One-time build step; the output is meant to be
copied into a web app's static assets and unpacked into IndexedDB on first use.

This is PR 2 of the vibrdrome-web integration: it only **generates static output**.
It does not modify any web app.

## Usage

```sh
# Defaults to the two official packs at their conventional clone locations:
#   ~/milk-presets                 (presets-cream-of-the-crop)
#   ~/milkdrop-original-presets    (presets-milkdrop-original)
node pack.mjs

# Or point at packs explicitly and choose an output dir:
node pack.mjs --out ./out \
  --pack cream-of-the-crop=/path/to/presets-cream-of-the-crop \
  --pack milkdrop-original=/path/to/presets-milkdrop-original

# Re-verify an existing build (decompress every shard, check counts):
node pack.mjs --verify-only
```

Source packs:
- https://github.com/projectM-visualizer/presets-cream-of-the-crop
- https://github.com/projectM-visualizer/presets-milkdrop-original

## Output (`out/presets/`)

```
out/presets/
  manifest.json            index of shards + totals + flat per-preset list
  <Category>.ndjson.gz      one gzipped NDJSON shard per top-level category
```

To deploy later (PR 3+), copy `out/presets/*` into `vibrdrome-web/public/presets/`.
`out/` is git-ignored (regenerable artifact).

### Shard format — gzipped NDJSON

Each shard decompresses to newline-delimited JSON, one preset per line:

```json
{"name":"Geiss - Cauldron","path":"cream-of-the-crop/Geometric/Geiss - Cauldron.milk","text":"<full .milk source>"}
```

`name` = display name (filename sans extension), `path` = unique key
(`<pack>/<relpath>`), `text` = the raw `.milk` source fed straight to
`PmEngine.load_preset`.

### `manifest.json`

```jsonc
{
  "version": 1,
  "sources": ["cream-of-the-crop", "milkdrop-original"],
  "totalPresets": 10347,
  "totalRawBytes": 121400000,
  "totalGzBytes": 10900000,
  "shards": [
    { "file": "Geometric.ndjson.gz", "category": "Geometric",
      "count": 1027, "rawBytes": 13800000, "gzBytes": 1240000 }
  ],
  "presets": [   // flat index so the app can list/search before fetching shards
    { "name": "...", "path": "...", "category": "Geometric", "shard": "Geometric.ndjson.gz" }
  ]
}
```

## Current corpus (regenerate to refresh)

- **10,347 presets** (9,795 cream-of-the-crop + 552 milkdrop-original), 12 shards
- ~116 MB raw `.milk` → **~10.4 MB** gzipped shards + ~2.1 MB `manifest.json`
- Decompresses on a modern browser via `DecompressionStream('gzip')`.
