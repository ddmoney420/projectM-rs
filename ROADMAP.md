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
   **Corpus shader compat: ~96.0% composite / ~97.0% warp produce valid WGSL** (up
   from 37% / 18%), via corpus-driven hardening tracked by the `shader_report`
   example: built-in noise `texsize_*` constants, HLSL implicit vector
   truncation on store *and* in binary ops (`float4 * float2` -> first two
   lanes), `const`/`static` qualifiers and `float1` scalars, top-level comma
   operator, scalar swizzles, 3D noise-volume textures, intrinsic argument-width
   coercion (`dot`/`distance`/`cross`/`reflect`/`smoothstep` and
   `min`/`max`/`clamp`/`lerp`/`pow` — truncate the wider operand, e.g.
   `dot(float4, float3)`), float `++`/`--` lowered to `+= 1`, WGSL
   reserved-word identifier escaping (a preset var named `mod`/`filter`/`move`
   -> `<name>_pm`), bool->numeric and int->float coercion in every numeric
   context (HLSL promotes comparison/logical results and ints: `f32(x > 0.5)`,
   `vec3<f32>(v > 0.5)`, `floatVar <= intVar`, `-(a < b)` -> `-(f32(a < b))`,
   `(a > b) * (c > d)` -> `f32(a > b) * f32(c > d)`, and a `bool`-returning
   function whose body is numeric -> `return (…) != 0.0`), `uv` / `uv_orig` as
   mutable function-locals (not `#define` macros) so a preset can write
   `uv.x += d` without an invalid chained-swizzle lvalue (`_uv.xy.x`), and
   `tex2D` coordinate truncation to `vec2` (HLSL uses only the first two
   components, so `tex2D(s, GetBlur1(uv))` with a float3 coord becomes `(…).xy`),
   and **matrix lowering (Bucket E)**: a `floatNxN(vec)` constructor is expanded
   to component args in source order (`float2x2(_qb)` -> `mat2x2<f32>(qb.x, qb.y,
   qb.z, qb.w)`, only when the vector width matches the matrix scalar count), and
   HLSL `mul(a, b)` is lowered to WGSL `(b * a)` — the operand flip that, paired
   with the same-order constructor, reproduces HLSL's row-major `mul` under WGSL's
   column-major matrices (proven: `mul((10,20), float2x2(1,2,3,4))` = (70,100) =
   `mat2x2<f32>(1,2,3,4) * vec2(10,20)`; scalar/component-wise `mul` is unchanged
   since the flip is commutative there). This cleared the `cannot cast` parse
   bucket (306 -> 9) for +255 valid shaders and zero valid-set regressions; it
   also *corrects* ~half of the previously-valid matrix-`mul` shaders that were
   silently transposed (verified by before/after render snapshots in
   `bucket-e-visuals/`, since a valid-set diff cannot detect a corrected-but-
   still-valid render — swirl/rotation direction flips, output stays coherent).
   and **runtime global-initializer lowering (Bucket F)**: a module-scope preset
   global whose initializer is not a WGSL const-expression (reads a uniform,
   another global, or a call — illegal in a module-scope initializer) is emitted
   as an uninitialized `var<private>` and its initializer is replayed as an
   assignment at the top of the `PS` entry, in declaration order (after
   `load_uniforms`, before the body). Keeping them `var<private>` (not PS-local
   `let`) preserves visibility for the helper functions that read them — 121 of
   210 F shaders do; `var` keeps mutated globals writable; declaration order
   preserves inter-global dependencies. This cleared the "Unexpected
   runtime-expression" parse bucket (210 -> 0) for +81 valid shaders and zero
   valid-set regressions (the other ~129 F shaders hit unrelated *secondary*
   errors once F clears — `modf` arity, `InvalidStoreTypes`, bool/int->vec
   auto-convert — separate buckets, not regressions). No currently-valid shader
   is touched (only non-compiling shaders have this pattern), so no visual diff
   was required; spot-renders of newly-valid F presets produce coherent output.
   The Bucket-F prologue assignments then revealed an **assignment-target
   coercion** gap: the replayed global initializers were emitted with raw
   `expr()` rather than the `emit_broadcast()` path a `Decl` init / `Expr::Assign`
   RHS uses, so a vec3 global initialized from a vec4 expression (`float3 suncol
   = .5 + normalize(roam_cos)`) stored vec4 into vec3 (`InvalidStoreTypes`) and a
   float global from a bool comparison (`float iter6 = rand_preset.z > .5`) stored
   bool into f32 (`automatic conversions cannot convert`). Routing the prologue
   through `emit_broadcast` (truncate wider vector to the declared width, bool/int
   -> float) cleared `InvalidStoreTypes` 68 -> 3 and the bool/int auto-convert
   bucket 23 -> 4 for +84 valid shaders, zero regressions. (The 3 residual stores
   are `mat3x4 * vec3` body products mis-typed as vec3 by `infer` — a matrix-width
   typing issue, deferred; the body `Expr::Assign` path itself already coerced
   correctly.) The coercion path was then completed at the **function return**:
   `emit_return_value` routes a value `return <expr>` through the same
   `emit_broadcast(expr, declared_return_type)` so a function declared narrower
   than its body's expression truncates (vec4 -> `f32`/`.x`, `-> vec3`/`.xyz`,
   `-> vec2`/`.xy`) and int/bool convert to float, alongside the pre-existing
   numeric->bool mask coercion. This cleared `InvalidReturnType` 22 -> 2 (the 2
   residual are *valueless*/fallthrough returns in a typed function — out of
   scope, they'd need a synthesized value) for +20 valid shaders, zero
   regressions; matching-type returns are byte-identical. Finally, **`all`/`any`
   numeric-vector coercion**: HLSL `all(x)`/`any(x)` treat a numeric vector as
   true where nonzero, but WGSL requires a bool vector, so `all(modi)` (modi a
   `float2`) now emits `all(modi != vec2<f32>(0.0))` (and `vecN<i32>(0)` for int
   vectors); a bool-vector arg is passed through unchanged. This cleared the
   `InvalidBooleanVector` bucket 27 -> 0 for +27 valid shaders, zero regressions.
   The remaining naga rejections, by descending frequency (`parse_buckets` /
   `validate_kinds` / `bucket_subgroups` examples): the `cannot cast` vec4->vec2
   matrix-index E tail (9), the 3 `mat3x4 * vec3` mis-typed-store residual, 2
   valueless returns, and small unary/binary tails (~15 validate-stage total).
   The larger remaining work is the translate-stage *parser* long-tail. The first
   slice of that is done: **`double`/`doubleN` are aliased to `float`/`floatN`**
   in the parser's type table (Milkdrop/projectM treat double as float on the
   GPU), so `double3 blur = GetBlur1(uv)` parses as a `float3` declaration. This
   cleared the largest parser cluster (~127 shaders failing at a `double`-typed
   identifier — `ist`/`crisp`/`zz`/`blur`) for +82 valid shaders, zero
   regressions (no valid shader used `double`, which didn't parse before); the
   other ~45 cascaded to unrelated secondary errors. The remaining translate-
   stage buckets are deferred by nature: **sampler/`sampler_state` declarations**
   (external texture assets, ~51), the parenthesized **comma operator** `(a,b)`
   (ambiguous C-comma vs vector, ~81), **array declarations + initializer lists**
   `float4 s[5]={…}` + indexing (~75), **brace initializers** `={…}` (~53), and
   empty-statement `;;` (~67); plus the composite `noise`/scope name-collision
   parse bucket (naga side). These need an asset system, array support, or
   semantic disambiguation rather than narrow coverage.
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
   flow. **Feedback-textured custom shapes**: a shape with the `textured` flag
   samples the warp feedback buffer (the previous-frame ping-pong texture,
   hazard-free vs. the one being drawn into) at per-vertex UVs derived from
   `tex_ang`/`tex_zoom` (port of `CustomShape`'s UV math), stamping a
   zoomed/rotated copy of the current image; untextured shapes are unchanged and
   a missing/mismatched UV set falls back to the flat-colour fill.
   **Not yet supported:** external image-file shape textures (`sampler_<file>`)
   — only the in-pipeline feedback texture is sampled. Remaining: external
   preset image assets (texture files).
6. **pm-core + pm-format + pm-app** — orchestrator, native format + importer,
   live windowed app (winit + cpal). ← *done*. pm-core's WarpEngine drives
   warp+waveform+composite; **`PresetPlayer`** wraps it to crossfade between
   presets (Milkdrop's ~2.7s soft cut): both the outgoing and incoming presets
   keep rendering independently and are blended over an elapsed-time window, the
   outgoing one dropped when the fade completes (duration 0 = hard cut).
   **pm-app** is a live winit window with a wgpu surface, cpal audio capture
   (graceful synthetic fallback), transition-blended preset cycling over the
   corpus, and keyboard controls: →/Space/N next · ←/P prev · R random · **F5/L
   reload current** · T transitions · **F perf overlay** · **H in-window HUD** ·
   **Pause/K freeze** · **A auto-advance (`[`/`]` interval)** · Esc/Q quit.
   **Auto-advance** (off by default, default 30 s, adjustable in 5 s steps down to
   5 s) cycles to the next *renderable* preset on a wall-clock timer that pauses
   with freeze and resumes without losing remaining time; any manual nav/reload
   resets it so the shown preset gets a full interval, skipped presets don't
   consume it, and it uses transitions when enabled. The HUD shows `AUTO nS / NS`.
   **Freeze** is an exact last-frame hold: while
   paused the app skips `player.render` entirely (so preset time, the frame
   counter, feedback iteration, and transitions all stop — no state mutation) and
   re-presents the last frame, keeping the window responsive; the time base is
   advanced by the wall delta so playback resumes without a jump. Navigation /
   reload still work while frozen (hard-cut to the new preset, shown frozen until
   unpaused), and the HUD shows `PAUSED`. The **HUD** (on by default) draws the current preset name,
   `[idx/total]`, transition state (XFADE/CUT), session skip counts, and a perf
   indicator in the top-left corner via a dependency-free embedded 5×7 bitmap
   font — CPU-rasterized into a small texture only when the text changes, then
   composited by an alpha-blended pass *after* the blit so the visualizer's
   framebuffer/feedback are untouched (uppercase-only; unsupported glyphs blank).
   Navigation
   probes candidates at low-res and **skips presets that fail to parse or render
   black, logging the reason and a per-jump skip tally** (and a session summary
   on exit) so a bad preset never black-screens or crashes the player. `PM_SCAN`
   prints a one-time CPU-only corpus compatibility summary at startup (presets
   found / loaded / skipped / custom-shader translate rate); on the 9,795-preset
   cream-of-the-crop set: 99.6% load, 98.0% shader-translate. **pm-format** is the native `.pmp`
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

## Regression snapshots

`pm-core`'s `snapshot` example renders a fixed set of presets (simple warp,
waveform-heavy, custom shape, feedback-textured shape, motion vectors, and a
crossfade captured at exactly 50%) with fixed inputs — fixed 256×256 canvas,
deterministic synthetic audio, a fixed frame sequence — and diffs them against
committed baseline PNGs in `crates/pm-core/tests/snapshots/`. This catches
visual regressions in *our own* renderer; it is **not** C++/projectM parity.

- `cargo run -p pm-core --example snapshot --release` — check (exit 1 on
  mismatch; writes actual + amplified-diff PNGs to `target/snapshot-out/`).
- `... -- --update` (or `PM_SNAPSHOT_UPDATE=1`) — regenerate baselines
  (intentional, never automatic).
- Comparison uses a small per-channel tolerance and reports max/mean delta +
  changed-pixel count. It's an example, not a `cargo test`, so the default
  suite stays deterministic and GPU-independent; it skips cleanly with no GPU.
  The renderer is bit-exact run-to-run on a given GPU (observed 0 delta).

## Parity strategy

Each crate is validated against the C++ original before moving on:
- `pm-eval`: differential tests vs. ns-eel expected values (see crate tests).
- `pm-audio`: synthetic-signal FFT/loudness snapshots.
- `pm-render`/`pm-preset`: per-frame pixel-diff of reference presets vs. C++
  projectM screenshots (golden-image tests) within a tolerance — still the main
  outstanding parity work (distinct from the self-baseline snapshots above).
