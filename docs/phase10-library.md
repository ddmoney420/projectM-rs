# Phase 10 — Content Library (10A.1 foundation)

The content library is the data foundation for the performance-VJ milestone
(library → preview → decks → crossfader). **10A.1 ships the model + storage
only** — no corpus loader, no decks, no preview, no crossfader, no UI.
Implementation lives in `web/src/library/`.

## Updated Phase 10 ordering

The original plan put Preview/Audition (10B) before the Deck abstraction (10C.1),
but the Milkdrop audition path depends on an inactive deck. Corrected sequence:

```
10A.1  Library model + IndexedDB        ← this change
10A.2  Milkdrop library integration     (preset-pack loader; corpus/licensing decision)
10A.3  Shader/Scene library
10A.4  Library browser UI
10C.1  Minimal Deck abstraction          (moved BEFORE preview)
10B    Preview/Audition Bank             (depends on an inactive deck)
10C.2  Master crossfader
10C.3  MIDI/keyboard performance controls
10D    Dual-Milkdrop benchmark/productionization
```

## LibraryItem schema (`web/src/library/types.ts`)

A common metadata envelope + a typed `payload`, stored in **two** IndexedDB
stores so listing/usage-bumps never rewrite heavy payload bytes.

```
LibraryItem { id, type:'milkdrop'|'shader'|'scene', name,
              author?, description?, tags?, collections?[],
              favorite, dateAdded, lastUsed?, usageCount,
              thumbnailRef?, license?, attribution?, origin, schemaVersion }
```

`origin ∈ { builtin, user, imported, pack }`. Optional fields are not required
per type. `LIBRARY_ITEM_SCHEMA_VERSION = 1`.

## Content-type payload strategy

- **milkdrop** — a *reference*, never bundled text copied into IndexedDB:
  `{ kind:'pack', packId, shard, path }`, or user-owned `{ kind:'inline', text }`.
  10A.1 defines the shape only; **no corpus is loaded**.
- **shader** — mirrors the engine's serialized `SourceState::Shader`
  (`source, mode, controls, mods, attribution, passes`). Full multipass state is
  preserved; a shader is **not** reduced to one GLSL string.
- **scene** — the opaque, versioned `SceneData` JSON exactly as the engine emits
  it (`schema_version` inside). Stored verbatim; no second scene format.

## Stable-ID strategy (`StableId`)

IDs are derived from stable identity, never a display name or array index, so
they survive favorites/recents/collections/reload/restart:

- pack:     `pack:<packId>:<path>`
- builtin:  `builtin:<type>:<slug(name)>`
- user:     `user:<type>:<uuid>`   (crypto.randomUUID)
- imported: `imported:<type>:<uuid>`

## IndexedDB database/store structure (`web/src/library/db.ts`)

Database `pm-web-library`, **version 1**. Stores:

| store         | keyPath | contents |
|---------------|---------|----------|
| `items`       | `id`    | lightweight metadata (indexed, listed) |
| `payloads`    | `id`    | heavy payload (fetched only on load) |
| `collections` | `id`    | user collection metadata |
| `previewBank` | `id`    | ordered item-id references (10B) |
| `thumbnails`  | `id`    | cached generated thumbnails (== item id) |

Indexes on `items`: `by_type`, `by_lastUsed`, `by_favorite` (0/1 — booleans are
not valid IndexedDB keys, so `favorite` is stored 0/1 and exposed as a boolean at
the store boundary), `by_collection` (multiEntry).

The existing localStorage keys (`pm-web-scene-v1`, `pm-web-midi-v1`,
`pm-web-phase5-v1`, `pm-web-onboarded-v1`) are **unchanged** — the library has its
own database and version.

## Collections strategy

**Multi-membership via an array on the item** (`item.collections: string[]`) plus
a `multiEntry` index — chosen over a separate join store because it keeps writes
atomic with the item, supports items in multiple collections, and remains
queryable (`listByCollection` uses the multiEntry index). Collection *metadata*
(id/name) lives in the `collections` store. (Full collection-management UI is 10A.4.)

## Favorites / recent behavior

- `setFavorite(id, bool)` — metadata-only write.
- `recordUsage(id)` — bumps `lastUsed` + `usageCount` with a metadata-only write
  (never rewrites the payload).
- `listRecent(limit)` — reverse cursor over `by_lastUsed`.
- `listFavorites()` — `by_favorite = 1`.

## Thumbnail references

`thumbnailRef` on the item + a `thumbnails` store (storage primitive only). **No
thumbnail rendering in 10A.1, and no Milkdrop engines are created for
thumbnails.** Future strategy: shaders/scenes generate on-demand and cache;
Milkdrop uses a placeholder, with optional pre-generated static thumbnails later.

## Migration strategy

Explicit versions. `applyMigrations(db, oldVersion, tx)` adds only what each
version introduces (`if (oldVersion < N)`), so upgrades are **non-destructive** —
user data survives. A failed upgrade aborts the transaction (no half-migrated
db). A `deleteLibraryDB()` reset helper exists for **dev/test only**, not as a
production upgrade path. Covered by a migration-harness test (v1 data survives a
v2 upgrade that adds a store).

## Failure handling

`LibraryStore.init()` never throws; it records `status ∈ {ready, unavailable,
error}` + `error`. If IndexedDB is absent / blocked / quota-exceeded / migration
fails, the app logs a warning and keeps running — **the renderer never depends on
the library**, and the library is initialized non-blocking after startup
(`web/src/main.ts`). Corrupt individual records are isolated on read
(`isValidItem`) rather than breaking a whole query.

## Milkdrop corpus licensing boundary

There is currently **0 committed production Milkdrop corpus** beyond the engine's
minimal fallback. The library works fully with **zero** Milkdrop pack entries
(shaders + scenes function normally). **Sourcing and licensing a shippable preset
corpus is a Phase 10A.2 release-content decision** — unknown license ≠
redistributable; nothing third-party is bundled in 10A.1.

## Tests

- `web/verify-library.mjs` (Playwright, real IndexedDB): CRUD, type round-trip
  (milkdrop ref / shader / scene), favorites-persist-across-reopen, recent
  ordering + usage counters, collection membership add/remove, non-destructive
  migration harness, corrupt-record isolation, zero-corpus init, and
  persistence across a full page reload — plus assertions that the renderer keeps
  advancing and no console errors occur.
