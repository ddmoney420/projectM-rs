# pm-web Known Limitations

Only items that are actually true as of the Phase 9 release.

## Hard requirements
- **WebGPU is required.** There is no WebGL fallback. On a browser without
  WebGPU the app shows a "WebGPU required" page and does not render.

## Browser / platform
- **Web MIDI** works in Chromium (Chrome/Edge). Firefox/Safari support varies;
  the MIDI panel shows an "unavailable" state when the API is absent.
- **Tab / system audio capture** depends on OS + browser; on some platforms only
  tab audio (not full system audio) is available, and the user must tick "share
  tab audio" in the picker.
- **Recording codec** is WebM; the exact video/audio codec is browser-dependent.
- **SharedArrayBuffer audio** needs cross-origin isolation (COOP/COEP). Without
  it the audio bridge falls back to `postMessage` (slightly higher latency) —
  rendering is unaffected.
- **Wake Lock**, **Fullscreen**, and **canvas capture** are feature-detected and
  reported in About → Browser capabilities.

## Projection (second screen)
- Opening the Output window needs pop-up permission.
- The output window **mirrors** the control canvas via `captureStream()`, so its
  resolution follows the source canvas (aspect-preserved, letterboxed on a
  differently-shaped display).
- Real multi-monitor behaviour, moving between displays with different DPR/scale,
  and real fullscreen were **verified manually only** — not in automation.
- `MediaStreamTrack` `postMessage` transfer is not supported in the target
  Chrome, so the stream is shared same-origin via `window.opener` (documented in
  `docs/pm-web-architecture.md`).

## MIDI
- Verified with **synthetic** injection only. Physical-hardware behaviour has not
  been smoke-tested. See the manual test plan below.

## Shaders
- Shadertoy **Sound** and **VR** passes are not supported (Phase 8d covers Image
  + Buffer A–D only).
- External channel sources (image asset, video, webcam, keyboard texture) are
  **not implemented**; `iChannelN` supports None / Audio / Buffer A–D / Self.
- Per-pass **resolution scaling** is architected but not exposed; all passes
  render at layer/output resolution.
- Multipass execution order is fixed **A → B → C → D → Image**; a pass reading an
  earlier buffer sees this frame, self/later buffers see the previous frame
  (one-frame latency — matches Shadertoy).

## Scenes
- Persistence is **local-first** (`localStorage`); there is no multi-scene
  IndexedDB library yet — the current scene auto-saves and share/export/import
  cover portability. No backend.

## MIDI hardware smoke-test plan (manual)
```
1. Enable MIDI (MIDI panel → Enable MIDI), select the input device.
2. Learn a CC → a layer's Opacity; move the knob; confirm it tracks.
3. Learn a CC → an effect parameter; confirm it tracks.
4. Learn a pad (Note) → a trigger (e.g. Tap tempo).
5. Test soft-takeover: move the software value away, then the knob — it should
   engage only on pickup.
6. Reload the page; confirm mappings persist.
7. Reorder a layer; confirm the mapping still targets the same layer.
```
