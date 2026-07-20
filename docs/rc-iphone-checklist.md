# v0.10.0-beta.1 — physical iPhone Safari validation checklist

**Status: NOT YET PERFORMED.** This is the manual gate to complete on a real
iPhone before tagging `v0.10.0-beta.1`. Do **not** mark PASS without actually
running each step on the device. Given the project's iPhone-specific history
(getDisplayMedia crash, audio suspension, orientation freeze), this is a
recommended release gate.

Test the RC build (release identity `0.10.0-beta.1 · 9cbbf91`). Until it is
deployed to a preview URL, use a preview deployment of the RC commit (do **not**
touch production beta.4).

## Startup
- [ ] App loads; visual renders within a few seconds.
- [ ] No raw JS/WASM error banner.
- [ ] About → shows **`0.10.0-beta.1 · 9cbbf91`** (correct version + commit).

## Library
- [ ] Open **Library**; search/filter responds.
- [ ] Load a **Shader** (live → Deck A).
- [ ] Load a **Scene** if one is saved.
- [ ] Import a local **.milk** and use it (if practical); it appears immediately.

## Audition
- [ ] **Audition** a Shader → Deck B; the **preview monitor** updates.
- [ ] Deck A / master output is **unchanged** by the audition.

## Dual deck + crossfade
- [ ] Audition Milkdrop and Shader combinations.
- [ ] **Crossfade A→B** with the slider; master transitions smoothly.
- [ ] **Crossfade B→A**; smooth, no black frame.

## Dual Milkdrop (device-policy dependent)
- [ ] Attempt **Milkdrop ↔ Milkdrop** audition. Expected: **works acceptably**
      OR a **clean capability refusal/degradation message** ("Dual Milkdrop is
      unavailable…"). **Never a crash or black master.**

## Orientation (Deck B active, crossfader ~0.5)
- [ ] Portrait → landscape: rendering continues; both decks react.
- [ ] Landscape → portrait: rendering continues; no black/frozen surface.

## Background / foreground
- [ ] Background Safari (home / app switch), then return.
- [ ] **AudioContext recovers** (mic/file reactivity resumes without reload).
- [ ] Visual rendering continues.

## Preview / master isolation
- [ ] Preview monitor = **raw Deck B**; master = **the mix** (they differ mid-fade).

## Recording (if practical)
- [ ] Short recording completes without crash; captures the **post-crossfade master**.

## Final acceptance
- [ ] **0** visible GPU error (diagnostics "GPU error" row = none).
- [ ] **0** frozen master.
- [ ] **0** persistent audio-reactivity loss after orientation/background.

**Record on completion:** device model, iOS version, Safari version, and the
per-section result. Only then may the RC be considered iPhone-validated.
