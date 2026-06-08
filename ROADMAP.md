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
   **Custom composite shaders now render** (`pm-core::PresetComposite`): the
   translated WGSL is wired with a per-frame `MdUniforms` buffer (`_cN`/`q`/rot)
   and `sampler_main`, swapping in for the default hue composite. Custom *warp*
   shaders + content generators (custom waveforms/shapes) are still to come.
5. **pm-preset** — full Milkdrop preset engine wiring eval + render + shader.
   ← *eval + warp + standard & custom waveforms + composite done*. Done: .milk
   parser, PresetState + defaults, per-frame/per-pixel eval; in **pm-core** the
   warp mesh + GPU feedback pass, the Circle/Line **standard waveform**, the
   **custom waveforms** (`wave_N` per-point geometry via per-frame/per-point
   eval, drawn with a per-vertex-color line renderer — the main content
   generator), the default **composite** (hue) and **custom composite shaders**.
   Remaining: more standard waveform modes, custom **shapes**, motion-vector/
   border/echo passes, custom **warp** shaders. Perf note: per-point eval in the
   tree-walker is slow at hundreds of points × 60fps — wants the bytecode pass.
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
