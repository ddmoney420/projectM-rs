// Phase 10A.2 — preset-pack manifest format + validation + license classing.
//
// A pack is a machine-readable manifest (metadata + a flat item index) plus one
// or more gzipped-NDJSON shards holding the actual `.milk` text (the existing
// tools/preset-pack layout). The manifest is the index: it lets the library list
// thousands of presets WITHOUT downloading a single shard. Per-item fields
// override pack-level defaults, because a pack is not always one uniform license.
//
// LICENSING POSTURE: no third-party corpus is bundled or mirrored by this
// project. `assumed-public-domain` is represented DISTINCTLY from an explicit
// dedication (CC0) so the UI can never imply a stronger license than a manifest
// actually provides.

import { Attribution, ContentType, LibraryItem, LIBRARY_ITEM_SCHEMA_VERSION, MilkdropPayload, Origin, StableId } from './types';

export const PACK_MANIFEST_SCHEMA_VERSION = 1;

export interface PackItem {
  path: string;
  name: string;
  shard: string;
  author?: string;
  license?: string;
  attribution?: string;
  category?: string;
}

export interface PackManifest {
  packId: string;
  name: string;
  version: string;
  /** Pack-level default license string (e.g. 'CC0-1.0', 'LGPL-2.1',
   *  'assumed-public-domain', 'unknown'). Per-item `license` overrides it. */
  license: string;
  licenseUrl?: string;
  source?: string;
  attribution?: string;
  takedownContact?: string;
  requiresTextures?: boolean;
  items: PackItem[];
}

/** How the app should REGARD a license — deliberately keeps "assumed public
 *  domain" separate from an explicit dedication so nothing is overstated. */
export type LicenseClass =
  | 'project-owned'
  | 'explicitly-licensed'
  | 'assumed-public-domain'
  | 'user-imported'
  | 'unknown-license';

const EXPLICIT_SPDX = /^(cc0-1\.0|cc-by(-sa)?-4\.0|mit|apache-2\.0|lgpl-2\.1(-only|-or-later)?|gpl-3\.0|bsd-3-clause|unlicense)$/i;

/** Classify a (license string, origin) into how the app should regard it. */
export function classifyLicense(license: string | undefined, origin: Origin): LicenseClass {
  if (origin === 'imported' || origin === 'user') return 'user-imported';
  const l = (license ?? '').trim().toLowerCase();
  if (origin === 'builtin') return 'project-owned';
  if (l === 'assumed-public-domain' || l === 'assumed public domain') return 'assumed-public-domain';
  if (EXPLICIT_SPDX.test(l)) return 'explicitly-licensed';
  return 'unknown-license';
}

export interface ValidationResult {
  ok: boolean;
  errors: string[];
  manifest?: PackManifest;
}

/** Structurally validate an untrusted manifest object. Isolates a bad manifest
 *  rather than letting it break the library. */
export function validatePackManifest(x: unknown): ValidationResult {
  const errors: string[] = [];
  if (!x || typeof x !== 'object') return { ok: false, errors: ['manifest is not an object'] };
  const o = x as Record<string, unknown>;
  const str = (k: string) => (typeof o[k] === 'string' && (o[k] as string).length > 0);
  if (!str('packId')) errors.push('missing packId');
  if (!str('name')) errors.push('missing name');
  if (!str('version')) errors.push('missing version');
  if (typeof o.license !== 'string') errors.push('missing license (use "unknown" if none)');
  if (!Array.isArray(o.items)) {
    errors.push('items must be an array');
    return { ok: false, errors };
  }
  const items: PackItem[] = [];
  (o.items as unknown[]).forEach((raw, i) => {
    if (!raw || typeof raw !== 'object') {
      errors.push(`item[${i}] is not an object`);
      return;
    }
    const it = raw as Record<string, unknown>;
    if (typeof it.path !== 'string' || !it.path) {
      errors.push(`item[${i}] missing path`);
      return;
    }
    if (typeof it.shard !== 'string' || !it.shard) {
      errors.push(`item[${i}] missing shard`);
      return;
    }
    items.push({
      path: it.path,
      name: typeof it.name === 'string' && it.name ? it.name : it.path.split('/').pop()!.replace(/\.milk$/i, ''),
      shard: it.shard,
      author: typeof it.author === 'string' ? it.author : undefined,
      license: typeof it.license === 'string' ? it.license : undefined,
      attribution: typeof it.attribution === 'string' ? it.attribution : undefined,
      category: typeof it.category === 'string' ? it.category : undefined,
    });
  });
  if (errors.length) return { ok: false, errors };
  return {
    ok: true,
    errors,
    manifest: {
      packId: o.packId as string,
      name: o.name as string,
      version: o.version as string,
      license: o.license as string,
      licenseUrl: typeof o.licenseUrl === 'string' ? o.licenseUrl : undefined,
      source: typeof o.source === 'string' ? o.source : undefined,
      attribution: typeof o.attribution === 'string' ? o.attribution : undefined,
      takedownContact: typeof o.takedownContact === 'string' ? o.takedownContact : undefined,
      requiresTextures: o.requiresTextures === true,
      items,
    },
  };
}

/** Build a lightweight LibraryItem (metadata) + a reference payload from a pack
 *  item. NO preset text is included — it is fetched lazily from the shard. */
export function packItemToLibraryItem(
  manifest: PackManifest,
  item: PackItem,
): { item: LibraryItem; payload: MilkdropPayload } {
  const type: ContentType = 'milkdrop';
  const license = item.license ?? manifest.license;
  const attribution: Attribution | undefined =
    item.attribution || manifest.attribution || item.author
      ? { author: item.author, license, attributionText: item.attribution ?? manifest.attribution, sourceUrl: manifest.source }
      : undefined;
  return {
    item: {
      id: StableId.pack(manifest.packId, item.path),
      type,
      name: item.name,
      author: item.author,
      tags: item.category ? [item.category] : undefined,
      collections: [],
      favorite: false,
      dateAdded: Date.now(),
      usageCount: 0,
      license,
      attribution,
      origin: 'pack',
      schemaVersion: LIBRARY_ITEM_SCHEMA_VERSION,
    },
    payload: { kind: 'pack', packId: manifest.packId, shard: item.shard, path: item.path },
  };
}
