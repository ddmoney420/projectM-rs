// Phase 10A.2 — shard decompression worker.
//
// Runs OFF the main/render thread: fetches a gzipped-NDJSON shard, decompresses
// it with the native DecompressionStream (no dependency, and NOT wasm — so the
// historical block_on constraint does not apply here), parses NDJSON, and
// returns one preset's `.milk` text. A bounded LRU keeps a few decompressed
// shards in the worker only — never persisted to IndexedDB.

import { fetchShardText, parseNdjson } from './shard-decode';

const ctx = self as unknown as Worker;

const cache = new Map<string, Map<string, string>>();
const MAX_SHARDS = 3;

async function loadShard(url: string): Promise<Map<string, string>> {
  const hit = cache.get(url);
  if (hit) {
    cache.delete(url);
    cache.set(url, hit); // LRU touch
    return hit;
  }
  const map = parseNdjson(await fetchShardText(url));
  cache.set(url, map);
  while (cache.size > MAX_SHARDS) {
    const oldest = cache.keys().next().value as string;
    cache.delete(oldest);
  }
  return map;
}

ctx.addEventListener('message', (e: MessageEvent) => {
  const { id, shardUrl, path } = e.data as { id: number; shardUrl: string; path: string };
  loadShard(shardUrl)
    .then((map) => ctx.postMessage({ id, ok: true, text: map.get(path) ?? null }))
    .catch((err) => ctx.postMessage({ id, ok: false, error: err instanceof Error ? err.message : String(err) }));
});

export {};
