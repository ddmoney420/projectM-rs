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

## Phase 10C.1 — performance-deck abstraction

Refactors the visual-output path behind a reusable `Deck` (crates/pm-web-vj/
src/lib.rs). **Equivalence-first:** the full WebGPU regression (88/88) and all
library suites pass unchanged; perf is flat (fps 32.3 / cpuMs 2.22 vs 32 / 2.26).

### PerformanceDeck contract

```
Deck { id, player: PresetPlayer, compositor: Compositor, output: Texture,
       format, loaded? }
  → render(ctx, time, audio, uniforms, modctx)  writes to `output`
```
A deck is **one visual source** (a Milkdrop `PresetPlayer` + a layer
`Compositor`) rendered to **its own output texture** — never the surface. The
master output is currently `deck_a.output` blitted to the surface by a master
`Blit`.

### Visual source model

A deck's compositor already unifies the three source types — the "source" is
whatever the compositor holds:
- **Milkdrop** → the deck's `PresetPlayer` feeds the compositor's Milkdrop layer.
- **ShaderProject** → shader layer(s) with full multipass state (Image + Buffer
  A–D + channels + controls + modulation; per-pass last-known-good intact).
- **Scene** → the compositor's whole layer stack (via `import_scene`).

`Deck::source_type()` reports `milkdrop | milkdrop+shader | shader | overlay` for
diagnostics.

### Shared vs deck-local state

| Shared / Master (State) | Deck-local (Deck) |
|---|---|
| wgpu Device + Queue | loaded content + `PresetPlayer` (Milkdrop temporal state) |
| pm-audio analysis (PCM/tempo/LFO) | `Compositor` (layers, effects, shader/scene feedback) |
| recording + projection (mirror the surface) | deck output texture |
| master diagnostics + `master_blit` | deck resize / temporal isolation |

Device/Queue and audio analysis are **never** duplicated (audio uniforms/features
are computed once and passed to every deck).

### Output-texture contract

Every deck exposes a `Texture` in the **surface format** at the surface size, so a
future `MasterCrossfade.draw(deck_a.output, deck_b.output, t)` needs no redesign.
`resize()` recreates the deck output (clamped to `max_texture_dimension_2d`),
rebuilds the Milkdrop engine (matching prior resize behavior), and resizes the
compositor (shaders retained). Ownership/lifetime: the deck owns its output; it is
recreated on resize and dropped with the deck.

### Lifecycle + isolation

Deck B (`deck_b: Option<Deck>`) can be created / loaded / unloaded independently.
Verified: **Deck B create/load/unload never changes Deck A** (layer/shader
counts, temporal state), a Deck B load failure/oddity keeps the renderer alive
and Deck A intact, and a canvas resize with Deck B present rebuilds both without
crashing (0 GPU errors). Deck B renders every frame to its own texture (isolated
feedback) but is **not blended** into the visible master yet.

### Effects ownership (current + future)

Effects currently live **inside the compositor** (per-layer + a global chain) —
i.e. **deck-local**, applied to the deck output. This is unchanged (identical
visible behavior). Future model: **deck effects → deck output; master effects →
post-crossfade output**. No global effect was moved to a master stage in 10C.1.

### Master output / recording / projection

Master = Deck A's texture blitted to the surface (10C.2 replaces this blit with
the crossfade). Recording and projection **mirror the surface**, so they follow
master with no coupling to deck internals and will record/mirror the future
crossfade output unchanged.

### SceneData vs future PerformanceSession

`SceneData` is unchanged — it still serializes **one** visual (a single deck's
compositor). Deck A/B + master state are **not** forced into SceneData. A future
`PerformanceSession { schemaVersion, deckA, deckB, crossfader, masterEffects }`
(separate from SceneData) will persist a whole performance setup without
corrupting existing scene semantics. **Not implemented in 10C.1.**

### Dual-Milkdrop readiness

Each deck already owns an independent `PresetPlayer` (Deck B loads its own preset
via `deck_b_load_preset`, isolated from Deck A) — so simultaneous
Milkdrop↔Milkdrop is structurally ready. Benchmarks, mobile policy, and GPU/memory
validation/productionization remain **10D**.

### Mobile / failure handling

`deck_b_create` returns a boolean and never crashes the device; a constrained
device that can't allocate a second deck keeps Deck A alive and the renderer
running (tested at a 390px viewport). Max-texture-dimension safeguards apply to
every deck.

### Diagnostics + tests

`deck_diagnostics_json` reports per-deck `{id, loaded, sourceType, width, height,
format, layerCount, shaderCount, bufferPasses, shaderPasses}` and `deckCount`.
`web/verify-deck.mjs` (16 checks): Deck A equivalence + output contract,
source switching (shader/scene, failed-switch-preserves), Deck B
create/load/unload isolation, both-output compatibility, resize resilience with
Deck B, mobile-graceful, 0 uploads.

## Phase 10B — Preview / Audition

Lets a performer prepare the next visual on the **inactive** deck without
disturbing the live master (`crates/pm-web-vj/src/lib.rs`, `web/src/
library-browser.ts`). **Master output stays Deck A throughout 10B.**

### Preview vs Audition

- **Preview** — a lightweight visual inspection: the audition **monitor** (a
  small on-screen canvas showing Deck B) and type placeholders in the browser.
  Not the authoritative live state.
- **Audition** — load + run the selected item in **Deck B** (the inactive/
  audition deck), which renders independently while Deck A stays live. For
  Milkdrop, audition on Deck B is the authoritative prep path (no third engine
  for a thumbnail).

### Live (Deck A) vs Audition (Deck B)

`Load` → live Deck A (master); `Audition` → Deck B (offscreen). The distinction is
explicit in the UI: a **LIVE / AUDITION** status bar names what's loaded in each
deck, using the words *Live* and *Audition* (not just A/B). Deck B is never shown
to the audience during 10B.

### Audition routing (transactional)

- Milkdrop → lazy `presetText` → `deck_b_load_preset` (Deck B's PresetPlayer).
- Shader → wrap the full `ShaderProject` in a one-layer scene → `deck_b_import_scene`
  (full multipass preserved).
- Scene → `deck_b_import_scene(SceneData)`.

Each is transactional: a failure keeps Deck B's previous content (and never
touches Deck A / master / recording / projection). Deck B is created on first
audition.

### Preview monitor architecture

`preview_attach(canvas)` creates a **second on-screen surface** and, each frame,
**blits Deck B's already-produced output texture** to it (`render_preview`) — no
extra deck execution, no GPU→CPU readback. It is **not** the master surface.
Deck B renders at **master resolution** (for seamless future crossfading); only
the monitor *display* is small. `preview_detach()` releases it; a device that
can't create a second surface returns `false` and the app is unaffected.

### Warm inactive deck

Deck B keeps advancing every frame while auditioning (Milkdrop/shader/scene
feedback stays warm), so a future crossfade starts from an already-active visual
rather than cold.

### Preview Bank

An ordered performance queue of **LibraryItem id references only** (persisted in
the 10A.1 `previewBank` store; survives reload). Actions: Add / Remove / Move
Up/Down / Clear / Audition / Prev-Next-audition. Missing referenced items are
**skipped gracefully** (never crash init); reorder/remove operate by id. One
entry per id. Reachable by button (not drag-only) for touch.

### Library actions

Rows gain **Audition** (quick, → Deck B) alongside **Load** (→ live Deck A) and
Favorite; the details panel adds **Audition** + **Add to Bank**. Keyboard (when
the list is focused, never in a text input/editor): `P` audition, `B` add-to-bank,
`[`/`]` prev/next-audition.

### Recording / projection isolation

Recording and projection **mirror the surface = master = Deck A**. Deck B /
audition never leak into the recording or the projector during 10B (the future
crossfade output records/projects naturally in 10C.2). Verified: audition changes
never alter Deck A's diagnostics/output.

### Shared audio analysis

Both decks react to the **same** canonical pm-audio analysis (computed once,
passed to each deck) — no pm-audio duplication. Per-deck audio is Phase 11.

### Failure / mobile guardrails

Deck B create/audition failures, missing packs/shards, and second-surface
allocation failures all keep **Deck A live, master visible, recording/projection
active, renderer alive** (tested at 390px). A constrained device that can't
sustain Deck B fails cleanly with a message; Deck A is never destroyed to make
room. Dual-Milkdrop on mobile remains 10D.

### Performance (early measurement)

CPU-in-render (stress-adjacent): Deck A only **2.45 ms**; Milkdrop live + Shader
audition + monitor **3.06 ms**; Milkdrop + Milkdrop audition + monitor **3.48
ms** — all far under a 16 ms budget; fps holds at display rate. This is an early
10B measurement, **not** 10D's full dual-Milkdrop production benchmark.

### Tests

`web/verify-audition.mjs` (15 checks): audition Milkdrop/Shader/Scene with Deck A
unchanged, failed-audition-preserves, master isolation, warm state, clear
audition, Preview Bank add/reorder/missing-skip/clear/persist-reload, preview
monitor attach, mobile no-overflow, 0 uploads.

## Phase 10C.2 — master A/B crossfader

Blends the two decks into the master (`crates/pm-web-vj/src/lib.rs`,
`web/src/library-browser.ts`). **Linear mode only** (luma/wipe/glitch/radial/
additive are future).

### Architecture

```
Deck A ─┐
        ├→ MasterCrossfade (mix) → master → Surface / Recording / Projection
Deck B ─┘
Deck B ───────────────────────────────────→ Preview monitor (raw, unchanged)
```
Reuses the existing `pm_core::Crossfade` primitive (`draw(from, to, t, output)` =
`mix(a.rgb, b.rgb, t)`, alpha 1.0) — no duplicate blend path. The blend renders
into a `master` texture (`TARGET_FORMAT` = `Rgba8Unorm`, linear), which
`master_blit` copies to the surface. **Endpoint bypass:** at `t == 0` (or Deck B
absent) the surface blits **Deck A directly** — byte-identical to the pre-10C.2
single-deck path (the 88/88 regression is unchanged); at `t == 1` it blits Deck B
directly; only the middle runs the blend. Blending happens in linear space
(sample → mix → write), so it is **gamma-correct** — no darkening/premultiplication
(measured luma A 103 / mid 61 / B 27, a clean midpoint; no value below the darker
endpoint or above the brighter one).

### Semantics + deck identity

`crossfader ∈ [0,1]` (clamped): **0.0 = 100% Deck A, 0.5 = linear A/B, 1.0 = 100%
Deck B**; default **0.0**. Deck identities are **permanently stable** — reaching
an endpoint never swaps A and B, and never changes what is loaded in either deck.
Runtime performance state only: **never serialized into SceneData** (belongs to
the future `PerformanceSession`). Exports: `set_crossfader`/`crossfader`;
reported in `deck_diagnostics_json`.

### Required behaviors

- **Empty/absent Deck B** → master shows Deck A at any `t` (never fades to black).
- **Clear Deck B** → the fader is reset to **0 (100% A)** *before* Deck B unloads,
  so the master is never left mixing toward an absent deck.
- **Audition** loads Deck B **without** moving the fader (prep stays hidden until
  you fade).
- **Direct Library Load** always targets **Deck A** (the live deck). If the fader
  is currently toward B, a Load still replaces Deck A — which is only partially or
  not visible until you fade back toward A; this is intentional and documented.
- **Warm decks:** both decks render every frame at `t=0` and `t=1` (Milkdrop /
  shader / scene feedback stays warm for an instant blend).
- **Transactional:** a failed Deck A replacement keeps the previous Deck A; a
  failed Deck B audition keeps the previous Deck B.
- **Recording + projection** mirror the surface = **post-crossfade master**.
- **Preview monitor** stays **raw Deck B** output (independent of the fader).

### UI / accessibility

An A◀▶B `<input type="range">` (min 0, max 1, step .01, default 0) in the library
panel — mouse, touch, and native keyboard (arrows/Home/End) with an
`aria-label`; a live `A / 42% / B` text readout (not color-only). The slider
re-syncs to the engine value after a Clear (which resets to A). Mobile-safe (no
horizontal overflow at 390px).

### Performance (CPU-in-render EMA, ms — GPU time not directly instrumented)

Deck A only `t=0` **2.67 ms**; Shader+Shader blend `t=0.5` **3.38 ms**;
Milkdrop+Milkdrop blend `t=0.5` **4.16 ms** — all far under the 16.67 ms/60 fps
budget; FPS holds at the display rate (the GPU-present-bound proxy — WebGPU
timestamp queries are not wired in this harness, so only CPU-in-render is a
measured number). This distinguishes **CPU frame timing** (measured) from **GPU
timing** (proxied by sustained FPS).

### Dual-Milkdrop status

Milkdrop↔Milkdrop crossfading is **functionally working** (two independent
`PresetPlayer`s blended, ~4.16 ms CPU) but **not declared production-ready** —
extended GPU-memory headroom and mobile validation remain **Phase 10D**.

### Tests

`web/verify-crossfader.mjs` (18 checks): set/clamp (0/0.5/1/over/under), deck
identity stable across endpoints, master isolation, warm decks at endpoints,
**color** (endpoints differ, middle is a between-endpoints blend, never black —
via real master-canvas PNG luma), empty-Deck-B-not-black, clear-resets-fader,
rapid 0↔1 + random values, resize during blend. Combinations exercised across the
suite: Shader↔Shader, Shader↔Scene, Milkdrop↔Shader, Milkdrop↔Milkdrop
(+ Scene↔Scene via scene decks).

## Phase 10C.3 — MIDI + keyboard performance controls

Makes the deck/audition/crossfader workflow performable without constant
browser-UI clicks (`crates/pm-web-vj/src/lib.rs`, `web/src/performance.ts`).
**Extends** the existing MIDI system — no parallel mapping database, no schema
change (existing mappings keep working; the 88/88 regression, which includes the
MIDI learn/pickup/momentary tests, is unchanged).

### Continuous targets vs actions

- **Continuous target** — a value across a range with soft-takeover; e.g. the
  master crossfader. Dispatched through `midi_set_value` / read via
  `midi_current` (the existing continuous path).
- **Trigger action** — a one-shot command. Implemented via the existing `app.*`
  target convention: `midi_fire` strips the `app.` prefix and queues the action
  string, which `midi_take_actions()` drains and JS routes to the command layer.
  **No action pretends to be a knob** — actions are `trigger` kind and the MIDI
  panel labels them `· action` vs `· param`.

### Master crossfader target

Stable id **`global.crossfader`** (group *Performance*, continuous, 0→1, 0=Deck
A / 1=Deck B). It maps to the **same** runtime `self.crossfader` as
`set_crossfader` — one source of truth, no second state. Participates in MIDI
Learn and the versioned mapping store like any continuous target.

### Soft takeover

Because `global.crossfader` is continuous, it inherits the existing pickup logic
for free (`midi_current` returns the live value; `pickup_engage` gates the
mapping). A physical fader far from the software value does **not** jump the
master — it engages only after catching/crossing the software position. Tested
both directions (physical below and above software) and value endpoints
(0→0.0, 127→1.0). Pickup state re-arms on mapping edit/import (reconnect).

### Performance actions (stable ids)

Trigger targets (id = `app.performance.*`, action = `performance.*`):
`audition_selected`, `bank_next`, `bank_previous`, `bank_audition_next`,
`clear_audition`, `random_milkdrop`, `favorite_selected`, `mix_to_a`,
`mix_to_b`, `mix_center`. Ids are stable strings (never display labels).

### Trigger/edge semantics

Reuses the router's edge detection: a Note-On (velocity>0) fires once; a CC fires
on the **0→127 rising edge** only — a held 127 does **not** retrigger, and
returning to 0 re-arms. Tested (fire-once / held-no-retrigger / re-arm).

### Canonical command layer

`PerformanceActions` (`web/src/performance.ts`) is the **single** implementation
of every performance behaviour; UI buttons, the document keyboard handler, and
MIDI actions all call it. It owns the Preview-Bank cursor so "next/previous bank
item" means the same everywhere. Audition always flows through the transactional
audition path; the crossfader always flows through the one `setCrossfader`.

### Keyboard layer + focus safety

Document-level shortcuts: `[`/`]` previous/next bank item, `P` audition selected,
`R` random Milkdrop (auditions — never disrupts live), `1`/`2`/`3` mix to A /
center / B, `Shift+←/→` nudge the crossfader ±0.05 (clamped). A centralized
`shouldHandlePerformanceShortcut(e)` suppresses **all** of them while focus is in
an input/textarea/select/contenteditable/CodeMirror — typing is never
intercepted. The list-local keys (arrows/Enter/F/B) are kept disjoint from these
to avoid double-firing.

### Selected-item state

Three distinct notions, documented: **Library selection** (browser highlight →
Library actions), **Preview-Bank cursor** (owned by `PerformanceActions` → bank
actions), **Auditioned item** (Deck B loaded content). They never conflict.

### Audition never auto-crossfades

A MIDI/keyboard **Audition** loads Deck B **without** moving the crossfader —
preparing and bringing-live stay separate performance moves. `mix_to_a/b/center`
and the fader are the only things that move the master.

### UI / MIDI / keyboard synchronization

One setter (`set_crossfader`) is the source of truth. A MIDI-driven crossfader
change is reflected into the UI slider immediately (midiTick detects
`global.crossfader` in the update stream → re-syncs the slider + `A/%/B`
readout). Keyboard/UI changes go through the same setter, so soft-takeover sees
the new software position. No DOM state is mutated independently of engine state.

### Persistence / migration / multi-device / reconnect

Mappings persist in the existing `pm-web-midi-v1` store (`MappingSet` v1);
**no schema change** was needed (actions are ordinary `Trigger` mappings), so all
existing mappings are preserved with no destructive reset. Mappings remain keyed
by **device name + channel + selector** (empty device = any) — unchanged
multi-device policy. On reconnect the mapping set is retained and pickup re-arms.

### Web MIDI availability / mobile

Web MIDI is feature-detected (`'requestMIDIAccess' in navigator`); the panel
shows a clear unsupported state and never throws. Decks / crossfader / library /
audition all keep working without MIDI. Keyboard shortcuts are naturally absent
on touch-only devices; the touch crossfader + Library/Audition controls remain
fully usable. **MIDI/keyboard are enhancements — every action stays available via
accessible buttons.**

### Future extension points

The `path`-based registry + `app.*` action convention accommodate future Phase 11
ids (`deckA.audio.volume`, `audio.crossfader`, `stem.vocals.mute`,
`action.stem.vocals.toggle_mute`) and future deck actions (Load→Deck A/B, swap)
without a redesign — none implemented now; stable A/B identity remains the rule.

### Tests

`web/verify-perf-controls.mjs` (22 checks): crossfader target/actions in the
registry, continuous CC map + range + UI-follows, soft-takeover (below/above/
engage), action learn + fire-once + held-no-retrigger + re-arm, `mix_center`
action, keyboard 1/2/3 + nudge + input-suppression, bank MIDI audition + empty
no-op, backward-compat targets, mapping persistence, reconnect retention.

## Phase 10D — dual-Milkdrop / dual-deck productionization

Hardening + capability validation of the dual-deck architecture. **Measured on a
single desktop environment** (see matrix); mobile + physical-device validation is
explicitly pending.

### Dual-deck resource model + worst-case engine count

Render targets are now **measured** live, not hand-estimated: every
`pm-render::Texture` registers on construction and unregisters on `Drop`
(`live_texture_count` / `live_texture_bytes`), surfaced via `perf_telemetry_json`
and the diagnostics **decks** row. A leak shows up as a count that never returns
to baseline.

Worst case for Milkdrop↔Milkdrop is **not** two engines — each deck's
`PresetPlayer` runs a *second* `WarpEngine` during its own preset transition, so
with both decks mid-transition **plus** the master crossfade the pipeline holds
**up to four `WarpEngine`s + two intra-deck crossfades + one master crossfade**.
This is benchmarked directly (below), not just steady-state two-engine.

### CPU vs GPU timing (do not conflate)

`cpuMs` is **CPU submission / render-loop EMA** — flagged `cpuMsIsGpuTime:false`.
It stays **3–5 ms** even at 1440p dual-Milkdrop, which shows the app is
**GPU-bound, not CPU-bound** (cheap submission; the GPU does the work). **GPU
execution time is NOT inferred from it.** WebGPU `timestamp-query` **is available**
on the tested adapter (reported by `deck_capability_json`), but pass-level GPU
instrumentation was intentionally **not** wired into the hot loop (to avoid
destabilizing normal runtime per the brief); sustained FPS + frame-progress are
the honest GPU proxies used here.

### Measured resource estimates (dual Milkdrop, 51 live textures)

| Resolution | Live textures | Est. texture bytes (measured) | CPU-in-render |
|---|---|---|---|
| ~1100×620 | 51 | ~79 MB | 3–5 ms |
| 1280×720 | 51 | 101.5 MB | ~5.3 ms |
| 1920×1080 | 51 | 226 MB | ~3.2 ms |
| 2560×1440 | 51 | 400 MB | ~4.7 ms |

Bytes are `Σ w·h·depth·bpp` (base level, no mips) over all live textures — an
**estimate from measured live allocations**, not GPU-driver residency. It scales
~linearly with pixel count; 4K (~3840×2160) extrapolates to **~0.9 GB** and was
**not** tested. Single-deck baseline is **26 live textures**.

### Soak / leak findings (short CI run)

The bounded harness (`verify-qualification.mjs`, default 40 iterations of
create→load→crossfade→unload) returns to the **exact 26-texture baseline** (no
leak), keeps the render loop advancing, and logs **0 WebGPU errors / 0 WASM
panics / 0 console errors**. The harness accepts `SOAK_ITERS=…` for **long manual
soak** runs (15/30/60-minute continuous sessions) — those long runs were **not**
performed in this session and are left as a manual step.

### Dual-Milkdrop state isolation + concurrent transitions

Verified: loading a preset on Deck B leaves Deck A's source/layer state
unchanged, and vice-versa (independent presets, feedback buffers, render targets;
no shared-feedback contamination) at t=0/0.5/1. The worst-case four-engine run
(both decks transitioning + master mid-blend) was **stable** (max 67 live
textures, ~4.7 ms CPU, 0 errors). **Concurrent preset-transition policy:** given
the measured headroom, **fully-concurrent (Policy A)** intra-deck transitions are
retained on desktop — no constraint was needed; the capability tier + degradation
message (below) are the guard for weaker devices.

### Adaptive capability policy (no UA sniffing)

`deck_capability_json` reports the adapter's real `max_texture_dimension_2d`,
`max_buffer_size`, `timestamp-query` availability, and a **tier from limits**
(≥8192 → tier 1 *supported*, ≥4096 → tier 2 *limited*, else tier 3
*experimental*). The JS layer (`dualMilkdropAllowed`) combines the tier with a
runtime override — **never** a user-agent string. Only the true **Milkdrop↔
Milkdrop** case is gated; Milkdrop↔Shader/Scene and everything else is always
allowed. Physical render size is bounded by `max_texture_dimension_2d` (the
existing DPR-aware clamp), so `devicePixelRatio` cannot multiply allocation past
safe limits.

### Graceful degradation / allocation failure / device loss

When dual-Milkdrop is disallowed (constrained tier / allocation failure), an
audition of a second Milkdrop is **refused with a message** — *"Dual Milkdrop is
unavailable at the current device capability. Try a Shader or Scene on the
audition deck."* — **Deck A stays live, master stays valid, no black frame, no
panic**; a Shader/Scene audition still works (verified). `deck_b_create` /
`deck_b_load_preset` return status rather than throwing, so an allocation failure
degrades cleanly. Full WebGPU **device loss** surfaces via the `on_uncaptured_error`
"GPU error" diagnostics row; recovering from a true device-loss generally requires
a page reload (WebGPU does not offer transparent recovery) — documented, not
faked.

### Crossfade / recording / projection / MIDI under load

Rapid 0↔1 crossfading with both decks running + intra-deck preset swaps: **no
validation errors, no black frames, renderer keeps advancing**. Recording remains
the **post-crossfade master** and stays stable during dual-deck fading; projection
mirrors the **post-crossfade master** while the preview stays **raw Deck B**; a
sustained synthetic-MIDI audition/crossfade/bank loop keeps the renderer alive
with 0 errors. All measured on the desktop environment below.

### Tested hardware matrix

| | |
|---|---|
| OS | Windows 11 |
| Browser | Chrome (headed, real GPU via `--enable-unsafe-webgpu`), Playwright-driven |
| GPU | discrete NVIDIA (per the project's verified runtime env); `max_texture_dimension_2d` = 16384; `timestamp-query` available |
| Resolutions | ~1100×620 → 2560×1440 (dual-Milkdrop measured); 4K not tested |
| DPR | 1 (physical size clamped to max texture dim) |

**Only one desktop environment was available** — integrated-GPU, Apple-Silicon,
and multi-adapter results are **not** claimed.

### Support recommendation

- **Desktop dual-Milkdrop: SUPPORTED** (tier 1) on the tested adapter — no leak,
  stable four-engine worst case, 3–5 ms CPU through 1440p, comfortable GPU
  headroom. Extended long-soak (60-min) confirmation remains a manual step.
- **Mobile dual-Milkdrop: EXPERIMENTAL / pending** — only viewport-simulated
  mobile was run (390px); no physical device this session. The capability tier +
  degradation path already restrict/deny dual-Milkdrop and keep Deck A alive on
  weaker devices. **Physical iPhone/Android and physical-MIDI validation are
  PENDING** and must not be claimed as passed until actually performed.

### Tests

- `web/verify-qualification.mjs` (13 checks): capability report, dual-Milkdrop
  isolation, worst-case four-engine stability, allocation-failure degradation,
  resource-growth soak + **no-texture-leak** (baseline↔final), crossfade/resize/
  recording/MIDI under load, no console errors / no WASM panics. Short CI mode by
  default; `SOAK_ITERS` for long manual soak.
- `pm-render` unit tests: byte-per-pixel + estimated-bytes accounting (workspace
  now **277** = 275 baseline + 2).

## Phase 10 implementation ordering (current)

```
10A.1 Library foundation      ✓ merged
10A.2 Milkdrop library         ✓ merged
10A.3 Shader/Scene library     ✓ merged
10A.4 Library browser          ✓ merged
10C.1 Deck abstraction         ✓ merged
10B   Preview/Audition         ✓ merged
10C.2 Master crossfader         ✓ merged
10C.3 MIDI/keyboard controls    ✓ merged
10D   Dual-Milkdrop production   ← this PR (final Phase 10 engineering step)
```

Next decision (not part of 10D): whether the accumulated Phase 10 work ships as
`v0.0.3-web-beta.5`, or whether the version-alignment policy (git `v0.0.3-web-beta.x`
vs VJ `APP_VERSION 0.9.x`) is resolved first.

## Tests (10A.1)

- `web/verify-library.mjs` (Playwright, real IndexedDB): CRUD, type round-trip
  (milkdrop ref / shader / scene), favorites-persist-across-reopen, recent
  ordering + usage counters, collection membership add/remove, non-destructive
  migration harness, corrupt-record isolation, zero-corpus init, and
  persistence across a full page reload — plus assertions that the renderer keeps
  advancing and no console errors occur.
