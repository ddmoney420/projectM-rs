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

## Phase 10A.2 — Milkdrop pack loader, shard worker, local import

Content-agnostic: the library works with **zero** configured packs; shaders,
scenes, and user-imported `.milk` presets function regardless. No third-party
corpus is loaded by default.

### Pack manifest format (`web/src/library/pack.ts`)

```
PackManifest { packId, name, version, license, licenseUrl?, source?,
               attribution?, takedownContact?, requiresTextures?, items[] }
PackItem     { path, name, shard, author?, license?, attribution?, category? }
```
The manifest is the **index** — listing thousands of presets needs no shard
download. `validatePackManifest()` isolates a bad manifest. Per-item fields
**override** pack-level defaults (a pack is not always one uniform license).

### License classification

`classifyLicense(license, origin)` →
`project-owned | explicitly-licensed | assumed-public-domain | user-imported | unknown-license`.
**`assumed-public-domain` is kept DISTINCT from an explicit CC0 dedication** so
the UI can never imply a stronger license than a manifest actually provides.

### Lazy shard architecture (`shard-decode.ts` / `shard-worker.ts` / `shard-client.ts`)

```
LibraryItem selected → resolve shard URL (relative to manifest)
  → ShardClient.getPresetText → Worker: fetch shard → decompress → parse NDJSON
  → return one preset's .milk text → (audition/Preset::load)
```
- **Worker**: gzip/NDJSON decompression runs off the render thread (native
  `DecompressionStream` — pure JS, **not wasm**, so no `block_on` concern).
- **Robust decode**: detects the gzip magic (`1f 8b`) and handles both raw-gzip
  and already-decoded (`Content-Encoding: gzip`) responses.
- **Fallback**: if the worker can't start/crashes, decompression falls back to
  the main thread (graceful degradation).
- **Bounded cache**: a small LRU of decompressed shards lives in the worker/client
  only — **never** persisted to IndexedDB (no duplicating pack text into the db).

### Milkdrop indexing + navigation (`milkdrop.ts`)

`MilkdropLibrary` holds an **in-memory index** built from the manifest (no bulk
IndexedDB writes for a 10k pack). Only items the user **touches** (favorite / use
/ import) are upserted into the store, so favorites/recent persist while the db
stays small. `randomId/nextId/prevId` operate over the index and are
deterministic for **0** (→ null) and **1** items. Loading a preset records usage
(metadata-only).

### Local `.milk` import (`import-milk.ts`)

User-gesture file selection → `readMilkFiles` → `importTexts` → `origin:'imported'`
+ `{kind:'inline', text}` in IndexedDB. **Stays local — nothing is uploaded.**
Author is parsed from the `Author - Title.milk` convention *only when it clearly
matches* (never fabricated). `detectTextureRefs` best-effort flags likely
external-texture needs (reported, not downloaded).

### Texture behavior

The unlicensed Milkdrop texture pack is **not** bundled. Presets referencing
external textures are preserved and flagged (`requires-textures` tag / detected
refs); missing textures never crash rendering (the engine already renders without
external samplers). No third-party texture assets are downloaded.

### Pack licensing / provenance policy

> **No third-party Milkdrop preset corpus is bundled or mirrored by default
> without explicit redistribution rights.** Hosting an "optional" pack from
> project infrastructure is still redistribution and is therefore not done until
> explicit permission/licensing is recorded.

- **Cream of the Crop / Classic / Milkdrop-original / Community / Base texture
  pack:** `USER IMPORT ONLY` (or EXCLUDE for the texture pack). No explicit
  license — `assumed-public-domain` or none.
- **En D:** first candidate for explicit-permission outreach (small,
  single-author). USER IMPORT ONLY pending a written license. *(No one is
  contacted without authorization.)*
- **Cream of the Crop:** large multi-author aggregation — curator permission may
  not resolve every author's rights; USER IMPORT ONLY pending stronger provenance.

### External-pack permission workflow (future)

```
pack candidate → verify explicit license/permission → author a license manifest
  → review → approve distribution → host/version off-repo
```
A public repository alone never enables project distribution.

### Original starter-pack plan

The only content shippable by default is an **original, project-owned starter
pack** (created under an explicit license, e.g. CC0/LGPL). That content effort is
separate from this library engine and is **not** created here. The engine already
supports it — a starter pack is just another manifest + shard.

### Zero-pack operation

With 0 configured packs the Milkdrop index is empty, navigation returns null, and
the app keeps full shader/scene/user-import functionality. No empty-pack state
blocks or crashes the library.

### Tests

- `web/verify-pack.mjs` (Playwright, real Worker + `DecompressionStream`, against
  project-owned CC0 fixtures in `web/public/__testpack__/`): valid/invalid
  manifest, per-item license override, lazy shard fetch+decompress+parse+lookup
  (worker), main-thread fallback, missing-preset/missing-shard/missing-manifest
  graceful degradation, navigation (incl. zero/one), favorites+recent, local
  single/multiple `.milk` import + reload persistence, zero-pack operation, and a
  **privacy check that imported content generates 0 upload requests**.

## Phase 10A.3 — Shader + Scene content library

Adds real Shader and Scene content to the unified library (`web/src/library/
{builtins,content}.ts`, `web/src/library-panel.ts`). No third-party shader
content is added.

### Built-in shader library

The project-owned examples (`examples.ts` single-pass, `multipass-examples.ts`
multipass) become complete `LibraryItem<ShaderPayload>` entries via
`builtinShaderItems()`. **13 built-ins** currently (10 single-pass + 3 multipass).
Stable ids `builtin:shader:<slug>` (never an array index); `origin:'builtin'`,
`license:'LGPL-2.1'`, structure-derived tags (`single-pass`/`multipass`/
`audio-reactive`/`feedback` — no fabricated/subjective metadata). Built-ins live
in memory; only ones the user touches (favorite/use/duplicate) are upserted to
IndexedDB.

### Built-in shader licensing/provenance

All built-in examples are **original, project-owned** content (LGPL-2.1 per their
file headers) — **not** Shadertoy/third-party-derived. No example has unclear
provenance; none were deleted. No third-party shader with unclear redistribution
rights is added.

### ShaderProject preservation

A shader payload mirrors the engine's `SourceState::Shader`
(`{source, mode, controls, mods, attribution?, passes}`) — full multipass state
(Image + Buffer A–D, per-pass source/mode/channels). **No second shader schema.**
Built-in payloads omit `attribution` (the engine's `Attribution` has a serde
default and required fields our library metadata doesn't carry); display
attribution lives on the LibraryItem. User-saved payloads round-trip the engine's
own attribution verbatim.

### Transactional shader/scene loading

Both shader and scene loads reuse the engine's **transactional** `import_scene`
(validates + applies atomically; retains the current scene on failure):

- **Load scene** → `import_scene(payload)` directly.
- **Load shader** → replace the current scene's shader-layer source with the
  payload (or append a shader layer), then `import_scene` the result. A
  structurally-invalid payload is rejected *before* touching the engine
  (`isValidShaderPayload`); a valid-but-bad-GLSL payload keeps each pass's
  last-known-good. Either way the master output is never blacked out or corrupted.

Save actions (`saveCurrentShader`/`saveCurrentScene`) never modify the running
shader/scene. `recordUsage` (metadata-only) updates `lastUsed`/`usageCount` on
load. Favorites/collections work through the 10A.1 metadata path (no payload
rewrite). Built-in entries are read-only (rename/delete refused; duplicate copies
them into a user entry).

### Scene/library schema distinction

Two independent versions, kept separate: the **library** DB/item schema
(`LIBRARY_DB_VERSION` / `LIBRARY_ITEM_SCHEMA_VERSION`) versions the envelope +
IndexedDB stores; the **scene** payload carries its **own** `SceneData
schema_version` and is stored **verbatim**. The library never reinterprets scene
internals.

### Attribution behavior

Shader items carry `author`/`license`/`attribution` metadata; scenes are stored
verbatim (existing per-layer shader `Attribution` is preserved inside the
SceneData). A scene that references library content currently embeds the source
state (as the engine's export does) rather than a lightweight reference — so an
exported scene keeps its legally-relevant per-layer attribution. **Limitation:**
until pack-reference-in-scene lands (a later deck-aware schema change), a scene
built from a large pack preset would embed that preset's text; 10A.3 does not
change the deck-aware scene schema.

### Search-ready + thumbnails

Shader/scene entries expose lightweight metadata (name/author/description/tags/
license/attribution) for 10A.4 search — no full-payload search. `thumbnailRef` is
supported but **no thumbnail generation** happens in 10A.3 (placeholder/none);
generation is deferred to 10A.4. No offscreen compositors are spun up for
browsing.

### Minimal UI

`web/src/library-panel.ts` — a small, lazily-loaded left-docked **Library** panel
(built-in shader list + Save Shader/Save Scene + a user list with Load/Favorite/
Rename/Duplicate/Delete). Reuses the existing panel patterns; intentionally
minimal so the full 10A.4 virtualized browser can supersede it cleanly.

### Zero-Milkdrop

The shader+scene library is fully functional with 0 Milkdrop packs and 0 imported
presets (built-in shaders + user shaders + saved scenes). Tested.

### Tests

`web/verify-content.mjs` (Playwright, real engine + IndexedDB) — 26 checks:
built-in enumeration/stable-ids/metadata, single-pass + multipass load,
save/load user shader, rename/duplicate/delete (built-ins protected), favorites,
**invalid shader → rejected, active visual retained**, scene save/load,
**invalid scene → rejected, current scene intact**, collections (shader+scene),
recent/usage, zero-Milkdrop, reload persistence, and **0 upload requests**.

## Phase 10A.4 — unified library browser

A fast, performance-oriented content browser over the 10A.1–10A.3 foundation
(`web/src/library-browser.ts`, opened from the **Library** toolbar button).

### UX / views

One left-docked panel with a search box, **view tabs** (`All`, `Milkdrop`,
`Shaders`, `Scenes`, `Favorites`, `Recent`, `Collections`), a Milkdrop
`Prev/Random/Next` bar (shown for All/Milkdrop/Favorites/Recent), an
`Import .milk` button, a virtualized result list, and a details/actions panel.

### One query pipeline

`applyFilter()` is the single predictable pipeline: **view filter → search →
sort**. It runs over a **lightweight in-memory aggregate** (`deps.collect()` =
built-in shaders ⊕ Milkdrop pack index ⊕ user/imported/saved items, deduped by
id with store state overriding). **Browse never loads a heavy payload** —
ShaderProject/SceneData/`.milk` text are fetched only on Load.

### Search / filters

Substring search over `name`/`author`/`tags`/`license`/`attribution`/
`description` (name/author/tags weighted by inclusion), combined cleanly with the
active view. Responsive over a synthetic **10,000-item** set (tested).

### Virtualization

Fixed row height + a windowed render: a spacer sized to `results·rowHeight`, and
only the visible range (± buffer) is in the DOM. Verified **bounded DOM (< 60
rows)** at 10k results, on initial render and after scrolling to the end.

### Favorites / Recent / Collections

Favorite toggles inline (metadata-only, persists, no payload rewrite). Recent =
items with `lastUsed`, sorted desc, updated on Load. Collections: create/rename/
delete (deleting a collection **keeps** its items) + add/remove membership; a
collection bar filters the view.

### Item rows + details + context actions

Each row: type-colored placeholder thumbnail + name + type badge + favorite +
Load. The details panel shows name/type/author/description/tags/license/
attribution/source/origin/texture-requirement (only known fields; nothing
invented). Actions are **context-sensitive**: built-ins get Load/Favorite/
Duplicate (no destructive rename/delete); user Shader/Scene get Rename/Duplicate/
Delete; pack/imported Milkdrop get Load/Favorite/Add-to-Collection (+ Delete for
imported).

### Load semantics (no preview yet)

Load applies to the **active** visual — it is **not** a preview. Routing:
Milkdrop → lazy `presetText` → the new transactional `load_preset` wasm export
(crossfades via `PresetPlayer::switch_to`; a parse failure keeps the current
preset); Shader → `content.loadShader` (transactional); Scene →
`content.loadScene`/`import_scene` (transactional). **A failed load preserves the
current master visual** (tested). True non-destructive preview is deferred to
10B (after the 10C.1 deck abstraction).

### Milkdrop navigation

`Random/Next/Previous` operate over the **current filtered Milkdrop result set**
(e.g. within a search or the Favorites view); deterministic no-op on zero
results.

### Thumbnails / empty states / mobile / a11y

Thumbnails are **type-specific placeholders** only (no generation, no per-card
renderers — deferred to a focused follow-up). Empty states are helpful per view
("Import .milk files…", "Save the current scene…", "No favorites yet…"). Mobile:
responsive width, touch-tappable rows/actions, no horizontal overflow (tested at
390px). Accessibility: focusable listbox with roles, `aria-selected`,
`aria-pressed` favorite buttons, labelled actions, type conveyed by text badge
(not color alone); keyboard `↑/↓` select, `Enter` load, `F` favorite — suppressed
while a text input/editor is focused.

### Pack-unavailable

If a pack shard can't be fetched, browse metadata stays visible, Load reports the
preset is unreachable, the renderer stays alive, and shaders/scenes/imports keep
working (tested).

### Tests

`web/verify-browser.mjs` — 16 checks: views, empty states, search
(name/author/tag/zero), real Milkdrop load, **10k virtualization + bounded DOM**,
large-set search, pack-unavailable graceful, favorites, recent, collections
(add/remove/delete-keeps-items), shader+scene load routing, mobile no-overflow,
favorite persistence across reload, 0 uploads.

## Phase 10 implementation ordering (current)

```
10A.1 Library foundation      ✓ merged
10A.2 Milkdrop library         ✓ merged
10A.3 Shader/Scene library     ✓ merged
10A.4 Library browser          ← this PR
10C.1 Deck abstraction         (before Preview)
10B   Preview/Audition
10C.2 Crossfader
10C.3 MIDI/keyboard
10D   Dual Milkdrop
```

## Tests (10A.1)

- `web/verify-library.mjs` (Playwright, real IndexedDB): CRUD, type round-trip
  (milkdrop ref / shader / scene), favorites-persist-across-reopen, recent
  ordering + usage counters, collection membership add/remove, non-destructive
  migration harness, corrupt-record isolation, zero-corpus init, and
  persistence across a full page reload — plus assertions that the renderer keeps
  advancing and no console errors occur.
