// Phase 10A.2 — content-agnostic Milkdrop library service.
//
// Holds an in-memory pack INDEX built from a manifest (so thousands of presets
// list without downloading any shard) and lazily loads a preset's `.milk` text
// from its shard on demand. Only items the user actually touches (favorite /
// use / import) are persisted to IndexedDB — the full pack index is never
// bulk-written. Works with ZERO packs configured.

import { LibraryStore } from './store';
import { ShardClient } from './shard-client';
import {
  PackManifest,
  classifyLicense,
  LicenseClass,
  packItemToLibraryItem,
  validatePackManifest,
} from './pack';
import { detectTextureRefs, parseMilkFilename } from './import-milk';
import { FullLibraryItem, LibraryItem, LIBRARY_ITEM_SCHEMA_VERSION, MilkdropPayload, StableId } from './types';

export interface PackLoadResult {
  ok: boolean;
  packId?: string;
  count: number;
  license?: string;
  licenseClass?: LicenseClass;
  requiresTextures?: boolean;
  error?: string;
}

interface IndexEntry {
  item: LibraryItem;
  payload: MilkdropPayload;
}

export class MilkdropLibrary {
  private index = new Map<string, IndexEntry>();
  private order: string[] = [];
  private manifest: PackManifest | null = null;
  private shardBase = '';

  constructor(
    private readonly store: LibraryStore,
    private readonly shards: ShardClient = new ShardClient(),
  ) {}

  // --- pack loading -------------------------------------------------------

  /** Fetch + validate a manifest and build the in-memory index. Downloads NO
   *  shards. Returns a result rather than throwing. */
  async loadPack(manifestUrl: string): Promise<PackLoadResult> {
    try {
      const res = await fetch(manifestUrl);
      if (!res.ok) return { ok: false, count: 0, error: `manifest fetch failed: ${res.status}` };
      const json = await res.json().catch(() => null);
      const v = validatePackManifest(json);
      if (!v.ok || !v.manifest) return { ok: false, count: 0, error: 'invalid manifest: ' + v.errors.join('; ') };
      const m = v.manifest;
      this.manifest = m;
      this.shardBase = new URL('.', manifestUrl).href;
      for (const it of m.items) {
        const { item, payload } = packItemToLibraryItem(m, it);
        if (this.index.has(item.id)) continue;
        this.index.set(item.id, { item, payload });
        this.order.push(item.id);
      }
      return {
        ok: true,
        packId: m.packId,
        count: m.items.length,
        license: m.license,
        licenseClass: classifyLicense(m.license, 'pack'),
        requiresTextures: m.requiresTextures === true,
      };
    } catch (e) {
      return { ok: false, count: 0, error: e instanceof Error ? e.message : String(e) };
    }
  }

  clearPack(): void {
    this.index.clear();
    this.order = [];
    this.manifest = null;
    this.shardBase = '';
  }

  indexCount(): number {
    return this.index.size;
  }

  /** The in-memory pack index (metadata only; favorite/lastUsed are defaults —
   *  the 10A.4 UI overlays persisted state via hydrate()). */
  listIndex(): LibraryItem[] {
    return this.order.map((id) => this.index.get(id)!.item);
  }

  license(): { license: string; class: LicenseClass; requiresTextures: boolean } | null {
    if (!this.manifest) return null;
    return {
      license: this.manifest.license,
      class: classifyLicense(this.manifest.license, 'pack'),
      requiresTextures: this.manifest.requiresTextures === true,
    };
  }

  private resolveShardUrl(shard: string): string {
    return this.shardBase ? new URL(shard, this.shardBase).href : shard;
  }

  // --- lazy preset text ---------------------------------------------------

  /** Load a preset's `.milk` text: inline payloads return directly; pack items
   *  fetch+decompress their shard lazily (worker). On success the item is
   *  upserted + usage recorded (so favorites/recent persist) — a metadata-only
   *  cost, no shard text in IndexedDB. Returns null if unavailable. */
  async presetText(id: string): Promise<string | null> {
    // Prefer a persisted item (imported/inline); else the in-memory pack index.
    const full = await this.store.getFull(id).catch(() => null);
    const payload = (full?.payload as MilkdropPayload | undefined) ?? this.index.get(id)?.payload;
    if (!payload) return null;
    let text: string | null = null;
    if (payload.kind === 'inline') {
      text = payload.text;
    } else {
      text = await this.shards.getPresetText(this.resolveShardUrl(payload.shard), payload.path).catch(() => null);
    }
    if (text !== null) await this.markUsed(id);
    return text;
  }

  /** Upsert a pack item into the store (if not already present) then bump usage. */
  private async markUsed(id: string): Promise<void> {
    const entry = this.index.get(id);
    if (entry && !(await this.store.get(id).catch(() => null))) {
      await this.store.put(entry.item, entry.payload).catch(() => {});
    }
    await this.store.recordUsage(id).catch(() => {});
  }

  // --- favorites ----------------------------------------------------------

  async setFavorite(id: string, favorite: boolean): Promise<void> {
    const entry = this.index.get(id);
    if (entry && !(await this.store.get(id).catch(() => null))) {
      await this.store.put(entry.item, entry.payload).catch(() => {});
    }
    await this.store.setFavorite(id, favorite).catch(() => {});
  }

  // --- navigation (deterministic for 0 and 1 items) ----------------------

  randomId(excludeId?: string): string | null {
    const n = this.order.length;
    if (n === 0) return null;
    if (n === 1) return this.order[0];
    let pick = this.order[Math.floor(Math.random() * n)];
    if (pick === excludeId) pick = this.order[(this.order.indexOf(pick) + 1) % n];
    return pick;
  }

  nextId(currentId?: string): string | null {
    const n = this.order.length;
    if (n === 0) return null;
    if (currentId === undefined) return this.order[0];
    const i = this.order.indexOf(currentId);
    if (i === -1) return this.order[0];
    return this.order[(i + 1) % n];
  }

  prevId(currentId?: string): string | null {
    const n = this.order.length;
    if (n === 0) return null;
    if (currentId === undefined) return this.order[n - 1];
    const i = this.order.indexOf(currentId);
    if (i === -1) return this.order[n - 1];
    return this.order[(i - 1 + n) % n];
  }

  // --- local import (stays local) ----------------------------------------

  /** Import `.milk` presets as inline, user-owned library entries. Returns the
   *  created items. Author is only set when the filename convention clearly
   *  matches (never fabricated). Detected texture references are recorded. */
  async importTexts(files: { name: string; text: string }[]): Promise<LibraryItem[]> {
    const created: LibraryItem[] = [];
    for (const f of files) {
      const { name, author } = parseMilkFilename(f.name);
      const textures = detectTextureRefs(f.text);
      const item: LibraryItem = {
        id: StableId.imported('milkdrop'),
        type: 'milkdrop',
        name,
        author,
        description: textures.length ? `references ${textures.length} texture(s): ${textures.slice(0, 6).join(', ')}` : undefined,
        tags: textures.length ? ['requires-textures'] : undefined,
        collections: [],
        favorite: false,
        dateAdded: Date.now(),
        usageCount: 0,
        license: 'user-imported',
        origin: 'imported',
        schemaVersion: LIBRARY_ITEM_SCHEMA_VERSION,
      };
      const payload: MilkdropPayload = { kind: 'inline', text: f.text };
      try {
        await this.store.put(item, payload);
        created.push(item);
      } catch {
        /* skip a failed insert */
      }
    }
    return created;
  }

  /** Full item + payload (for auditioning/loading). */
  getFull(id: string): Promise<FullLibraryItem | null> {
    return this.store.getFull(id).catch(() => null);
  }

  dispose(): void {
    this.shards.dispose();
  }
}
