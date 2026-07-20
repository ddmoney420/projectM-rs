// Phase 10A.3 — promote the project-owned shader examples into LibraryItems.
//
// The single-pass (`examples.ts`) and multipass (`multipass-examples.ts`)
// examples are ORIGINAL, project-owned content (LGPL-2.1, per their file
// headers) — not third-party/Shadertoy-derived. Each becomes a complete
// LibraryItem<ShaderPayload> preserving full multipass state. Payloads mirror
// the engine's SourceState::Shader (`{source, mode, controls, mods, attribution,
// passes}`), so no second shader schema is introduced.

import { EXAMPLES } from '../examples';
import { MULTIPASS_EXAMPLES } from '../multipass-examples';
import { Attribution, LibraryItem, LIBRARY_ITEM_SCHEMA_VERSION, ShaderPayload, StableId } from './types';

const BUILTIN_LICENSE = 'LGPL-2.1';
const BUILTIN_AUTHOR = 'projectM-rs';
const BUILTIN_ATTR: Attribution = {
  author: BUILTIN_AUTHOR,
  license: BUILTIN_LICENSE,
  attributionText: 'Original example authored for projectM-rs (LGPL-2.1)',
};

interface PassLike {
  pass_type: string;
  enabled: boolean;
  source: string;
  mode: number;
  channels: [string, string, string, string];
}

// Built-in payloads OMIT `attribution` — the engine defaults it and its
// Attribution struct requires fields our library metadata doesn't carry. Display
// attribution lives on the LibraryItem (BUILTIN_ATTR) instead.
function singlePassPayload(source: string, mode: 'shadertoy' | 'raw'): ShaderPayload {
  return { source, mode: mode === 'raw' ? 1 : 0, controls: [], mods: [], passes: [] };
}

function multipassPayload(passes: { type: string; source: string; channels: [string, string, string, string] }[]): ShaderPayload {
  const image = passes.find((p) => p.type === 'image') ?? passes[passes.length - 1];
  const mapped: PassLike[] = passes.map((p) => ({ pass_type: p.type, enabled: true, source: p.source, mode: 0, channels: p.channels }));
  return { source: image.source, mode: 0, controls: [], mods: [], passes: mapped };
}

/** Structure-derived technical tags only (no fabricated/subjective metadata). */
export function shaderTags(payload: ShaderPayload): string[] {
  const tags: string[] = [];
  const passes = payload.passes as PassLike[];
  tags.push(passes.length > 0 ? 'multipass' : 'single-pass');
  const channelAudio = passes.some((p) => Array.isArray(p.channels) && p.channels.includes('audio'));
  const sampleAudio = /iChannel0/.test(payload.source) && passes.length === 0; // single-pass audio texture is iChannel0
  if (channelAudio || sampleAudio) tags.push('audio-reactive');
  if (passes.some((p) => Array.isArray(p.channels) && p.channels.includes('self'))) tags.push('feedback');
  return tags;
}

function makeBuiltin(name: string, payload: ShaderPayload): LibraryItem {
  return {
    id: StableId.builtin('shader', name),
    type: 'shader',
    name,
    author: BUILTIN_AUTHOR,
    tags: shaderTags(payload),
    collections: [],
    favorite: false,
    dateAdded: 0, // stable/deterministic for built-ins
    usageCount: 0,
    license: BUILTIN_LICENSE,
    attribution: { ...BUILTIN_ATTR },
    origin: 'builtin',
    schemaVersion: LIBRARY_ITEM_SCHEMA_VERSION,
  };
}

/** All project-owned built-in shader library items (single-pass + multipass). */
export function builtinShaderItems(): { item: LibraryItem; payload: ShaderPayload }[] {
  const out: { item: LibraryItem; payload: ShaderPayload }[] = [];
  for (const ex of EXAMPLES) {
    const payload = singlePassPayload(ex.source, ex.mode);
    out.push({ item: makeBuiltin(ex.name, payload), payload });
  }
  for (const ex of MULTIPASS_EXAMPLES) {
    const payload = multipassPayload(ex.passes);
    out.push({ item: makeBuiltin(ex.name, payload), payload });
  }
  return out;
}

/** Structural payload validation — a load must reject a corrupt shader payload
 *  BEFORE touching the engine (so the active visual is retained). */
export function isValidShaderPayload(x: unknown): x is ShaderPayload {
  if (!x || typeof x !== 'object') return false;
  const o = x as Record<string, unknown>;
  return (
    typeof o.source === 'string' &&
    typeof o.mode === 'number' &&
    Array.isArray(o.passes) &&
    Array.isArray(o.controls) &&
    Array.isArray(o.mods)
  );
}
