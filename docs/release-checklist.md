# pm-web Release Checklist

The browser visualizer (`crates/pm-web` + `web/`) release gate. Run from `web/`
unless noted. Real-WebGPU browser tests are a **local** gate (hosted CI cannot
run headed WebGPU reliably — see `docs/deployment.md`).

## One-shot verification

```bash
# from repo root
bash scripts/release-check.sh
```

This runs, in order (failing fast):

1. `cargo build --target wasm32-unknown-unknown -p pm-web`
2. `cargo build --workspace`
3. `cargo test --workspace`
4. `cd web && npx tsc --noEmit`
5. `cd web && npm run build` (wasm-pack release + `vite build`)
6. Headed-Chrome Playwright (`node web/verify.mjs`) — requires a running preview
   server on the target port (`vite preview --port 5174 --strictPort`).

## Manual gate

- [ ] Tracked working tree clean (`git status`).
- [ ] `web/src/version.ts` `APP_VERSION` bumped.
- [ ] Rust: `cargo build --workspace` + `cargo test --workspace` pass.
- [ ] wasm32 build passes (`cargo build --target wasm32-unknown-unknown -p pm-web`).
- [ ] TypeScript: `npx tsc --noEmit` clean.
- [ ] Production build: `npm run build` produces `web/dist/`.
- [ ] Browser regression: `node verify.mjs` — all checks true, 0 WebGPU errors,
      0 WASM panics. Record the total check count.
- [ ] Bundle sizes recorded (initial JS, wasm, lazy chunks) — see
      `docs/performance-baseline.md`.
- [ ] Known issues reviewed (`docs/known-limitations.md`).
- [ ] Release notes prepared.

### Manual smoke tests (mark which were actually done)

- [ ] Microphone input reacts.
- [ ] Tab/system audio capture.
- [ ] Real fullscreen enter/exit.
- [ ] Second-monitor projection (move the Output window to another display).
- [ ] Screen Wake Lock during a long session.
- [ ] A long (5–10 min) recording produces a valid file.
- [ ] Physical MIDI controller (see `docs/known-limitations.md` → MIDI smoke test).

Anything not performed must be reported as **Not yet verified** in the handoff.
