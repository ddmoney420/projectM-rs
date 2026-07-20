# pm-web versioning & release convention

Canonical policy for the browser product (pm-web VJ). Establishes one SemVer line
going forward and preserves the historical beta line as immutable history.

## 1. Product release version

The user-facing product version is a SemVer pre-1.0 string, e.g. **`0.10.0-beta.1`**
(betas) or **`0.10.0`** (stable). `PRODUCT_BASE_VERSION` in `web/src/version.ts`
is the single hard-coded source of truth for the current minor line (`0.10.0`);
bump it once per product milestone — **not** once per beta.

## 2. Git tag convention

```
v<major>.<minor>.<patch>[-beta.<n>]
```
- `v0.10.0-beta.1` → product `0.10.0-beta.1`
- `v0.10.0`        → product `0.10.0`

The git tag is the release's identity; the exact build is pinned by the tag's
**dereferenced commit SHA**.

## 3. Beta numbering

Betas within a minor line increment: `v0.10.0-beta.1`, `-beta.2`, … then the
stable `v0.10.0`. A subsequent feature milestone starts a new minor:
`v0.11.0-beta.1`. **Phase numbers are never encoded in versions.**

## 4. Stable release

`v0.10.0` (no suffix) → product `0.10.0`.

## 5. Internal Cargo crate versions — independent

The Rust workspace crates stay at their own `workspace.package.version`
(currently `0.0.1`) — they are **internal implementation components**, not
independently published, so they are **not** bumped in lockstep with the product
release. Product releases version the **web app**, not the crates. This avoids
pointless Cargo churn. (If a crate is ever published to crates.io, it gets its own
independent version at that time.)

## 6. Historical tag compatibility (immutable)

The old line is preserved verbatim and **never reinterpreted**:
```
v0.0.3-web-beta.1  (8048788)
v0.0.3-web-beta.2  (dfff323)
v0.0.3-web-beta.3  (dd14614)
v0.0.3-web-beta.4  (1dce57c)  ← current production
```
`parseReleaseTag('v0.0.3-web-beta.4')` returns kind `legacy`, version
`0.0.3-web-beta.4` — it is **not** mapped to `0.10.0-beta.4`. These tags are
immutable historical releases and are not moved/renamed/deleted. Building a legacy
tag with the current deploy tooling reproduces its **historical** artifact (the
release commit's own source predates build-time version injection, so the app
shows whatever `APP_VERSION` was hard-coded at that commit — `0.9.1` for beta.4).

## 7. Development-build identity

An untagged (local/dev) build is honest and never fabricates a beta number:
```
Version: 0.10.0-dev · <short commit> · development
```
`resolveDisplayVersion({ releaseTag:'', … })` → `0.10.0-dev`.

## 8. Commit metadata

Every build embeds the git commit (short SHA in About). A tagged release embeds
the **dereferenced tag commit**; a dev build embeds the working-tree HEAD.

## 9. Build-time injection (no manual edit, no network)

Release identity is injected at build time via Vite `define`
(`web/vite.config.ts`) from environment variables — no per-beta source edit, no
network call:
- `APP_RELEASE_TAG` — the release tag being built (empty ⇒ dev).
- `APP_GIT_COMMIT` — the exact commit (falls back to `git rev-parse HEAD` locally).

`web/src/version.ts` reads the injected `__APP_RELEASE_TAG__` / `__APP_GIT_COMMIT__`
and resolves the display version through `web/src/version-tag.ts` (pure, unit-tested).

## 10. Deployment integration

`scripts/deploy-cloudflare-pages.sh` already resolves a release ref → annotated
tag object → **dereferenced commit** → isolated worktree → deterministic build.
It now exports `APP_RELEASE_TAG` + `APP_GIT_COMMIT` (= the resolved tag + commit,
**not** current `main`) into the build, so the artifact embeds the exact release
identity. All existing safeguards are unchanged (isolated worktree, explicit
`--branch main`, canonical-alias + hashed-asset + COOP/COEP verification, bounded
edge-cache poll, credential safety). A bare commit ref (no `v…` tag) builds as
`-dev`.

## 11. About display

`About` (`web/src/help.ts`) shows `Version · Commit` (+ build mode for dev):
```
release:  0.10.0-beta.1 · abc1234
dev:      0.10.0-dev · eea3f07 · production
```
No credentials / account IDs / tokens are ever displayed.

## 12. Release-candidate gate (proposed)

For the next public beta (`v0.10.0-beta.1`), the recommended gate is:

```
A. merge version-alignment PR
B. build the exact prospective RC (deploy script --dry-run on the RC commit)
C. full automated regression (verify.mjs + all Phase-10 suites)
D. Phase-10 short stress (verify-qualification.mjs)
E. ≥1 longer desktop soak if practical (SOAK_ITERS)      ← manual
F. physical iPhone Safari smoke                           ← recommended gate
G. optional physical MIDI validation (non-blocking)       ← if hardware
H. tag v0.10.0-beta.1 (annotated) on the RC commit
I. deploy via scripts/deploy-cloudflare-pages.sh (--branch main)
J. verify canonical alias + hashed assets + COOP/COEP
K. physical post-deploy iPhone smoke
```
Given the project's iPhone-specific history, **physical iPhone validation is a
recommended beta-release gate**. Physical MIDI may remain non-blocking but must be
reported honestly (currently simulated only).

## 13. Migration from `v0.0.3-web-beta.x`

The old `v0.0.3-web-beta.N` line stops at beta.4 (it remains valid history). The
next public beta uses the new line `v0.10.0-beta.1`. There is no renumbering of
past releases; the two lines coexist, distinguished by the tag parser's
`legacy` flag.

## RC qualification status (v0.10.0-beta.1 @ 9cbbf91 — FINAL RC commit)

Final RC commit `9cbbf91` supersedes the earlier `c6ca3cd` (PR #19 added only docs
+ the standalone soak harness — no runtime/build change — but the tag will point
at `9cbbf91`, so the full gate was re-run there).

```
RC build provenance:                 PASS (About shows 0.10.0-beta.1 · 9cbbf91)
Workspace tests:                     277 passed / 0 failed
Version metadata tests:              6/6
Deployment-tool tests:               22/22
All Phase 10 suites:                 green (23/23/26/16/16/17/16/22/13)
Full WebGPU regression:              88/88 boolean (114 entries), 0 errors/panics/console
Short qualification stress:          PASS (leak-free 26 → worst 67 → 26)
Extended desktop soak:               PASS at 5.03 min (301,955 ms, 385 iters,
                                       9283 frames; leak-free; FPS 25.7–33.1
                                       avg 31.3; CPU 4.8→5.3 ms flat; 0 errors)
60-minute desktop soak:              NOT YET RUN (only 5.03 min completed) — accepted beta risk
Physical iPhone:                     NOT YET RUN (checklist: docs/rc-iphone-checklist.md)
Physical MIDI:                       NOT YET RUN (simulated harness only; non-blocking for beta.1)
```

**Release-facing performance wording:** dual Milkdrop is *supported on qualified
desktop hardware*; the heavy RC workload averaged ~31 FPS on the tested discrete
NVIDIA env, so we do **not** promise 60 FPS — performance varies with GPU
capability, resolution, preset complexity, and simultaneous preset transitions.
Mobile dual-Milkdrop is capability-gated / experimental.
Desktop env: Windows 11 · Chrome (headed, real GPU) · discrete NVIDIA
(`max_texture_dimension_2d` 16384, `timestamp-query` available) · 1280×800 · DPR 1.
