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
   **Corpus shader compat: ~79% composite / ~81% warp produce valid WGSL** (up
   from 37% / 18%), via corpus-driven hardening tracked by the `shader_report`
   example: built-in noise `texsize_*` constants, HLSL implicit vector
   truncation on store *and* in binary ops (`float4 * float2` -> first two
   lanes), `const`/`static` qualifiers and `float1` scalars, top-level comma
   operator, scalar swizzles, 3D noise-volume textures, `lerp` broadcasting to
   the widest operand, and float `++`/`--` lowered to `+= 1`. The remaining
   naga rejections (user-defined function return-type inference, matrix `mul`,
   non-lvalue assignment targets) are the next hardening work.
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
   warp shaders** (`shader_body` warp fragment over the warp mesh), the
   built-in **noise textures** (`noise_lq/mq/hq`, `noisevol_lq/hq` 3D volumes,
   generated once and bound to `sampler_noise*` references), and the per-frame
   **blur chain** (`sampler_blur1/2/3` — separable Gaussian at 1/2, 1/4, 1/8
   res, rebuilt each frame from the feedback; `GetBlur1/2/3`). The **standard
   waveform** now covers the full `nWaveMode` family: Circle (0), XY-oscillation
   spiral (1), centered spiro (2/3), derivative line (4), explosive hash (5),
   Line (6) and double line (7), ported from projectM's `Waveforms/`; spectrum
   line (8) and the Milkdrop2077 modes fall back to Line. The **default
   (non-shader) final composite** now ports `FinalComposite`/`VideoEcho`/`Filters`:
   the animated hue tint, **video echo** (blend a zoomed+oriented copy), **gamma**
   brighten, and the **brighten/darken/solarize/invert** filters — the original's
   multi-pass GL blend tricks collapsed to closed-form colour math in one
   fragment. The **inner/outer border** frames (`ob_*`/`ib_*`) draw into the
   feedback buffer on top of the waveforms (`Border`). **Motion vectors**
   (`mv_*`, `MotionVectors`): the warp pass writes its per-pixel sampling UV to a
   motion-field texture, and a grid of `mv_x`×`mv_y` line segments samples it in
   the vertex stage (port of `PresetMotionVectorsVertexShader`, with the
   `length_multiplier`/`minimum_length` trail logic) to visualise the optical
   flow. Remaining: textured shapes (needs external preset image assets).
6. **pm-core + pm-format + pm-app** — orchestrator, native format + importer,
   live windowed app (winit + cpal). ← *done*. pm-core's WarpEngine drives
   warp+waveform+composite; **`PresetPlayer`** wraps it to crossfade between
   presets (Milkdrop's ~2.7s soft cut): both the outgoing and incoming presets
   keep rendering independently and are blended over an elapsed-time window, the
   outgoing one dropped when the fade completes (duration 0 = hard cut).
   **pm-app** is a live winit window with a wgpu surface, cpal audio capture
   (graceful synthetic fallback), transition-blended preset cycling over the
   corpus, and keyboard controls (incl. `T` to toggle transitions). **pm-format** is the native `.pmp`
   preset format: a structured, lossless representation (scalar params + named
   code blocks) with a `.milk` importer/exporter and a readable text form.
   Validated over the 14k-preset corpus: `.milk → native → .pmp → native`
   round-trips losslessly for 99.99%, and the reconstructed `.milk` loads in the
   engine at exactly the original rate (99.65%) — i.e. zero behavioural drift.

## Performance

Measured headless at 1280×720 on a GeForce GTX 960 (`bench_render` example;
each frame timed for CPU-return and again after a `poll(Wait)` so GPU ≈
total − CPU). 60 fps budget is 16.67 ms.

| scenario | total avg | cpu / gpu | p99 | ~fps |
|---|---|---|---|---|
| simple | 1.2 ms | 1.0 / 0.2 | 3.4 | 805 |
| heavy warp (per-pixel) | 3.0 ms | 2.8 / 0.2 | 3.5 | 331 |
| heavy waveform + shapes | 3.0 ms | 2.8 / 0.2 | 3.7 | 332 |
| heavy shader (warp+comp) | 1.3 ms | 1.0 / 0.3 | 2.0 | 781 |
| transition warp↔shader | 4.2 ms | 4.0 / 0.2 | 4.8 | 236 |
| transition shapes↔shader | 4.3 ms | 4.1 / 0.2 | 4.8 | 233 |

Findings: the bottleneck is **CPU, not GPU** — GPU work is ~0.2 ms everywhere,
and the cost is the per-pixel mesh **eval** (heavy presets 2.8 ms vs 1.0 ms
baseline). A crossfade costs ≈ the sum of both presets' CPU (as designed) and
still holds 60 fps with >3× headroom — transitions are not a perf risk. No
optimization was warranted by the data. The per-pixel eval is the thing to
watch if much higher resolutions or weak integrated GPUs become targets.
On-device frame timing is available live with `PM_PERF=1 cargo run -p pm-app`.

## Parity strategy

Each crate is validated against the C++ original before moving on:
- `pm-eval`: differential tests vs. ns-eel expected values (see crate tests).
- `pm-audio`: synthetic-signal FFT/loudness snapshots.
- `pm-render`/`pm-preset`: per-frame pixel-diff of reference presets vs. C++
  projectM screenshots (golden-image tests) within a tolerance.
