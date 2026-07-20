// Public entry point for the content library.
export * from './types';
export * from './db';
export { LibraryStore } from './store';
export type { LibraryStatus } from './store';
// Phase 10A.2 — Milkdrop packs, shards, import.
export * from './pack';
export { ShardClient } from './shard-client';
export { MilkdropLibrary } from './milkdrop';
export type { PackLoadResult } from './milkdrop';
export { parseMilkFilename, detectTextureRefs, readMilkFiles } from './import-milk';

import {
  ContentType,
  LibraryItem,
  LIBRARY_ITEM_SCHEMA_VERSION,
  Origin,
  StableId,
} from './types';

/** Build a LibraryItem envelope with sensible defaults. `id` may be supplied
 *  (bundled/pack) or auto-derived from origin+type (user/imported). Callers pass
 *  the payload separately to LibraryStore.put(). */
export function makeItem(
  fields: Partial<LibraryItem> & { type: ContentType; name: string; origin: Origin },
): LibraryItem {
  const id =
    fields.id ??
    (fields.origin === 'imported' ? StableId.imported(fields.type) : StableId.user(fields.type));
  return {
    id,
    type: fields.type,
    name: fields.name,
    author: fields.author,
    description: fields.description,
    tags: fields.tags,
    collections: fields.collections ?? [],
    favorite: fields.favorite ?? false,
    dateAdded: fields.dateAdded ?? Date.now(),
    lastUsed: fields.lastUsed,
    usageCount: fields.usageCount ?? 0,
    thumbnailRef: fields.thumbnailRef,
    license: fields.license,
    attribution: fields.attribution,
    origin: fields.origin,
    schemaVersion: fields.schemaVersion ?? LIBRARY_ITEM_SCHEMA_VERSION,
  };
}
