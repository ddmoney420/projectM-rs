# pm-web Deployment

The browser app is a **static site**. No backend is required. The only special
requirement is **cross-origin isolation** for the SharedArrayBuffer audio path.

## Required response headers

The app enables cross-origin isolation so the AudioWorklet ring buffer can use
`SharedArrayBuffer`. Serve every response (at least the HTML) with:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

(These are the exact values used by the dev/preview server — see
`web/vite.config.ts`.)

### Implications of `require-corp`
- Cross-origin **images, scripts, audio, and other subresources** must send
  `Cross-Origin-Resource-Policy` / proper CORS, or they will be blocked. The app
  bundles all of its assets, so this only matters if you add external resources.
- If you cannot set these headers, the app still runs — it falls back to a
  `postMessage` audio transport (no `SharedArrayBuffer`) with slightly higher
  latency. Rendering is unaffected. To ship without isolation, you may drop the
  headers, but keeping them is recommended.

## Build

```bash
cd web
npm install
npm run build     # wasm-pack (release) + vite build → web/dist/
```

`web/dist/` is the deployable static output. It contains `index.html`,
`output.html` (the projection window), the wasm module, and JS chunks
(`main` + lazy `shader-console` / `midi` chunks).

## Static hosts

Any static host that lets you set custom response headers works. Notes:

- **Cloudflare Pages** — add a `_headers` file to the deploy root:
  ```
  /*
    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp
  ```
- **Netlify** — add a `_headers` file (same content) or configure `netlify.toml`
  `[[headers]]`.
- **GitHub Pages** — does **not** support custom response headers, so
  cross-origin isolation cannot be enabled there; the app will run with the
  `postMessage` audio fallback. Prefer a host with header control for the best
  audio path.

Serve `index.html` as the entry; `output.html` is opened by the app as a pop-up
(same origin) and needs no special routing.

## Production deploy — Cloudflare Pages (canonical)

The beta site is Cloudflare Pages project `projectm-rs-web-beta`, production
branch **`main`**, alias **https://projectm-rs-web-beta.pages.dev**.

> **Critical rule:** a successful Wrangler *upload* does **not** prove the
> production alias was updated. Deploying from a detached-HEAD tag checkout makes
> Wrangler label the deployment branch `head`, producing a **preview**
> deployment — the per-deployment URL serves the new build while production keeps
> serving the old one. This exact failure mode hit `v0.0.3-web-beta.3`.

A production release is only verified when **all** of these hold:

```
explicit --branch main
+ Cloudflare reports the deployment on branch main (not a preview)
+ the canonical alias serves the exact hashed main asset we built
+ the canonical alias returns the COOP/COEP headers
```

Use the deterministic helper — it enforces every one of those and fails loudly
otherwise:

```bash
# resolve tag → build in an isolated worktree → detect asset → verify _headers,
# then STOP (publishes nothing). Always run this first.
scripts/deploy-cloudflare-pages.sh v0.0.3-web-beta.3 --dry-run

# real production publish (requires credentials in the environment):
CLOUDFLARE_API_TOKEN=…  CLOUDFLARE_ACCOUNT_ID=…  \
  scripts/deploy-cloudflare-pages.sh v0.0.3-web-beta.3

# verify what is ALREADY live matches a release (no deploy, no credentials):
scripts/deploy-cloudflare-pages.sh v0.0.3-web-beta.4 --verify-only
```

The script:

1. requires an **explicit immutable release ref** (tag or commit) — it never
   deploys "whatever is checked out"; it never creates or moves tags;
2. resolves the **dereferenced commit** (`<ref>^{commit}`) for annotated tags;
3. builds in a **temporary git worktree** at that commit (your checkout is
   untouched; the worktree is removed on exit);
4. records the exact **hashed main asset** the build produced
   (`assets/main-<hash>.js` — never hard-coded);
5. writes and verifies the deployment-only `_headers` (COOP/COEP) in `dist/`;
6. deploys with **`--branch main`** explicitly (never inferred from the current
   git branch / detached HEAD / tag name);
7. verifies via the Pages API that Cloudflare recorded the deployment on branch
   `main` — a preview result fails the run;
8. polls the canonical alias (bounded; edge caches `index.html` for ~1 min after
   deploy) until it serves the **expected** hashed asset — HTTP 200 alone and
   correct headers alone are **not** treated as success;
9. verifies COOP/COEP on the live alias.

**Credentials** come only from the environment Wrangler expects
(`CLOUDFLARE_API_TOKEN` with Pages:Edit, `CLOUDFLARE_ACCOUNT_ID`). They are never
accepted as CLI args, echoed, or written to the repo; if either is missing the
script stops before deploying. The reusable operational notes live in the
project memory `cloudflare-pages-prod-deploy.md`.

### Release identity metadata (proposal — not implemented)

Diagnostics currently can't show which git commit/tag produced a build (the
user-facing `APP_VERSION` in `web/src/version.ts` is a separate value, e.g.
`0.9.0`, and aligning it with the `v0.0.3-web-beta.x` git line is a separate
backlog task). A **non-invasive** addition would be a Vite `define` injecting
`__BUILD_COMMIT__` / `__BUILD_TAG__` from `git rev-parse`/`git describe` at build
time, surfaced read-only under **About → diagnostics**. This changes no
user-facing version string and touches only build config + the About panel; it
is recorded here as a proposal and intentionally left out of the deploy-hardening
change.
