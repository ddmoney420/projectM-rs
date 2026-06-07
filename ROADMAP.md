# projectm-rs — a 1-to-1 Rust port of projectM

Goal: a faithful, cross-platform Rust reimplementation of
[projectM](https://github.com/projectM-visualizer/projectm) (the open-source
Milkdrop visualizer). Renders Milkdrop-style visuals from live audio, loads the
existing `.milk` preset ecosystem unchanged, and also defines a Rust-native
preset format with a `.milk` importer/converter.

## Design decisions

- **Rendering: wgpu.** One codebase targets every platform. wgpu compiles our
  WGSL shaders to **native Metal** on Apple, Vulkan on Linux, DX12 on Windows,
  and WebGPU on the web. This is how we get "native Metal" *and* "cross-platform
  everywhere" without hand-maintaining three shader languages.
- **Shaders: WGSL is canonical.** Milkdrop presets ship HLSL-flavored pixel
  shaders; `pm-shader` translates them to WGSL once, then naga lowers WGSL to the
  platform-native backend.
- **Math: `glam`** replaces `glm`. **`image`** replaces `stb_image`.
  **`rustfft`** replaces the bespoke FFT. wgpu replaces `glad`/OpenGL.
- **No `unsafe` in the engine crates** except where wgpu/FFI demands it,
  isolated in `pm-render`.

## Crate map (C++ → Rust)

| projectM (C++)                               | Rust crate    |
|----------------------------------------------|---------------|
| `vendor/projectm-eval`                       | `pm-eval`     |
| `Audio/`                                     | `pm-audio`    |
| `Renderer/`                                  | `pm-render`   |
| `vendor/hlslparser` + `MilkdropShader`       | `pm-shader`   |
| `MilkdropPreset/`                            | `pm-preset`   |
| `ProjectM.cpp`, `TimeKeeper`, `PresetFactory`| `pm-core`     |
| (new) Rust-native format + `.milk` importer  | `pm-format`   |
| SDL/frontend examples                        | `pm-app`      |

## Phases

1. **pm-eval** — expression compiler (the preset math language). Headless,
   fully unit-tested. ← *done*
2. **pm-audio** — PCM ring buffer, FFT, loudness, waveform alignment, beat
   detection. Headless. ← *done*
3. **pm-render** — wgpu framebuffers, textures, meshes, samplers, blend modes,
   shader cache, transitions. ← *foundation done* (context, texture,
   framebuffer, fullscreen pass, readback, render context, vertex types)
4. **pm-shader** — Milkdrop HLSL → WGSL translation. ← *core done + hardening*
   (preprocessor with `#if`/`#ifdef` conditionals, HLSL lexer/parser, WGSL codegen
   with type inference + param-mutation shadowing; output validated by naga).
   The full preset-shader pipeline (wrap `shader_body` → uniform/intrinsic header
   → transpile → assemble bindings + entry) lives in `pm-preset::preset_shader`.
   **Corpus shader compat: ~37% composite / ~18% warp produce valid WGSL** — the
   remaining naga rejections (type-inference edge cases, matrix `mul`, intrinsic
   tail) are the main hardening work, tracked by the `shader_report` example.
5. **pm-preset** — full Milkdrop preset engine wiring eval + render + shader.
   ← *eval + warp + waveform + composite done*. Done: .milk parser, PresetState
   + defaults, per-frame & per-pixel evaluation; and in **pm-core** the warp
   mesh + GPU feedback pass, the Circle/Line **waveform** drawn into the
   feedback buffer (alpha/additive), and the default **composite** (animated hue
   gradient) to a display target. Renders a recognizable Milkdrop preset from a
   real `.milk` string + audio. Remaining: more waveform modes + custom
   waveforms/shapes, motion-vector/border/echo passes, preset shader WGSL
   integration (Milkdrop shader wrapper + uniform/texture bindings).
6. **pm-core + pm-format + pm-app** — orchestrator, native format + importer,
   live windowed app (winit + cpal). ← *pm-core started* (WarpEngine drives a
   preset's warp render); pm-format and pm-app still to come.

## Parity strategy

Each crate is validated against the C++ original before moving on:
- `pm-eval`: differential tests vs. ns-eel expected values (see crate tests).
- `pm-audio`: synthetic-signal FFT/loudness snapshots.
- `pm-render`/`pm-preset`: per-frame pixel-diff of reference presets vs. C++
  projectM screenshots (golden-image tests) within a tolerance.
