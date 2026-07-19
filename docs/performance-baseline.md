# pm-web Performance Baseline (Phase 9)

Environment (measured):

| | |
|---|---|
| Base commit | `9acae4c` (Phase 8d) → Phase 9 |
| Rust | 1.94.1 |
| Node | 24.14.1 |
| Browser | installed Chrome (headed, real GPU), Playwright-driven |
| OS | Windows 11 |
| GPU note | `powerPreference` ignored on Windows (benign Chrome warning) |

## Bundle sizes

Code-splitting the two heavy UI modules (shader editor / MIDI panel) moved
CodeMirror out of the initial download.

| Chunk | Before | After |
|---|---:|---:|
| Initial JS (`main`) | 595 KB | **111 KB** (gzip 32 KB) |
| `shader-console` (lazy, CodeMirror) | — | 484 KB (gzip 157 KB) — loaded on first Console open |
| `midi-ui` + `midi` (lazy) | — | ~10 KB — loaded on first MIDI open |
| `output` / `projection-protocol` | 2 KB / 1 KB | 2 KB / 1 KB |
| wasm (`pm_web_bg`) | 7.77 MB | 7.77 MB |

The wasm module dominates total transfer and is required for the first frame;
the JS split mainly reduces initial parse/execute and defers CodeMirror until
the editor is actually opened.

## Frame time

Measured via the diagnostics panel (`cpuMs` = EMA of time spent in the Rust
`render()`; FPS from the frame-counter delta).

| Scene | FPS | CPU in `render()` |
|---|---:|---:|
| Stress (multiple layers + multipass shader + per-layer & global effects, audio) | ~32 fps | ~6.2 ms/frame |

The stress figure is the accumulated end-of-regression scene. CPU-in-render is
~6 ms, well under a 60 fps budget, so the app is **not CPU-bound** in that
scene — headroom exists; the frame rate there is bounded by GPU present /
compositor + multipass GPU work on the test machine. Lighter default scenes
(Milkdrop + waveform) run comfortably at the display refresh rate.

## Allocation / pipeline / pool notes

- **Pipelines** are created once (compositor, blit, effect übershader, overlay,
  each shader pass) and reused; parameter/uniform changes never recreate a
  pipeline (live effect params drive a `mode` uniform; shader controls drive a
  uniform buffer). Recompiling a shader pass builds one new pipeline and keeps
  the previous as last-known-good.
- **Render targets** come from bounded pools / fixed per-instance textures
  (two ping-pong accumulators; per-layer effect output; per-shader-pass ping-pong
  history; a reset-on-frame effect pool). Nothing allocates a full-res texture
  per frame. Resize recreates size-dependent textures and clears history; it does
  not recreate pipelines.
- The regression harness repeatedly loads/imports scenes, adds/removes layers and
  effects, resizes, and resets across ~99 checks with **0 WebGPU validation
  errors and 0 WASM panics**, and the soak segment confirms the frame counter
  keeps advancing (~32 fps sustained) — consistent with bounded resource use.

## How to reproduce

```bash
cd web && npm run build          # bundle sizes in the vite output
# open the app, expand the diagnostics panel for FPS + CPU ms,
# or run: node verify.mjs        # p9SoakFps / p9CpuMsValue in shots/results.json
```
