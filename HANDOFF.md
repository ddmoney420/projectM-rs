# projectM-rs — `v0.0.1-demo`

The first demo-ready snapshot of **projectM-rs**, an **LGPL-2.1 Rust port and derivative work of projectM / Milkdrop components**, plus its runnable player, **`pm-app`**.

## Tag

- **Tag:** `v0.0.1-demo` (annotated)
- **Points to commit:** `77b005dc50c0c88a597d0c816ececd541f8fbcc4`
- A later follow-up commit, `662f85c`, only clarified wording in a developer-only example (`matrix_visual`). It does **not** change runtime behavior and is not part of the tagged state.

## What's included

- **Rust projectM/Milkdrop visualizer port:** per-frame/per-pixel equations, custom warp and composite HLSL shaders transpiled to WGSL and naga-validated, waveforms, custom shapes, feedback, motion vectors, and final composite — rendered on the GPU via `wgpu`.
- **`pm-app`:** a runnable demo/player with a live window that cycles a real `.milk` preset corpus in time with audio, with a synthetic fallback when no input device is present.
- **Real `.milk` corpus loading with robust skip handling:** invalid, unparsed, and black-rendering presets are skipped cleanly and logged, with a session summary on exit.
- **Runtime features:** transitions, HUD, on-screen help overlay, pause/freeze, single-frame step while paused, timed auto-advance, shuffle with no-repeat history, screenshots, persisted preferences, and last-preset restore.

## Verification

- ✅ Full workspace tests pass (25 test binaries)
- ✅ `clippy` clean (`--workspace --all-targets`)
- ✅ All examples compile
- ✅ `pm-app` launches and renders (verified on **Windows / NVIDIA**)
- ✅ README present and accurate

## Current shader compatibility

Driven by a real ~14k-preset corpus, the HLSL-to-WGSL transpiler produces valid, naga-validated WGSL for approximately:

- **97.0%** of warp shaders
- **96.0%** of composite shaders

Latest full corpus scan (the 9,795-preset *cream-of-the-crop* set):

```text
presets found:           9795
loaded successfully:     9758  (99.6%)
skipped (load/parse):    37
custom shaders:          15834
shader translate failed: 323   (98.0% translate OK)
```

Run your own scan any time with:

```sh
PM_SCAN=1 cargo run -p pm-app --release -- <path-to-preset-folder>
```

## How to run

```sh
cargo run -p pm-app --release -- <path-to-preset-folder>
```

Point it at a folder of `.milk` presets, such as the projectM *cream-of-the-crop* or *classic* packs. Press `/` in the window for the on-screen controls.

## License and provenance

This project is licensed as **LGPL-2.1** and includes **Rust ports / derivative work based on projectM / libprojectM and projectm-eval components**. See [`COPYING`](COPYING) and [`NOTICE`](NOTICE) for the license text and upstream attribution.

No third-party / community preset packs are bundled in this repository.

## Known limitations

- External sampler/image assets are not supported yet (only in-pipeline feedback textures are supported).
- A parser long-tail remains deferred: sampler / `sampler_state` declarations, the comma operator, arrays/initializer lists, brace initializers, and empty-statement cleanup.
- A small naga/matrix validate-stage tail remains.
- The HUD/help font is uppercase-only and has no arrow glyphs, so directions are spelled out.
- Screenshot capture is synchronous and briefly blocks the render loop.
- macOS/Linux smoke tests are still pending — expected to work via `wgpu` / `winit` / `cpal`, but not yet verified.

## Suggested next actions

- Push the tag once a remote is configured.
- Optionally create a GitHub release from this tag using this note.
- Optionally run macOS/Linux smoke tests.
- Optionally make a later style-policy decision for `rustfmt` — the codebase currently uses a consistent compact hand-style with no `rustfmt.toml`; a one-shot reformat would be large and is best deferred.
