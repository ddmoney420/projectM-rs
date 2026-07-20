// Release-tag parsing for the canonical product-version convention.
//
// Canonical (Phase 10 onward):  v<major>.<minor>.<patch>[-beta.<n>]
//   v0.10.0-beta.1  → product version 0.10.0-beta.1
//   v0.10.0         → product version 0.10.0
// Legacy (immutable history):   v0.0.3-web-beta.<n>
//   preserved verbatim as a *historical* release identity — NEVER reinterpreted
//   as 0.10.0-beta.<n>.
//
// Pure + dependency-free so it is unit-testable under `node --test`.

export type ReleaseKind = 'beta' | 'stable' | 'legacy';

export interface ParsedRelease {
  /** The git tag as given (e.g. "v0.10.0-beta.1"). */
  tag: string;
  /** User-facing product version (no leading "v"), e.g. "0.10.0-beta.1". */
  version: string;
  /** The X.Y.Z base, e.g. "0.10.0". */
  base: string;
  kind: ReleaseKind;
  /** True for the historical v0.0.3-web-beta.N line. */
  legacy: boolean;
  /** Beta number when kind === 'beta'/'legacy', else undefined. */
  beta?: number;
}

const CANONICAL = /^v(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$/;
const LEGACY = /^v(\d+\.\d+\.\d+)-web-beta\.(\d+)$/;

/** Parse a release tag into product-version metadata, or null if malformed.
 *  Never throws; a malformed tag returns null (callers must not silently accept
 *  it as a release). */
export function parseReleaseTag(tag: string): ParsedRelease | null {
  if (typeof tag !== 'string' || tag.length === 0) return null;
  const legacy = LEGACY.exec(tag);
  if (legacy) {
    const base = legacy[1];
    const beta = Number(legacy[2]);
    return { tag, version: `${base}-web-beta.${beta}`, base, kind: 'legacy', legacy: true, beta };
  }
  const m = CANONICAL.exec(tag);
  if (!m) return null;
  const base = `${m[1]}.${m[2]}.${m[3]}`;
  if (m[4] !== undefined) {
    const beta = Number(m[4]);
    return { tag, version: `${base}-beta.${beta}`, base, kind: 'beta', legacy: false, beta };
  }
  return { tag, version: base, base, kind: 'stable', legacy: false };
}

/** The user-facing version string for a build, given the injected build metadata.
 *  A tagged build shows the release version; an untagged (dev) build shows
 *  `<base>-dev` — it must never fabricate a beta number. */
export function resolveDisplayVersion(opts: {
  releaseTag: string;
  productBase: string;
  isDev: boolean;
}): { version: string; kind: ReleaseKind | 'dev' } {
  const parsed = opts.releaseTag ? parseReleaseTag(opts.releaseTag) : null;
  if (parsed) return { version: parsed.version, kind: parsed.kind };
  return { version: `${opts.productBase}-dev`, kind: 'dev' };
}
