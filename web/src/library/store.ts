// Phase 10A.1 — LibraryStore: the high-level content-library API over IndexedDB.
//
// Design notes:
//  - Metadata (`items`) and payload (`payloads`) are separate stores, so
//    list/query/recordUsage/setFavorite never rewrite heavy payload bytes.
//  - `favorite` is persisted as 0/1 (booleans are not valid IndexedDB keys) and
//    exposed as a boolean at this boundary.
//  - Every op is failure-tolerant: if the db never opened (unavailable / quota /
//    migration failure) the store reports status and reads return empty / writes
//    reject with a clear message — the renderer must keep working regardless.

import {
  IDX_COLLECTION,
  IDX_FAVORITE,
  IDX_LAST_USED,
  IDX_TYPE,
  LIBRARY_DB_NAME,
  LIBRARY_DB_VERSION,
  STORE_COLLECTIONS,
  STORE_ITEMS,
  STORE_PAYLOADS,
  STORE_PREVIEW_BANK,
  STORE_THUMBNAILS,
  openLibraryDB,
  reqToPromise,
  withTx,
} from './db';
import {
  Collection,
  ContentType,
  FullLibraryItem,
  LibraryItem,
  LibraryPayload,
  LIBRARY_ITEM_SCHEMA_VERSION,
  PreviewBank,
  ThumbnailRecord,
  isValidItem,
} from './types';

export type LibraryStatus = 'uninitialized' | 'ready' | 'unavailable' | 'error';

/** Stored metadata shape (favorite as 0/1). Private to the store. */
interface StoredMeta extends Omit<LibraryItem, 'favorite'> {
  favorite: 0 | 1;
}

const now = (): number => Date.now();

export class LibraryStore {
  private db: IDBDatabase | null = null;
  private _status: LibraryStatus = 'uninitialized';
  private _error: string | null = null;
  private readonly name: string;
  private readonly version: number;

  constructor(name = LIBRARY_DB_NAME, version = LIBRARY_DB_VERSION) {
    this.name = name;
    this.version = version;
  }

  get status(): LibraryStatus {
    return this._status;
  }
  get error(): string | null {
    return this._error;
  }
  ready(): boolean {
    return this._status === 'ready' && this.db !== null;
  }

  /** Open the database. Never throws — records status and returns it, so a
   *  library failure can never crash app startup. */
  async init(): Promise<LibraryStatus> {
    if (this._status === 'ready') return this._status;
    try {
      this.db = await openLibraryDB(this.name, this.version);
      this._status = 'ready';
      this._error = null;
    } catch (e) {
      this.db = null;
      this._error = e instanceof Error ? e.message : String(e);
      this._status = /not available/i.test(this._error) ? 'unavailable' : 'error';
    }
    return this._status;
  }

  close(): void {
    this.db?.close();
    this.db = null;
    this._status = 'uninitialized';
  }

  private require(): IDBDatabase {
    if (!this.db) throw new Error(`library store not ready (${this._status}: ${this._error ?? 'no db'})`);
    return this.db;
  }

  private static toStored(item: LibraryItem): StoredMeta {
    return { ...item, favorite: item.favorite ? 1 : 0 };
  }
  private static toItem(meta: StoredMeta): LibraryItem {
    return { ...meta, favorite: meta.favorite === 1 };
  }

  // --- CRUD ---------------------------------------------------------------

  /** Insert or replace an item (metadata + payload) atomically. */
  async put(item: LibraryItem, payload: LibraryPayload): Promise<void> {
    const db = this.require();
    const meta = LibraryStore.toStored({ ...item, schemaVersion: LIBRARY_ITEM_SCHEMA_VERSION });
    await withTx(db, [STORE_ITEMS, STORE_PAYLOADS], 'readwrite', (tx) => {
      tx.objectStore(STORE_ITEMS).put(meta);
      tx.objectStore(STORE_PAYLOADS).put({ id: item.id, payload });
    });
  }

  /** Metadata only (fast — no payload bytes). Corrupt records are isolated. */
  async get(id: string): Promise<LibraryItem | null> {
    const db = this.require();
    const meta = await withTx(db, [STORE_ITEMS], 'readonly', (tx) =>
      reqToPromise<unknown>(tx.objectStore(STORE_ITEMS).get(id)),
    );
    if (!isValidItem(meta)) return null;
    return LibraryStore.toItem(meta as unknown as StoredMeta);
  }

  /** Metadata + payload (join across the two stores). */
  async getFull(id: string): Promise<FullLibraryItem | null> {
    const db = this.require();
    const [metaRaw, payRaw] = await withTx(db, [STORE_ITEMS, STORE_PAYLOADS], 'readonly', (tx) =>
      Promise.all([
        reqToPromise<unknown>(tx.objectStore(STORE_ITEMS).get(id)),
        reqToPromise<{ id: string; payload: LibraryPayload } | undefined>(tx.objectStore(STORE_PAYLOADS).get(id)),
      ]),
    );
    if (!isValidItem(metaRaw) || !payRaw) return null;
    return { ...LibraryStore.toItem(metaRaw as unknown as StoredMeta), payload: payRaw.payload };
  }

  async delete(id: string): Promise<void> {
    const db = this.require();
    await withTx(db, [STORE_ITEMS, STORE_PAYLOADS, STORE_THUMBNAILS], 'readwrite', (tx) => {
      tx.objectStore(STORE_ITEMS).delete(id);
      tx.objectStore(STORE_PAYLOADS).delete(id);
      tx.objectStore(STORE_THUMBNAILS).delete(id);
    });
  }

  /** Patch metadata fields only (never rewrites payload). The put is issued from
   *  the get's onsuccess so the transaction stays active across the read+write. */
  async update(id: string, patch: Partial<Omit<LibraryItem, 'id' | 'type'>>): Promise<LibraryItem | null> {
    const db = this.require();
    let updated: LibraryItem | null = null;
    await withTx(db, [STORE_ITEMS], 'readwrite', (tx) => {
      const store = tx.objectStore(STORE_ITEMS);
      const g = store.get(id);
      g.onsuccess = () => {
        if (!isValidItem(g.result)) return;
        const cur = g.result as unknown as StoredMeta;
        const next: StoredMeta = {
          ...cur,
          ...patch,
          favorite: patch.favorite === undefined ? cur.favorite : patch.favorite ? 1 : 0,
          id: cur.id,
          type: cur.type,
        };
        store.put(next);
        updated = LibraryStore.toItem(next);
      };
    });
    return updated;
  }

  // --- queries ------------------------------------------------------------

  private static collect(store: IDBObjectStore | IDBIndex, range?: IDBKeyRange | null): Promise<LibraryItem[]> {
    return new Promise((resolve, reject) => {
      const out: LibraryItem[] = [];
      const req = store.openCursor(range ?? null);
      req.onsuccess = () => {
        const cur = req.result;
        if (!cur) {
          resolve(out);
          return;
        }
        const v = cur.value;
        if (isValidItem(v)) out.push(LibraryStore.toItem(v as unknown as StoredMeta)); // skip corrupt rows
        cur.continue();
      };
      req.onerror = () => reject(req.error ?? new Error('cursor failed'));
    });
  }

  async getAll(): Promise<LibraryItem[]> {
    const db = this.require();
    return withTx(db, [STORE_ITEMS], 'readonly', (tx) => LibraryStore.collect(tx.objectStore(STORE_ITEMS)));
  }

  async listByType(type: ContentType): Promise<LibraryItem[]> {
    const db = this.require();
    return withTx(db, [STORE_ITEMS], 'readonly', (tx) =>
      LibraryStore.collect(tx.objectStore(STORE_ITEMS).index(IDX_TYPE), IDBKeyRange.only(type)),
    );
  }

  async listFavorites(): Promise<LibraryItem[]> {
    const db = this.require();
    return withTx(db, [STORE_ITEMS], 'readonly', (tx) =>
      LibraryStore.collect(tx.objectStore(STORE_ITEMS).index(IDX_FAVORITE), IDBKeyRange.only(1)),
    );
  }

  async listByCollection(collectionId: string): Promise<LibraryItem[]> {
    const db = this.require();
    return withTx(db, [STORE_ITEMS], 'readonly', (tx) =>
      LibraryStore.collect(tx.objectStore(STORE_ITEMS).index(IDX_COLLECTION), IDBKeyRange.only(collectionId)),
    );
  }

  /** Most-recently-used first, up to `limit`. Uses the lastUsed index in reverse. */
  async listRecent(limit = 24): Promise<LibraryItem[]> {
    const db = this.require();
    return withTx(db, [STORE_ITEMS], 'readonly', (tx) => {
      const idx = tx.objectStore(STORE_ITEMS).index(IDX_LAST_USED);
      return new Promise<LibraryItem[]>((resolve, reject) => {
        const out: LibraryItem[] = [];
        const req = idx.openCursor(null, 'prev'); // only rows with a defined lastUsed are indexed
        req.onsuccess = () => {
          const cur = req.result;
          if (!cur || out.length >= limit) {
            resolve(out);
            return;
          }
          if (isValidItem(cur.value)) out.push(LibraryStore.toItem(cur.value as unknown as StoredMeta));
          cur.continue();
        };
        req.onerror = () => reject(req.error ?? new Error('recent cursor failed'));
      });
    });
  }

  // --- favorites / recents ------------------------------------------------

  async setFavorite(id: string, favorite: boolean): Promise<void> {
    await this.update(id, { favorite });
  }

  /** Bump lastUsed + usageCount (metadata-only write). */
  async recordUsage(id: string): Promise<void> {
    const db = this.require();
    await withTx(db, [STORE_ITEMS], 'readwrite', (tx) => {
      const store = tx.objectStore(STORE_ITEMS);
      const g = store.get(id);
      g.onsuccess = () => {
        if (!isValidItem(g.result)) return;
        const cur = g.result as unknown as StoredMeta;
        store.put({ ...cur, lastUsed: now(), usageCount: (cur.usageCount ?? 0) + 1 });
      };
    });
  }

  // --- collections --------------------------------------------------------

  async createCollection(name: string, id?: string): Promise<Collection> {
    const db = this.require();
    const col: Collection = { id: id ?? `col:${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6).toString(36)}`, name, dateAdded: now() };
    await withTx(db, [STORE_COLLECTIONS], 'readwrite', (tx) => tx.objectStore(STORE_COLLECTIONS).put(col));
    return col;
  }

  async listCollections(): Promise<Collection[]> {
    const db = this.require();
    return withTx(db, [STORE_COLLECTIONS], 'readonly', (tx) =>
      reqToPromise<Collection[]>(tx.objectStore(STORE_COLLECTIONS).getAll()),
    );
  }

  async deleteCollection(collectionId: string): Promise<void> {
    const db = this.require();
    // Remove the collection and strip its id from any member items.
    const members = await this.listByCollection(collectionId);
    await withTx(db, [STORE_COLLECTIONS, STORE_ITEMS], 'readwrite', (tx) => {
      tx.objectStore(STORE_COLLECTIONS).delete(collectionId);
      const items = tx.objectStore(STORE_ITEMS);
      for (const m of members) {
        const meta = LibraryStore.toStored(m);
        meta.collections = (meta.collections ?? []).filter((c) => c !== collectionId);
        items.put(meta);
      }
    });
  }

  async addToCollection(itemId: string, collectionId: string): Promise<void> {
    await this.mutateCollections(itemId, (cols) => (cols.includes(collectionId) ? cols : [...cols, collectionId]));
  }

  async removeFromCollection(itemId: string, collectionId: string): Promise<void> {
    await this.mutateCollections(itemId, (cols) => cols.filter((c) => c !== collectionId));
  }

  private async mutateCollections(itemId: string, fn: (cols: string[]) => string[]): Promise<void> {
    const db = this.require();
    await withTx(db, [STORE_ITEMS], 'readwrite', (tx) => {
      const store = tx.objectStore(STORE_ITEMS);
      const g = store.get(itemId);
      g.onsuccess = () => {
        if (!isValidItem(g.result)) return;
        const cur = g.result as unknown as StoredMeta;
        store.put({ ...cur, collections: fn(cur.collections ?? []) });
      };
    });
  }

  // --- preview bank (storage primitive only; 10B implements behavior) -----

  async getPreviewBank(id = 'default'): Promise<PreviewBank> {
    const db = this.require();
    const rec = await withTx(db, [STORE_PREVIEW_BANK], 'readonly', (tx) =>
      reqToPromise<PreviewBank | undefined>(tx.objectStore(STORE_PREVIEW_BANK).get(id)),
    );
    return rec ?? { id, itemIds: [] };
  }

  async setPreviewBank(itemIds: string[], id = 'default'): Promise<void> {
    const db = this.require();
    await withTx(db, [STORE_PREVIEW_BANK], 'readwrite', (tx) =>
      tx.objectStore(STORE_PREVIEW_BANK).put({ id, itemIds }),
    );
  }

  // --- thumbnails (storage primitive only; no rendering here) -------------

  async putThumbnail(rec: ThumbnailRecord): Promise<void> {
    const db = this.require();
    await withTx(db, [STORE_THUMBNAILS], 'readwrite', (tx) => tx.objectStore(STORE_THUMBNAILS).put(rec));
  }

  async getThumbnail(id: string): Promise<ThumbnailRecord | null> {
    const db = this.require();
    const rec = await withTx(db, [STORE_THUMBNAILS], 'readonly', (tx) =>
      reqToPromise<ThumbnailRecord | undefined>(tx.objectStore(STORE_THUMBNAILS).get(id)),
    );
    return rec ?? null;
  }

  /** Count of metadata rows (cheap). */
  async count(): Promise<number> {
    const db = this.require();
    return withTx(db, [STORE_ITEMS], 'readonly', (tx) => reqToPromise<number>(tx.objectStore(STORE_ITEMS).count()));
  }
}
