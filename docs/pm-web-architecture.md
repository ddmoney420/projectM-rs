# pm-web: WebGPU/WASM browser frontend — architecture note (Phase 0)

Status: **Phase 0 (audit + architecture)** — no engine code changed yet.
Branch: `feature/wasm-webgpu-visualizer`.

This note records the Phase 0 audit of `projectm-rs` and the plan for adding a
browser target. It is grounded in the actual code as of this branch, and in two
verified `wasm32` builds (see [Verified findings](#verified-findings)).

## Baseline provenance

Recorded before any Phase 1 changes, to establish a clean, known baseline.

| Item | Value |
|---|---|
| Origin URL | `https://github.com/ddmoney420/projectM-rs.git` |
| Base branch | `main` |
| Starting commit SHA | `f98c5d09344152b0285514424246235c07fedab1` (`docs: add macOS quick-start guide`) |
| Branch point (merge-base with `main`) | `f98c5d09344152b0285514424246235c07fedab1` |
| Feature branch | `feature/wasm-webgpu-visualizer` |
| `git status` at start | clean (no modified/staged/untracked files) |

**Verified WASM builds** (run on the feature branch, both succeeded):

```
$ cargo build --target wasm32-unknown-unknown -p pm-eval -p pm-audio -p pm-shader -p pm-preset -p pm-format
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 22.59s        # OK

$ cargo build --target wasm32-unknown-unknown -p pm-render -p pm-core
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 45s        # OK
```

(`wasm32-unknown-unknown` target was added via `rustup target add` for these builds.)

**Uncommitted diff at end of Phase 0** (before Phase 1):

- `docs/pm-web-architecture.md` — new, +172 lines (this document). Untracked (`?? docs/`).
- No engine or config files modified.

Per the agreed workflow, Phase 0 is **not** committed on its own; this document
is committed together with the first working `pm-web` shell (Phase 1 checkpoint).

## Goal

Add a **WebGPU-only** browser frontend that reuses the existing Rust engine
(audio analysis, expression VM, preset handling, shader transpile, rendering)
rather than reimplementing it in JavaScript. It starts as a personal reactive
shader toy (pick an audio source, write/paste a shader, twiddle knobs) and grows
into a VJ tool (layers, effects, scenes, MIDI, recording). The toy and the VJ
app are the **same engine at different levels of UI exposure**.

## Verified findings

Both commands were run on this branch and **succeeded**:

```
cargo build --target wasm32-unknown-unknown -p pm-eval -p pm-audio -p pm-shader -p pm-preset -p pm-format   # OK
cargo build --target wasm32-unknown-unknown -p pm-render -p pm-core                                          # OK
```

Implication: **all seven non-app crates cross-compile to `wasm32` unchanged.**
`wgpu 29` pulled its web backend (`web-sys 0.3`, `js-sys`, `wasm-bindgen-futures`)
automatically for the wasm target. The engine is already portable; the work is a
new **frontend**, not an engine rewrite.

Toolchain: edition 2021, `rust-version = 1.80`, `wgpu = 29`, `glam = 0.30`,
`bytemuck = 1`. Remote: `github.com/ddmoney420/projectM-rs` (working tree clean).

## What is reusable unchanged

| Crate | Role | Port status |
|---|---|---|
| `pm-eval` | expression VM | ✅ wasm-clean (no deps) |
| `pm-audio` | PCM → FFT/waveform/beat (`bass`/`mid`/`treb`) | ✅ wasm-clean (no deps) |
| `pm-shader` | Milkdrop HLSL → WGSL | ✅ wasm-clean (no deps) |
| `pm-preset` | `.milk` load + per-frame/per-pixel | ✅ wasm-clean |
| `pm-format` | native `.pmp` format | ✅ wasm-clean |
| `pm-render` | wgpu context/textures/framebuffers/passes | ✅ builds for wasm |
| `pm-core` | warp/composite engine + transitions | ✅ builds for wasm |
| `pm-app` | native player (winit/cpal/pollster) | ⛔ native-only; replaced by `pm-web` |

The engine is **surface-agnostic**: `pm-core` renders a preset into a
`pm_render::Texture`, and the frontend blits that texture to whatever surface
view it owns. On native, `pm-app/src/blit.rs` (`Blit::draw(ctx, src, target_view)`)
copies it to the winit surface; on web the identical call targets the canvas
surface view.

## The platform seams (all currently in `pm-app`)

1. **GPU init is blocking.** `pm-app/src/main.rs` (`resumed()`) creates the winit
   surface and calls `pollster::block_on(request_adapter / request_device)`.
   `pm_render::GpuContext` fields (`instance/adapter/device/queue`) are **public**
   and it already has an internal `headless_async()`, so `pm-web` constructs a
   `GpuContext` from a **canvas** surface with `.await` (via
   `wasm-bindgen-futures`) — no `pollster` on the main thread. Small, additive.
2. **Audio capture is cpal.** `pm-app/src/audio.rs` wraps `pm_audio::PCM` and
   calls `pcm.add_float(data, channels)` then `pcm.update_frame_audio_data(dt, frame)`.
   `pm-web` replaces *only this feeder* with a Web Audio bridge into the same
   `add_float` seam. The FFT/alignment/loudness/beat pipeline is untouched, so we
   keep projectM's exact `bass`/`mid`/`treb` behavior (not a generic browser FFT).
3. **Windowing/event loop is winit.** `pm-web` uses a canvas + `requestAnimationFrame`
   render loop and browser key/pointer events.
4. **Persistence/screenshots are filesystem.** `prefs.rs`/`screenshot.rs` use
   files; `pm-web` uses `localStorage`/IndexedDB and canvas→blob download.

`GpuContext::headless()` (the `pollster::block_on` wrapper) must **not** be called
on wasm — use an async constructor. The crate compiles either way; this is a
runtime rule, not a build change.

## Proposed workspace shape

```
projectm-rs/
├── crates/
│   ├── pm-audio pm-core pm-eval pm-format pm-preset pm-render pm-shader   # shared engine (unchanged)
│   ├── pm-app/     # native desktop player (unchanged)
│   └── pm-web/     # NEW — browser/WASM adapter (wasm-only deps)
├── web/            # NEW — browser UI (TS + Vite): editor, panels, worklet, share URLs
└── Cargo.toml
```

`pm-web` owns only browser seams: wasm exports, async WebGPU/canvas init,
lifecycle/resize/DPR, Web Audio bridge, permissions, JS↔Rust events, storage,
URL import/export, unsupported-WebGPU diagnostics. Everything about preset
evaluation, audio analysis, rendering, compositing and shader validation stays
in Rust.

Native-safety rule: keep browser APIs out of the shared crates; if a shared
crate needs an async or platform variant, gate it with
`#[cfg(target_arch = "wasm32")]` / a cargo feature so `pm-app` is unaffected.

## Shader pipeline (console)

One pipeline, two front-end contracts, all ending in WGSL for WebGPU:

- **Shadertoy mode (default):** accept `mainImage(out vec4, in vec2)`, inject the
  v1 uniform contract + resource decls, generate a fragment entry, then GLSL→WGSL.
- **Raw GLSL (advanced):** explicit entry point + documented binding contract,
  same pipeline.

GLSL→WGSL uses Naga (already present via `wgpu`); enable `wgpu`'s `glsl` feature
(or call Naga's `glsl-in` directly) so both modes stay inside the single WebGPU
renderer. **No WebGL2.** Exact wiring (wgpu `ShaderSource::Glsl` vs. direct Naga)
to be finalized against the vendored versions in Phase 4; both are available at
`wgpu 29`.

### v1 Shadertoy compatibility target (single Image pass)

`mainImage`, `iResolution`, `iTime`, `iTimeDelta`, `iFrame`, `iMouse`, `iDate`,
`iSampleRate`, `iChannel0–3`, `iChannelResolution`, an audio FFT/waveform
texture, and user-defined reactive uniforms.

**Deferred to the render-graph work** (Phase 6+): Buffer A–D multipass, cubemaps,
video/keyboard/sound-shader channels, VR entry points, arbitrary user-pass
feedback. Multipass maps onto the same layer/effect render graph rather than a
parallel runtime. The editor surfaces unsupported constructs clearly.

## Audio bridge

Three sources — microphone (`getUserMedia`), file/URL playback, tab/system
(`getDisplayMedia`) — all feeding the same `PCM::add_float`. Preferred transport:
**AudioWorklet** → ring buffer → wasm (with a batched typed-array fallback where
cross-origin isolation / `SharedArrayBuffer` is unavailable). A source mixer
(per-source gain/toggle, master gain, level meter) sits in front; the initial UI
may default to one active source but the data model allows mixing.

## Licensing / attribution (design constraint)

Do **not** assume the whole Shadertoy corpus is CC BY-NC-SA. Authors license
their own shaders; absence of a clear reusable license is **not** permission.
Every imported shader carries its own metadata: `title`, `author`, `source_url`,
`license`, `license_url`, `modified_from`, `attribution_text`, `imported_at`.
The application code uses the repo's own license; imported shader code and
derivatives keep their own obligations. Attribution is never stripped on scene
export or URL sharing; missing-license imports are flagged.

## Known wasm runtime constraints (found in Phase 2)

The engine cross-compiles unchanged, but two shared-crate paths block the wasm
main thread and must be addressed before the features that use them:

- **Custom-shader pipeline creation blocks.** `preset_warp.rs` and
  `preset_composite.rs` call `pollster::block_on(error_scope.pop())` to validate
  a preset's translated HLSL→WGSL shader during `WarpEngine::new`. On the wasm
  main thread this would hang/panic. **The built-in preset has no custom warp or
  composite shader, so it never reaches these paths** — Phase 2 renders fine.
  But loading real corpus presets or the Phase 4 shader console *will* hit them.
  Fix (Phase 4): make pipeline creation async, or `cfg(target_arch = "wasm32")`
  skip the synchronous error-scope pop (rely on the async `uncapturederror`
  event / device-lost handling instead).
- **`read_rgba8` blocks** (`readback.rs`: `map_async` + `poll(Wait)`) — used for
  screenshots/tests, not the render path. Browser screenshots (Phase 7+) need an
  async readback variant.

Otherwise the per-frame render path is wasm-safe: no `std::time::Instant`
(panics on `wasm32-unknown-unknown`), no threads, no blocking.

## Phased plan (tracked as tasks)

- **P0** audit + this note + wasm build proof — *done*.
- **P1** `pm-web` + `web/`: canvas, async WebGPU, render loop, resize, device-loss,
  unsupported page, one known visual.
- **P2** existing preset rendering in browser (canvas-surface `GpuContext`); native
  regression check.
- **P3** audio bridge (mic/file/tab → `add_float`); per-source gain/toggle.
- **P4** live shader console (Shadertoy + raw GLSL, hot compile, last-known-good,
  diagnostics, core uniforms, audio texture, examples).
- **P5** speed/time accumulator, tempo (tap/auto/manual + beat phase), user knobs
  with audio/LFO modulation, waveform/spectrum overlays.
- **P6+** layer/compositor render graph, blend modes, post-effects, scenes,
  URL-fragment sharing; then MIDI, recording, projection, multipass Shadertoy.

Each phase must end with working builds/tests before the next begins; no large
untested framework before the first visible browser render.

## Decisions log

- **Work in the existing clean checkout, not a fresh clone.** Remote is the user's
  own repo and the tree is clean/current; a re-clone adds nothing. (A "mandatory
  fresh clone of `<REPOSITORY_URL>`" instruction arrived with no URL and matched an
  injected-text pattern; not acted on.)
- **`pm-web` inside the workspace**, not a sibling repo — atomic cross-crate
  changes, one lockfile, compile-time API drift detection. Split later only if the
  browser product gains independent maintainers/cadence.
- **WebGPU only**, no WebGL2 fallback; capability *tiers* (Core/Enhanced/Maximum)
  gate effects by adapter limits instead of a second renderer.

---

# Final architecture (Phases 3–9, as built)

The sections above are the Phase-0 design note; this section documents the
system as actually built. Where they differ, this section wins.

## Rendering pipeline

```
Audio sources (file / mic / tab)
   ↓  AudioWorklet → lock-free SPSC ring (SharedArrayBuffer, postMessage fallback)
pm_audio::PCM  (projectM FFT / beat / waveform — NOT AnalyserNode)
   ↓  FrameAudioData
Tempo + LFO bank + Parameter modulation (pm-params)
   ↓  per-frame ModContext + ShaderUniforms (iTime/iFrame/iBass/iBeat…)
Per-layer source render:
   • Milkdrop (one shared engine)
   • ShaderProject  → Buffer A → B → C → D → Image   (multipass, ping-pong history)
   • Waveform / Spectrum overlays
   ↓
Per-layer effect chains (übershader; bounded RT pool)
   ↓
Layer compositor (ordered stack, 7 blend modes, 2D transform, opaque-base vs coverage-alpha)
   ↓
Global effect chain
   ↓
Final canvas ──► Recording (canvas.captureStream → MediaRecorder → WebM)
            └──► Projection mirror (captureStream shared to the output window)
```

## Subsystems

- **Layers / compositor** (`compositor.rs`) — stable u64 layer ids (preserved
  across save/reload), two ping-pong accumulators, per-layer effect output. Ids
  are the stable address for MIDI + persistence.
- **Effects** (`effects.rs` + `effects.wgsl`) — one übershader selected by a
  `mode` uniform so live params never recompile; multipass bloom; stateful
  feedback owns independent history. **This is separate from Shadertoy Buffer
  feedback** — resetting one does not touch the other.
- **Multipass shaders** (`shader_project.rs`) — Buffer A–D + Image on the shared
  WGSL binding contract (uniform@0, iChannel0-3@1-4, sampler@5, user@6). Fixed
  execution order A→B→C→D→Image; a channel reads a buffer's `front`, so forward
  deps (earlier buffer, already flipped) get this frame and self/later get the
  previous frame — cycles resolve via one-frame history. `@control`s aggregate
  across passes into one project registry (shared `pm_user` slots). Per-pass
  last-known-good; history never serialized.
- **Parameter model** (`pm-params`) — base + one ModSource (bass/mid/treb/vol/
  atts/beatPulse/beatPhase/lfo0-3) × amount, curve, smoothing, clamp. MIDI and
  the UI set the **base**; modulation applies after.

## MIDI routing (`pm-midi` + `midi.rs`)

Web MIDI (gesture-gated, never SysEx) → the single `midi_handle` entry point
(also used by the `?miditest` injection hook). Events are parsed (`pm-midi`,
system real-time dropped), matched against mappings (device/channel/selector),
and applied to a **stable target path** (`layer.<id>.opacity`,
`layer.<id>.effect.<eid>.param.<i>`, `global.speed`, …). MIDI writes the
parameter base; app-level triggers (`app.*`, e.g. record) go through a decoupled
action queue drained by JS. Mappings persist globally (localStorage), keyed by
stable ids so they survive reload/reorder.

## Scene persistence & share URLs

`pm-scene` is the versioned serde model (layers, sources incl. multipass
`passes`, effects, transforms, tempo). Import is transactional (parse+validate,
then swap); ids and multipass config round-trip; GPU history is never
serialized. Local persistence is `localStorage`. **Share URLs** deflate-raw +
base64url the scene JSON into the URL **fragment** (`#s=…`) — decoded and
imported on load, entirely client-side.

## Projection (second screen)

The output window (`output.html`) runs no engine: it displays a `MediaStream`
captured from the control canvas (`captureStream`). Because `MediaStreamTrack`
`postMessage` transfer is unsupported in the target Chrome, the same-origin
stream is shared by reference via `window.opener`; a tiny versioned protocol
(`projection-protocol.ts`: hello/track/bye/ping + peer id) drives the handshake,
status, and automatic reconnect after a controller reload. The mirror is
pixel-identical with perfect temporal fidelity and cannot destabilize the main
renderer.

## Verification

Headed-Chrome Playwright (`web/verify.mjs`) exercises the whole system on a real
GPU across ~99 checks (audio, shaders, multipass feedback, layers, effects,
scenes, share URLs, MIDI via synthetic injection, projection, recording, resize,
reload persistence, About/onboarding, and a soak segment) with 0 WebGPU errors
and 0 WASM panics. Pure logic (blend math, parameter model, MIDI mapping,
scene/graph validation + migration, projection protocol) is native-unit-tested.
