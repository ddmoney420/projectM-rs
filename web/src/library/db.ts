// Phase 10A.1 — versioned IndexedDB layer for the content library.
//
// Object stores (database `pm-web-library`, version 1):
//   items       keyPath 'id'  — LIGHTWEIGHT metadata (indexed, listed/queried)
//   payloads    keyPath 'id'  — HEAVY payload, fetched only when an item loads
//   collections keyPath 'id'  — user collection metadata
//   previewBank keyPath 'id'  — ordered item-id references (10B)
//   thumbnails  keyPath 'id'  — cached generated thumbnails (== item id)
//
// The metadata/payload split means recordUsage()/setFavorite()/list never touch
// heavy payload bytes. Booleans are not valid IndexedDB keys, so `favorite` is
// stored as 0/1 in the metadata record and indexed; the store boundary maps it
// to/from the public boolean.

export const LIBRARY_DB_NAME = 'pm-web-library';
export const LIBRARY_DB_VERSION = 1;

export const STORE_ITEMS = 'items';
export const STORE_PAYLOADS = 'payloads';
export const STORE_COLLECTIONS = 'collections';
export const STORE_PREVIEW_BANK = 'previewBank';
export const STORE_THUMBNAILS = 'thumbnails';

export const IDX_TYPE = 'by_type';
export const IDX_LAST_USED = 'by_lastUsed';
export const IDX_FAVORITE = 'by_favorite';
export const IDX_COLLECTION = 'by_collection';

/** Create/upgrade stores for a given version step. Non-destructive: only adds
 *  what a version introduces, so user data survives upgrades. Future versions
 *  extend the switch; do NOT delete-and-recreate as an upgrade strategy. */
export function applyMigrations(db: IDBDatabase, oldVersion: number, _tx: IDBTransaction | null): void {
  if (oldVersion < 1) {
    const items = db.createObjectStore(STORE_ITEMS, { keyPath: 'id' });
    items.createIndex(IDX_TYPE, 'type', { unique: false });
    items.createIndex(IDX_LAST_USED, 'lastUsed', { unique: false });
    items.createIndex(IDX_FAVORITE, 'favorite', { unique: false }); // 0 | 1
    items.createIndex(IDX_COLLECTION, 'collections', { unique: false, multiEntry: true });

    db.createObjectStore(STORE_PAYLOADS, { keyPath: 'id' });
    db.createObjectStore(STORE_COLLECTIONS, { keyPath: 'id' });
    db.createObjectStore(STORE_PREVIEW_BANK, { keyPath: 'id' });
    db.createObjectStore(STORE_THUMBNAILS, { keyPath: 'id' });
  }
  // if (oldVersion < 2) { …add store/index for v2… }
}

export function isIndexedDbAvailable(): boolean {
  try {
    return typeof indexedDB !== 'undefined' && indexedDB !== null;
  } catch {
    return false;
  }
}

/** Open (and migrate) the library database. Rejects if IndexedDB is unavailable,
 *  blocked, or the upgrade throws — callers degrade gracefully. `name`/`version`
 *  are injectable so tests can exercise a fresh db and the migration path. */
export function openLibraryDB(name = LIBRARY_DB_NAME, version = LIBRARY_DB_VERSION): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    if (!isIndexedDbAvailable()) {
      reject(new Error('IndexedDB is not available in this browser/context'));
      return;
    }
    let req: IDBOpenDBRequest;
    try {
      req = indexedDB.open(name, version);
    } catch (e) {
      reject(e instanceof Error ? e : new Error(String(e)));
      return;
    }
    req.onupgradeneeded = (ev) => {
      const db = req.result;
      try {
        applyMigrations(db, ev.oldVersion, req.transaction);
      } catch (e) {
        // Abort the upgrade transaction so we never leave a half-migrated db.
        try {
          req.transaction?.abort();
        } catch {
          /* ignore */
        }
        reject(e instanceof Error ? e : new Error(String(e)));
      }
    };
    req.onsuccess = () => {
      const db = req.result;
      // A concurrent tab requesting a newer version needs us to step aside.
      db.onversionchange = () => db.close();
      resolve(db);
    };
    req.onerror = () => reject(req.error ?? new Error('IndexedDB open failed'));
    req.onblocked = () => reject(new Error('IndexedDB open blocked by another connection'));
  });
}

// --- promisified primitives ----------------------------------------------

export function reqToPromise<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error('IndexedDB request failed'));
  });
}

/** Run `fn` inside a transaction and resolve when it COMMITS (oncomplete), so
 *  callers know the write is durable. Any error/abort rejects. */
export function withTx<T>(
  db: IDBDatabase,
  stores: string[],
  mode: IDBTransactionMode,
  fn: (tx: IDBTransaction) => T,
): Promise<Awaited<T>> {
  return new Promise<Awaited<T>>((resolve, reject) => {
    let tx: IDBTransaction;
    try {
      tx = db.transaction(stores, mode);
    } catch (e) {
      reject(e instanceof Error ? e : new Error(String(e)));
      return;
    }
    let result: T;
    // `result` may be a value or a Promise (fn issued IDB requests and returns a
    // Promise); resolving with a thenable flattens it. The cast is required
    // because TS can't prove a generic T relates to Awaited<T>.
    tx.oncomplete = () => resolve(result as Awaited<T>);
    tx.onerror = () => reject(tx.error ?? new Error('IndexedDB transaction failed'));
    tx.onabort = () => reject(tx.error ?? new Error('IndexedDB transaction aborted'));
    try {
      result = fn(tx);
    } catch (e) {
      try {
        tx.abort();
      } catch {
        /* ignore */
      }
      reject(e instanceof Error ? e : new Error(String(e)));
    }
  });
}

/** Delete the whole database (dev/test reset helper — NOT a production upgrade
 *  strategy). */
export function deleteLibraryDB(name = LIBRARY_DB_NAME): Promise<void> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.deleteDatabase(name);
    req.onsuccess = () => resolve();
    req.onerror = () => reject(req.error ?? new Error('deleteDatabase failed'));
    req.onblocked = () => resolve(); // best-effort
  });
}
