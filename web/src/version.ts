// Application version metadata, surfaced under About / Diagnostics.
//
// Release identity is injected at BUILD time (Vite `define`, see vite.config.ts)
// from the exact release being built — never derived from current `main` and
// never fetched over the network. A tagged production build embeds its release
// tag + dereferenced commit; an untagged dev build shows `<base>-dev` + the
// working commit and never pretends to be a release.
//
// `PRODUCT_BASE_VERSION` is the single source of truth for the current product
// minor line; bump it once per product milestone (Phase 10 → 0.10.0). The beta
// suffix comes from the git tag, not a manual edit here.

import { resolveDisplayVersion } from './version-tag';

declare const __APP_RELEASE_TAG__: string;
declare const __APP_GIT_COMMIT__: string;

/** Current product minor line (the only hard-coded version base). */
export const PRODUCT_BASE_VERSION = '0.10.0';

const releaseTag = typeof __APP_RELEASE_TAG__ === 'string' ? __APP_RELEASE_TAG__ : '';
const isDev = (import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV === true;

const resolved = resolveDisplayVersion({ releaseTag, productBase: PRODUCT_BASE_VERSION, isDev });

/** User-facing version, e.g. "0.10.0-beta.1" (release) or "0.10.0-dev" (dev). */
export const APP_VERSION = resolved.version;
/** Short git commit of the exact build (dereferenced release commit if tagged). */
export const GIT_COMMIT = (typeof __APP_GIT_COMMIT__ === 'string' ? __APP_GIT_COMMIT__ : '').slice(0, 7) || 'unknown';
/** The release tag this build was produced from, or '' for a dev build. */
export const RELEASE_TAG = releaseTag;
/** 'development' | 'production' (build mode, not release identity). */
export const BUILD_MODE = isDev ? 'development' : 'production';
