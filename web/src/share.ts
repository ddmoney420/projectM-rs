// Shareable scene URLs. The current scene (export_scene → JSON) is deflate-raw
// compressed and base64url-encoded into the URL fragment (#s=…), so it is never
// sent to the server. On load, a #s= payload is decompressed and imported
// (transactionally in Rust — a bad payload keeps the default scene). Large
// scenes are rejected with a warning rather than producing an unusable URL.

import { export_scene, import_scene } from './pm_web/pm_web.js';

const MAX_URL_PAYLOAD = 24_000; // chars in the fragment; keep URLs practical

function b64urlEncode(bytes: Uint8Array): string {
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
function b64urlDecode(s: string) {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/');
  const bin = atob(b64 + '==='.slice((b64.length + 3) % 4));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function compress(str: string): Promise<string> {
  const cs = new CompressionStream('deflate-raw');
  const w = cs.writable.getWriter();
  void w.write(new TextEncoder().encode(str));
  void w.close();
  const buf = await new Response(cs.readable).arrayBuffer();
  return b64urlEncode(new Uint8Array(buf));
}
async function decompress(payload: string): Promise<string> {
  const ds = new DecompressionStream('deflate-raw');
  const w = ds.writable.getWriter();
  void w.write(b64urlDecode(payload));
  void w.close();
  const buf = await new Response(ds.readable).arrayBuffer();
  return new TextDecoder().decode(buf);
}

/** Build a shareable URL for the current scene, or null if it's too large. */
export async function buildShareUrl(): Promise<string | null> {
  const payload = await compress(export_scene());
  if (payload.length > MAX_URL_PAYLOAD) return null;
  return `${location.origin}${location.pathname}#s=${payload}`;
}

/** Build + copy the share URL to the clipboard. Returns a status message. */
export async function copyShareUrl(): Promise<string> {
  const url = await buildShareUrl();
  if (!url) return 'scene too large to share as a URL';
  try {
    await navigator.clipboard.writeText(url);
    // Reflect it in the address bar too (so a manual copy works).
    history.replaceState(null, '', url);
    return 'share URL copied';
  } catch {
    history.replaceState(null, '', url);
    return 'share URL set in address bar';
  }
}

/** If the page loaded with a #s= scene, decompress + import it. Returns true if
 *  a scene was applied. Never throws — a bad payload leaves the current scene. */
export async function loadFromUrl(): Promise<boolean> {
  const h = location.hash;
  if (!h.startsWith('#s=')) return false;
  try {
    const json = await decompress(h.slice(3));
    const res = JSON.parse(import_scene(json)) as { ok: boolean; error?: string };
    if (!res.ok) console.warn('shared scene rejected:', res.error);
    return res.ok;
  } catch (e) {
    console.warn('failed to load shared scene:', e);
    return false;
  }
}
