// Phase 10A.3 — Shader + Scene content library.
//
// Reuses the engine's transactional import path for BOTH shader and scene
// loads: import_scene validates and applies atomically and retains the current
// scene on failure, so a corrupt saved shader/scene can never black out or
// corrupt the master output. A shader load is expressed as "replace the shader
// layer's source in the current scene, then import that scene", which inherits
// the same guarantee. Built-in shaders live in memory; only items the user
// touches (favorite/use/save) are written to IndexedDB.

import { LibraryStore } from './store';
import { builtinShaderItems, isValidShaderPayload, shaderTags } from './builtins';
import {
  Collection,
  ContentType,
  FullLibraryItem,
  LibraryItem,
  LIBRARY_ITEM_SCHEMA_VERSION,
  ScenePayload,
  ShaderPayload,
  StableId,
} from './types';

export interface ImportResult {
  ok: boolean;
  error?: string;
}

function newUserItem(type: ContentType, name: string): LibraryItem {
  return {
    id: StableId.user(type),
    type,
    name,
    collections: [],
    favorite: false,
    dateAdded: Date.now(),
    usageCount: 0,
    origin: 'user',
    schemaVersion: LIBRARY_ITEM_SCHEMA_VERSION,
  };
}

/** Minimal engine surface the content library needs (wired to the wasm exports
 *  in main.ts). Kept tiny + injectable so this module stays testable/pure. */
export interface EngineAdapter {
  exportScene(): string;
  importScene(json: string): ImportResult;
}

type SceneJson = { layers?: SceneLayer[] } & Record<string, unknown>;
type SceneLayer = { source?: { kind?: string } & Record<string, unknown> } & Record<string, unknown>;

export class ContentLibrary {
  private builtins = new Map<string, { item: LibraryItem; payload: ShaderPayload }>();

  constructor(
    private readonly store: LibraryStore,
    private readonly engine: EngineAdapter,
  ) {
    for (const b of builtinShaderItems()) this.builtins.set(b.item.id, b);
  }

  // --- built-in shaders (in-memory) --------------------------------------

  listBuiltinShaders(): LibraryItem[] {
    return [...this.builtins.values()].map((b) => b.item);
  }

  // --- shader: save / load (transactional) -------------------------------

  /** Save the current shader project (the first shader layer's full source) as
   *  a user library entry. Does not alter the running shader. */
  async saveCurrentShader(name: string): Promise<LibraryItem | null> {
    const payload = this.currentShaderPayload();
    if (!payload) return null;
    const item = newUserItem('shader', name);
    item.tags = shaderTags(payload);
    item.license = 'user-created';
    await this.store.put(item, payload);
    return item;
  }

  /** Load a shader item: validate → apply into the current scene atomically. On
   *  failure the active project is retained (last-known-good). */
  async loadShader(id: string): Promise<ImportResult> {
    const payload = await this.resolveShaderPayload(id);
    if (!payload || !isValidShaderPayload(payload)) return { ok: false, error: 'invalid or missing shader payload' };
    const res = this.applyShaderPayload(payload);
    if (res.ok) await this.markUsed(id);
    return res;
  }

  private currentShaderPayload(): ShaderPayload | null {
    let scene: SceneJson;
    try {
      scene = JSON.parse(this.engine.exportScene());
    } catch {
      return null;
    }
    const layer = (scene.layers ?? []).find((l) => l.source?.kind === 'shader');
    if (!layer?.source) return null;
    const { kind: _kind, ...rest } = layer.source;
    return rest as unknown as ShaderPayload;
  }

  private async resolveShaderPayload(id: string): Promise<ShaderPayload | null> {
    const b = this.builtins.get(id);
    if (b) return b.payload;
    const full = await this.store.getFull(id).catch(() => null);
    return full && full.type === 'shader' ? (full.payload as ShaderPayload) : null;
  }

  /** Build a minimal single-shader-layer scene JSON for auditioning a shader in
   *  a deck (e.g. Deck B) without disturbing the current scene. Null if
   *  unresolved. */
  async sceneJsonForShader(id: string): Promise<string | null> {
    const payload = await this.resolveShaderPayload(id);
    if (!payload) return null;
    const scene = {
      schema_version: 1,
      scene_id: 'audition',
      name: 'Audition',
      layers: [
        { id: 1, name: 'Shader', enabled: true, visible: true, opacity: 1, blend: 'normal', source: { kind: 'shader', ...payload }, effects: [] },
      ],
      speed: 1,
      paused: false,
      bpm: 120,
      tempo_manual: false,
      subdivision: 1,
      global_effects: [],
    };
    return JSON.stringify(scene);
  }

  /** Replace the current scene's first shader layer source with `payload` (or
   *  append a shader layer if none) and import the scene transactionally. */
  private applyShaderPayload(payload: ShaderPayload): ImportResult {
    let scene: SceneJson;
    try {
      scene = JSON.parse(this.engine.exportScene());
    } catch (e) {
      return { ok: false, error: 'could not read current scene' };
    }
    const source = { kind: 'shader', ...payload } as SceneLayer['source'];
    const layers = scene.layers ?? (scene.layers = []);
    const existing = layers.find((l) => l.source?.kind === 'shader');
    if (existing) {
      existing.source = source;
    } else {
      layers.push({
        id: Date.now(),
        name: 'Shader',
        enabled: true,
        visible: true,
        opacity: 1,
        blend: 'normal',
        source,
        effects: [],
      });
    }
    return this.engine.importScene(JSON.stringify(scene));
  }

  // --- scene: save / load (transactional) --------------------------------

  /** Save the current scene verbatim (its own SceneData `schema_version`
   *  preserved) as a user library entry. Does not alter the active scene. */
  async saveCurrentScene(name: string): Promise<LibraryItem | null> {
    let payload: ScenePayload;
    try {
      payload = JSON.parse(this.engine.exportScene());
    } catch {
      return null;
    }
    const item = newUserItem('scene', name);
    await this.store.put(item, payload);
    return item;
  }

  /** Load a saved scene via the engine's transactional import (retains current
   *  scene on failure). */
  async loadScene(id: string): Promise<ImportResult> {
    const full = await this.store.getFull(id).catch(() => null);
    if (!full || full.type !== 'scene') return { ok: false, error: 'missing scene' };
    const res = this.engine.importScene(JSON.stringify(full.payload));
    if (res.ok) await this.store.recordUsage(id).catch(() => {});
    return res;
  }

  // --- user entry management (built-ins protected) -----------------------

  async rename(id: string, name: string): Promise<LibraryItem | null> {
    if (this.builtins.has(id)) return null; // built-ins are read-only
    return this.store.update(id, { name });
  }

  /** Duplicate any item (incl. a built-in) into a new user-owned entry. */
  async duplicate(id: string, name?: string): Promise<LibraryItem | null> {
    const src = this.builtins.get(id) ?? (await this.getFull(id).then((f) => (f ? { item: f, payload: f.payload } : null)));
    if (!src) return null;
    const item = newUserItem(src.item.type, name ?? `${src.item.name} copy`);
    item.tags = src.item.tags;
    item.license = src.item.type === 'shader' ? 'user-created' : undefined;
    await this.store.put(item, src.payload);
    return item;
  }

  /** Delete a USER entry. Built-ins cannot be destructively deleted. */
  async delete(id: string): Promise<boolean> {
    if (this.builtins.has(id)) return false;
    const meta = await this.store.get(id).catch(() => null);
    if (meta && meta.origin === 'builtin') return false;
    await this.store.delete(id);
    return true;
  }

  // --- favorites / collections / recent (metadata-only) ------------------

  async setFavorite(id: string, favorite: boolean): Promise<void> {
    await this.ensurePersisted(id);
    await this.store.setFavorite(id, favorite).catch(() => {});
  }

  async addToCollection(id: string, collectionId: string): Promise<void> {
    await this.ensurePersisted(id);
    await this.store.addToCollection(id, collectionId).catch(() => {});
  }
  removeFromCollection(id: string, collectionId: string): Promise<void> {
    return this.store.removeFromCollection(id, collectionId).catch(() => {});
  }
  listByCollection(collectionId: string): Promise<LibraryItem[]> {
    return this.store.listByCollection(collectionId).catch(() => []);
  }
  createCollection(name: string): Promise<Collection> {
    return this.store.createCollection(name);
  }

  listByType(type: ContentType): Promise<LibraryItem[]> {
    return this.store.listByType(type).catch(() => []);
  }
  listRecent(limit = 24): Promise<LibraryItem[]> {
    return this.store.listRecent(limit).catch(() => []);
  }
  listFavorites(): Promise<LibraryItem[]> {
    return this.store.listFavorites().catch(() => []);
  }
  getFull(id: string): Promise<FullLibraryItem | null> {
    return this.store.getFull(id).catch(() => null);
  }

  /** Upsert a built-in into the store (so favorite/usage can persist). No-op for
   *  items that already live in the store. */
  private async ensurePersisted(id: string): Promise<void> {
    const b = this.builtins.get(id);
    if (b && !(await this.store.get(id).catch(() => null))) await this.store.put(b.item, b.payload).catch(() => {});
  }

  private async markUsed(id: string): Promise<void> {
    await this.ensurePersisted(id);
    await this.store.recordUsage(id).catch(() => {});
  }
}
