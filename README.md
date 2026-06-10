# projectm-rs

A from-scratch **Rust port of [projectM](https://github.com/projectM-visualizer/projectm) / Milkdrop**, the
classic music-reactive visualizer. It renders Milkdrop `.milk` presets — per-frame/per-pixel equations,
custom warp & composite **HLSL shaders** (transpiled to WGSL and validated with [naga](https://github.com/gfx-rs/wgpu)),
waveforms, custom shapes, feedback, motion vectors, and the final composite — on the GPU via
[`wgpu`](https://github.com/gfx-rs/wgpu).

**`pm-app`** is the runnable demo/player: a live window that cycles a real Milkdrop/projectM preset corpus
in time with audio (or a synthetic signal when no input device is present).

> ⚠️ **Early project / demo status** (version `0.0.1`). It's functional and well-tested, but not a polished,
> packaged release. See [Current limitations](#current-limitations).

## Compatibility status

Driven by a real ~14k-preset corpus, the HLSL→WGSL transpiler currently produces valid, naga-validated WGSL for:

- **~97.0%** of warp shaders
- **~96.0%** of composite shaders

Latest full corpus scan (the 9,795-preset *cream-of-the-crop* set):

```
presets found:           9795
loaded successfully:     9758  (99.6%)
skipped (load/parse):    37
custom shaders:          15834
shader translate failed: 323  (98.0% translate OK)
```

Some features remain **deferred** — most notably external sampler/image assets and a parser long-tail
(see [Current limitations](#current-limitations)).

## Quick start

### Prerequisites

- A **Rust toolchain** (stable; edition 2021, Rust ≥ 1.80). Install via [rustup](https://rustup.rs/).
- A **GPU + graphics driver** with a `wgpu`-supported backend (Vulkan / DX12 / Metal). Software/headless
  fallback may work but is not the target.
- A folder of **`.milk` presets** — e.g. the
  [Cream-of-the-Crop](https://github.com/projectM-visualizer/presets-cream-of-the-crop) and
  [Classic](https://github.com/projectM-visualizer/presets-milkdrop-original) projectM preset packs.

### Build

```sh
cargo build --release
```

### Run the player

Point `pm-app` at a folder of presets (it scans recursively for `.milk` files):

```sh
cargo run -p pm-app --release -- <path-to-preset-folder>
```

> **Always pass a preset folder.** If you omit the argument, the build falls back to a *developer-local
> default path* that almost certainly won't exist on your machine — in that case the app prints a short
> message telling you how to pass a preset folder and continues with its tiny built-in fallback preset.
> Pass your corpus directory explicitly.

## Controls

| Action | Key(s) |
|---|---|
| Next preset | `→` / `Space` / `N` |
| Previous preset | `←` / `P` |
| Random preset | `R` |
| Reload current preset | `F5` / `L` |
| Toggle transitions (crossfade) | `T` |
| Toggle perf overlay | `F` |
| Toggle HUD | `H` |
| Freeze / pause | `Pause` / `K` |
| Step one frame (while paused) | `.` |
| Toggle auto-advance | `A` |
| Adjust auto-advance interval | `[` / `]` |
| Toggle shuffle | `S` |
| Screenshot | `C` |
| Help overlay | `/` or `?` |
| Quit | `Esc` / `Q` |

Press `/` in the window for an on-screen list of these controls at any time.

## Environment variables

| Variable | Effect |
|---|---|
| `PM_PERF=1` | Start with the per-second **perf overlay** on. This *forces* it on for the launch **without** silently changing your saved preference (toggling with `F` is what persists). |
| `PM_SCAN=1` | Run a one-off **corpus compatibility scan** at startup (presets found / loaded / skipped / shader-translate rate), then continue normally. It is never persisted. |

## Runtime behavior

- **Invalid, unparsed, or black-rendering presets are skipped cleanly** — navigation probes candidates and
  moves on to the next one that actually renders, logging *why* each was skipped, plus a session summary on exit.
- **Auto-advance** cycles to the next renderable preset on a timer (default 30 s, adjustable with `[` / `]`),
  using the same skip logic; it never advances while paused.
- **Shuffle** (`S`) picks random presets while avoiding the current one and a recent-history queue, so it
  doesn't immediately repeat.
- **Freeze / pause** holds the **exact last rendered frame** — preset time, feedback, and transitions all stop
  (no drift), and the window stays responsive.
- **Step** (`.`, while paused) advances **exactly one deterministic 1/60-second frame**, then re-freezes.
- **Screenshots** (`C`) capture the **clean, pre-HUD visualizer frame** (the HUD/help overlay is never included).

## Files and locations

- **Screenshots** are written to **`./screenshots/`** (relative to the working directory), named with a
  **UTC timestamp** plus a sanitized preset name, e.g. `2026-06-09_210101_martin-lightning.png`. Existing files
  are never overwritten.
- **Preferences** (HUD, transitions, perf, auto-advance + interval, shuffle) are saved as a human-readable
  `key=value` file:
  - **Windows:** `%APPDATA%\pm-app\config.txt`
  - **Linux/macOS:** `$XDG_CONFIG_HOME/pm-app/config.txt`, or `~/.config/pm-app/config.txt`
- **Last shown preset** is remembered in **`last_preset.txt`** next to the config. It's stored as a path
  **relative to the corpus root** you launched with, and the next launch resumes there — gracefully falling
  back to a fresh start if the saved preset is missing, renamed, or from a different corpus.

A missing config means defaults; a malformed config logs a warning and falls back to defaults rather than failing.

## Testing / development

```sh
cargo test --workspace               # full unit/integration suite (GPU-independent)
cargo clippy --workspace --all-targets
```

- A **self-baseline snapshot regression harness** exists as an example (`cargo run -p pm-core --example snapshot`)
  with **6 committed baseline scenarios** (`crates/pm-core/tests/snapshots/`). It is *intentionally* an example,
  **not** part of the default `cargo test`, so the standard suite stays GPU-independent. (These are self-baseline
  regression checks, not C++ projectM parity tests.)
- Diagnostic/analysis examples under `crates/pm-preset/examples/` (e.g. `shader_report`, `validate_kinds`,
  `parse_buckets`) report corpus shader-compatibility breakdowns.

The workspace is split into focused crates: `pm-eval` (expression VM), `pm-audio`, `pm-shader` (HLSL→WGSL),
`pm-preset` (`.milk` loading + per-frame/per-pixel), `pm-render` (wgpu helpers), `pm-core` (the warp/composite
engine + transitions + snapshot harness), `pm-format` (native `.pmp` preset format), and `pm-app` (the player).

## Current limitations

- **External sampler / image assets are not supported yet** — presets that load their own gradient/texture
  files don't bind those; only in-pipeline feedback textures work.
- **Parser long-tail (deferred):** `sampler` / `sampler_state` declarations, the C-style comma operator
  `(a, b)`, array declarations + initializer lists, brace initializers (`= { ... }`), and empty-statement
  (`;;`) cleanup.
- A **small naga / matrix validate-stage tail** remains (a few matrix-index/​store and value-less-return cases).
- The **HUD/help font is uppercase-only** and has no arrow glyphs (directions are spelled out, e.g.
  `NEXT: RIGHT/SPACE/N`).
- **Screenshot capture is synchronous** — it briefly blocks the render loop while reading back the frame.
- **Verified runtime environment this session was Windows + NVIDIA.** macOS/Linux are expected to work through
  `wgpu`/`winit`/`cpal` but a smoke test on those platforms is still pending.

## License and provenance

Licensed under **LGPL-2.1** — see [`COPYING`](COPYING) for the full license text and [`NOTICE`](NOTICE) for
attribution. Version `0.0.1`: an early, demo-ready port, not a packaged release.

projectM-rs is a **Rust port and derivative work** of components of the LGPL-2.1
[projectM / libprojectM](https://github.com/projectM-visualizer/projectm) visualizer and the
[projectm-eval](https://github.com/projectM-visualizer/projectm-eval) expression library — **not** a clean-room
reimplementation. Many source files retain a per-file `//! Port of …` header identifying the specific upstream
subsystem they were ported from. The project is independent and not endorsed by the projectM maintainers.

**No third-party / community `.milk` preset packs are bundled** — bring your own preset corpus.

Third-party Rust dependencies (wgpu, naga, winit, cpal, pollster, glam, bytemuck, image) are distributed under
their own permissive licenses (MIT and/or Apache-2.0).
