# `__testpack__` — project-owned CC0 test fixtures

Minimal, **original, project-owned** Milkdrop pack fixtures used by
`web/verify-pack.mjs` to exercise the Phase 10A.2 pack loader / shard
decompression / lazy loading / navigation code paths. **Not** derived from any
community/third-party pack, and **not** the future starter pack.

`basic.ndjson.pack` is the gzip-compressed form of `basic.ndjson` (regenerate
with `gzip -c basic.ndjson > basic.ndjson.pack`). It uses a `.pack` extension
(not `.ndjson.gz`) because Vite's preview server sets `Content-Encoding: gzip`
with a compressed `Content-Length` for `.gz` files, which breaks `fetch`. Real
packs may use `.ndjson.gz`; the shard loader detects the gzip magic bytes and
handles both raw-gzip and already-decoded responses. The app does **not** load
this pack by default — it is served as a static same-origin file only so the
automated test (including the Web Worker) can fetch a real URL; Playwright
route-mocking does not reliably intercept worker requests. Licensed **CC0-1.0**
— see `LICENSE`.
