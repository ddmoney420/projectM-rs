// Phase 10A.2 — main-thread client for the shard worker.
//
// Prefers the worker (keeps decompression off the render thread); if the worker
// can't start or errors, it falls back to decompressing on the main thread so a
// worker failure degrades gracefully rather than breaking preset loading. A
// bounded per-shard cache avoids re-fetching; decompressed text is NEVER
// persisted to IndexedDB.

import { fetchShardText, parseNdjson } from './shard-decode';

const MAX_SHARDS = 3;
const WORKER_TIMEOUT_MS = 15000;

export class ShardClient {
  private worker: Worker | null = null;
  private workerDead = false;
  private seq = 0;
  private pending = new Map<number, { resolve: (t: string | null) => void; reject: (e: Error) => void }>();
  private mainCache = new Map<string, Map<string, string>>();

  /** `forceMainThread` skips the worker entirely (used by tests to exercise the
   *  main-thread decompression fallback deterministically). */
  constructor(private readonly opts: { forceMainThread?: boolean } = {}) {}

  private ensureWorker(): Worker | null {
    if (this.opts.forceMainThread || this.workerDead) return null;
    if (this.worker) return this.worker;
    try {
      const w = new Worker(new URL('./shard-worker.ts', import.meta.url), { type: 'module' });
      w.onmessage = (e: MessageEvent) => {
        const { id, ok, text, error } = e.data as { id: number; ok: boolean; text?: string | null; error?: string };
        const p = this.pending.get(id);
        if (!p) return;
        this.pending.delete(id);
        if (ok) p.resolve(text ?? null);
        else p.reject(new Error(error ?? 'shard worker error'));
      };
      w.onerror = () => {
        this.workerDead = true;
        this.worker = null;
        for (const p of this.pending.values()) p.reject(new Error('shard worker crashed'));
        this.pending.clear();
      };
      this.worker = w;
      return w;
    } catch {
      this.workerDead = true;
      return null;
    }
  }

  /** Return one preset's `.milk` text from a shard, or null if absent. */
  async getPresetText(shardUrl: string, path: string): Promise<string | null> {
    const w = this.ensureWorker();
    if (w) {
      try {
        return await this.viaWorker(w, shardUrl, path);
      } catch {
        /* fall through to main-thread decompression */
      }
    }
    return this.viaMainThread(shardUrl, path);
  }

  private viaWorker(w: Worker, shardUrl: string, path: string): Promise<string | null> {
    return new Promise<string | null>((resolve, reject) => {
      const id = ++this.seq;
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error('shard worker timeout'));
      }, WORKER_TIMEOUT_MS);
      this.pending.set(id, {
        resolve: (t) => {
          clearTimeout(timer);
          resolve(t);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
      w.postMessage({ id, shardUrl, path });
    });
  }

  private async viaMainThread(shardUrl: string, path: string): Promise<string | null> {
    let map = this.mainCache.get(shardUrl);
    if (!map) {
      map = parseNdjson(await fetchShardText(shardUrl));
      this.mainCache.set(shardUrl, map);
      while (this.mainCache.size > MAX_SHARDS) {
        const oldest = this.mainCache.keys().next().value as string;
        this.mainCache.delete(oldest);
      }
    }
    return map.get(path) ?? null;
  }

  dispose(): void {
    this.worker?.terminate();
    this.worker = null;
    this.pending.clear();
    this.mainCache.clear();
  }
}
