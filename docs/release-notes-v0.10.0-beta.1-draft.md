# projectM-rs pm-web — v0.10.0-beta.1 (DRAFT — not published)

> Draft release notes for the first Phase 10 public beta. **Not published; no tag
> created; production remains v0.0.3-web-beta.4.** Finalize after physical iPhone
> validation.

The browser VJ app grows from a single-visual player into a **performance
instrument** — browse a content library, prepare the next visual on a hidden
deck, and crossfade it live.

## New

- **Unified Content Library** — Milkdrop presets, shader projects, and scenes in
  one place.
- **Milkdrop packs + local import** — a lazy, worker-decompressed pack format
  (manifest + gzipped shards) and **local `.milk` import** (single or multiple),
  all fully local. *No third-party preset corpus is bundled.*
- **Shader & Scene library** — the built-in example shaders become first-class
  library items (full multipass preserved); **Save Shader / Save Scene** to your
  local library.
- **Virtualized browser** — fast search + type filters over thousands of items
  with a bounded DOM; **Favorites / Recent / Collections**.
- **Preview Bank** — an ordered audition queue (references only), persisted.
- **Audition deck (Deck B)** — load and run the next visual **off-air** with a
  small **preview monitor**, without disturbing the live output.
- **Dual decks + master crossfader** — an accessible **A◀▶B** crossfader (mouse /
  touch / keyboard), gamma-correct linear blend; recording and projection follow
  the **post-crossfade master**.
- **Performance controls** — **MIDI** continuous crossfader (with soft-takeover)
  + trigger actions (audition, bank next/prev, mix to A/B/center, random Milkdrop,
  favorite), and a **keyboard** layer (`[` `]` bank, `P` audition, `R` random,
  `1`/`2`/`3` mix, `Shift+←/→` nudge) — all through one command layer.
- **On-device diagnostics** — live deck sources, crossfader, and measured
  render-target count/size; a "GPU error" row.

## Capability & honesty

- **Desktop dual-Milkdrop: supported on qualified hardware** (leak-free, stable
  under a four-engine worst case). **Performance varies** with GPU capability,
  resolution, preset complexity, and simultaneous preset transitions — we don't
  promise a specific frame rate (the heavy dual-Milkdrop RC workload averaged
  ~31 FPS on the tested discrete-NVIDIA desktop).
- **Mobile dual-Milkdrop: may be restricted** by an adaptive capability policy
  (adapter limits, no user-agent sniffing); on constrained devices a second
  Milkdrop is cleanly refused with a message and the live deck stays alive —
  Milkdrop+Shader/Scene still works.
- **Physical MIDI validation: pending** — MIDI is validated with a synthetic
  harness; no physical controller has been tested yet.

## Privacy

All content, MIDI mappings, scenes, and recordings stay local; nothing is
uploaded; share URLs encode the scene in the URL fragment.

## Version

`0.10.0-beta.1` · commit `9cbbf91` (shown in About). The historical
`v0.0.3-web-beta.x` line remains as-is; this begins the new `v0.10.0` product
line.
