// Phase 10A.2 — robust shard fetch/decompress shared by the worker and the
// main-thread client.
//
// A shard may arrive as raw gzip bytes OR already-decompressed: some servers/CDNs
// (including Vite's preview server) set `Content-Encoding: gzip` on a `.gz` file,
// so the browser transparently decodes it and our explicit decompression would
// otherwise double-decode. We detect the gzip magic (0x1f 0x8b) and only run
// DecompressionStream when the bytes are actually gzip; otherwise we decode the
// bytes as UTF-8 directly. This keeps preset text handling correct regardless of
// how the shard is served — and it is pure JS (no wasm / no block_on concern).

export async function fetchShardText(url: string): Promise<string> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`shard fetch failed: ${res.status}`);
  const buf = new Uint8Array(await res.arrayBuffer());
  if (buf.length >= 2 && buf[0] === 0x1f && buf[1] === 0x8b) {
    const stream = new Blob([buf]).stream().pipeThrough(new DecompressionStream('gzip'));
    return await new Response(stream).text();
  }
  return new TextDecoder().decode(buf);
}

export function parseNdjson(text: string): Map<string, string> {
  const map = new Map<string, string>();
  for (const line of text.split('\n')) {
    const t = line.trim();
    if (!t) continue;
    try {
      const o = JSON.parse(t) as { path?: unknown; text?: unknown };
      if (typeof o.path === 'string' && typeof o.text === 'string') map.set(o.path, o.text);
    } catch {
      /* skip a corrupt line, keep the rest of the shard */
    }
  }
  return map;
}
