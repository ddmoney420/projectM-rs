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
   fully unit-tested. ← *done*. Compiles to a slot-resolved IR (variables →
   `Vec` slot indices, functions → opcode enum, no per-call allocation) so the
   hot per-pixel/per-point loops run without hashing — ~2.5–3× the original
   tree-walker. Hosts hold `VarSlot` handles for the hottest variables.
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
   **Corpus shader compat: ~70% composite / ~70% warp produce valid WGSL** (up
   from 37% / 18%), via corpus-driven hardening tracked by the `shader_report`
   example: built-in noise `texsize_*` constants, HLSL implicit vector
   truncation on store, `const`/`static` qualifiers and `float1` scalars,
   top-level comma operator, scalar swizzles, and 3D noise-volume textures. The
   remaining naga rejections (scattered `InvalidBinary` type-inference edges,
   matrix `mul`, intrinsic tail) are the next hardening work.
   **Custom composite *and* warp shaders now render.** Composite
   (`pm-core::PresetComposite`): the translated WGSL is wired with a per-frame
   `MdUniforms` buffer (`_cN`/`q`/rot) and `sampler_main`, swapping in for the
   default hue composite. Warp (`pm-core::CustomWarp`): the preset's warp
   `shader_body` runs as the *fragment* over the warp mesh — the vertex stage
   computes the warped sampling UV (same math as the default warp), the fragment
   samples `sampler_main` at that UV and runs the preset's color math, fed the
   same `MdUniforms` block. Both fall back to the built-in pass when the WGSL
   fails to compile (error-scope guarded).
5. **pm-preset** — full Milkdrop preset engine wiring eval + render + shader.
   ← *eval + warp + standard & custom waveforms + composite done*. Done: .milk
   parser, PresetState + defaults, per-frame/per-pixel eval; in **pm-core** the
   warp mesh + GPU feedback pass, the Circle/Line **standard waveform**, the
   **custom waveforms** (`wave_N` per-point geometry), the **custom shapes**
   (`shape_N` filled N-gons w/ gradient + border, per-instance per-frame eval),
   the default **composite** (hue), **custom composite shaders**, **custom
   warp shaders** (`shader_body` warp fragment over the warp mesh), and the
   built-in **noise textures** (`noise_lq/mq/hq`, `noisevol_lq/hq` 3D volumes,
   generated once and bound to `sampler_noise*` references). Remaining: more
   standard waveform modes, textured shapes, motion-vector/border/echo passes,
   real per-frame blur textures (`sampler_blur1/2/3`, currently feedback
   stand-ins).
6. **pm-core + pm-format + pm-app** — orchestrator, native format + importer,
   live windowed app (winit + cpal). ← *pm-core + pm-app done*. pm-core's
   WarpEngine drives warp+waveform+composite; **pm-app** is a live winit window
   with a wgpu surface, cpal audio capture (graceful synthetic fallback), preset
   cycling over the 9,795-preset corpus, and keyboard controls. pm-format (native
   `.milk`-importing format) still to come.

## Parity strategy

Each crate is validated against the C++ original before moving on:
- `pm-eval`: differential tests vs. ns-eel expected values (see crate tests).
- `pm-audio`: synthetic-signal FFT/loudness snapshots.
- `pm-render`/`pm-preset`: per-frame pixel-diff of reference presets vs. C++
  projectM screenshots (golden-image tests) within a tolerance.
