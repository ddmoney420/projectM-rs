// Phase 10A.1 — unified content-library data model.
//
// A LibraryItem is a common envelope (metadata) plus a typed `payload` for one
// of three content kinds. Metadata and payload are stored in SEPARATE IndexedDB
// stores (see db.ts) so list/query/usage-bump operations never rewrite a heavy
// payload (a full multipass shader project or a scene can be hundreds of KB).
//
// Nothing here reinterprets the engine's existing serialization: a `scene`
// payload is exactly the JSON that pm-scene's SceneData produces/consumes, and a
// `shader` payload mirrors SourceState::Shader (source/mode/controls/mods/
// attribution/passes). We keep those compatible rather than inventing a second
// schema.

/** Bumped when the *item* shape (metadata) changes incompatibly. Distinct from
 *  the IndexedDB database version in db.ts. */
export const LIBRARY_ITEM_SCHEMA_VERSION = 1;

export type ContentType = 'milkdrop' | 'shader' | 'scene';

/** Where an item came from — governs redistribution + whether payload is a
 *  reference (bundled/pack) or owned inline (user/imported). */
export type Origin = 'builtin' | 'user' | 'imported' | 'pack';

/** Mirrors pm-scene `Attribution` (crates/pm-scene/src/lib.rs). All fields
 *  optional; an empty/absent `license` is deliberately NOT the same as an
 *  explicitly permissive one — unknown license ≠ redistributable. */
export interface Attribution {
  title?: string;
  author?: string;
  sourceUrl?: string;
  license?: string;
  licenseUrl?: string;
  modifiedFrom?: string;
  attributionText?: string;
}

// --- payloads -------------------------------------------------------------

/** A Milkdrop preset is either a REFERENCE into a preset-pack shard (bundled
 *  content is never copied into IndexedDB) or user-imported inline `.milk` text
 *  the user owns. 10A.1 defines the shape only; no corpus is loaded yet. */
export type MilkdropPayload =
  | { kind: 'pack'; packId: string; shard: string; path: string }
  | { kind: 'inline'; text: string };

/** Mirrors the serialized SourceState::Shader (pm-scene). Kept structurally
 *  compatible so a shader library entry can round-trip through the existing
 *  engine import/export without a second schema. */
export interface ShaderPayload {
  source: string;
  mode: number;
  controls: number[][];
  mods: unknown[];
  /** Engine-shaped attribution (SourceState::Shader.attribution). Optional — the
   *  engine defaults it, so built-in payloads omit it and carry display
   *  attribution on the LibraryItem instead. User-saved payloads round-trip the
   *  engine's own attribution object verbatim. */
  attribution?: Record<string, unknown>;
  passes: unknown[];
}

/** A scene payload is the opaque, versioned SceneData JSON exactly as the engine
 *  emits it (has its own `schema_version`). We store it verbatim. */
export type ScenePayload = { schema_version: number } & Record<string, unknown>;

export type LibraryPayload = MilkdropPayload | ShaderPayload | ScenePayload;

// --- item -----------------------------------------------------------------

/** Library metadata (the small, indexed record). The heavy `payload` lives in a
 *  separate store and is fetched only when an item is actually loaded. */
export interface LibraryItem {
  id: string;
  type: ContentType;
  name: string;
  author?: string;
  description?: string;
  tags?: string[];
  /** Multi-collection membership by collection id (see COLLECTIONS decision in
   *  docs/phase10-library.md): the item carries its collection ids and the
   *  `items` store has a multiEntry index on this field. */
  collections?: string[];
  favorite: boolean;
  dateAdded: number;
  lastUsed?: number;
  usageCount: number;
  thumbnailRef?: string;
  license?: string;
  attribution?: Attribution;
  origin: Origin;
  /** Item metadata schema version (for future metadata migrations). */
  schemaVersion: number;
}

/** A full item = metadata + its payload (joined across the two stores). */
export interface FullLibraryItem extends LibraryItem {
  payload: LibraryPayload;
}

export interface Collection {
  id: string;
  name: string;
  dateAdded: number;
}

/** Ordered list of item ids — the future Preview Bank stores REFERENCES only,
 *  never full payloads. 10B implements behavior; 10A.1 only reserves storage. */
export interface PreviewBank {
  id: string; // single well-known row, e.g. 'default'
  itemIds: string[];
}

export interface ThumbnailRecord {
  id: string; // == item id
  dataUrl: string;
  width: number;
  height: number;
  generatedAt: number;
}

// --- stable ids -----------------------------------------------------------

/** Stable id strategy. IDs must survive favorites/recents/collections/reload/
 *  restart, so they are derived from stable identity — never a display name or
 *  array index. */
export const StableId = {
  /** Bundled preset-pack entry: identity is (packId, path). */
  pack: (packId: string, path: string): string => `pack:${packId}:${path}`,
  /** Project-owned built-in (e.g. an example shader): identity is a slug. */
  builtin: (type: ContentType, key: string): string => `builtin:${type}:${slug(key)}`,
  /** User-created entry: a generated persistent id. */
  user: (type: ContentType): string => `user:${type}:${uuid()}`,
  /** User-imported third-party content the user now owns a copy of. */
  imported: (type: ContentType): string => `imported:${type}:${uuid()}`,
};

export function slug(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80) || 'item';
}

function uuid(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  // Fallback (non-secure contexts): time+random; ids need only be locally unique.
  return `${Date.now().toString(36)}-${Math.floor(Math.random() * 1e9).toString(36)}`;
}

// --- validation -----------------------------------------------------------

const CONTENT_TYPES: ReadonlySet<string> = new Set(['milkdrop', 'shader', 'scene']);
const ORIGINS: ReadonlySet<string> = new Set(['builtin', 'user', 'imported', 'pack']);

/** Structural validation used to ISOLATE corrupt records on read rather than let
 *  one bad entry break the whole library. Returns true for a usable metadata
 *  record. */
export function isValidItem(x: unknown): x is LibraryItem {
  if (!x || typeof x !== 'object') return false;
  const o = x as Record<string, unknown>;
  return (
    typeof o.id === 'string' &&
    o.id.length > 0 &&
    typeof o.type === 'string' &&
    CONTENT_TYPES.has(o.type) &&
    typeof o.name === 'string' &&
    typeof o.origin === 'string' &&
    ORIGINS.has(o.origin) &&
    typeof o.dateAdded === 'number' &&
    (typeof o.favorite === 'boolean' || typeof o.favorite === 'number') &&
    typeof o.usageCount === 'number'
  );
}
