# pm-web Release Checklist

The browser visualizer (`crates/pm-web-vj` + `crates/pm-web-player` + `web/`)
release gate. Run from `web/` unless noted. Real-WebGPU browser tests are a
**local** gate (hosted CI cannot run headed WebGPU reliably — see
`docs/deployment.md`). To **publish** a build to production, see
`docs/deployment.md` → *Production deploy — Cloudflare Pages* and use
`scripts/deploy-cloudflare-pages.sh`.

## One-shot verification

```bash
# from repo root
bash scripts/release-check.sh
```

This runs, in order (failing fast):

1. `cargo build --target wasm32-unknown-unknown -p pm-web-vj` (and `-p pm-web-player`)
2. `cargo build --workspace`
3. `cargo test --workspace`
4. `cd web && npx tsc --noEmit`
5. `cd web && npm run build` (wasm-pack release + `vite build`)
6. Headed-Chrome Playwright (`node web/verify.mjs`) — requires a running preview
   server on the target port (`vite preview --port 5174 --strictPort`).

## Manual gate

- [ ] Tracked working tree clean (`git status`).
- [ ] Release identity is **injected at build time** — do NOT hand-edit a beta
      number. Bump `PRODUCT_BASE_VERSION` in `web/src/version.ts` only when the
      product **minor line** changes; tag + commit come from the deploy tooling.
      See [versioning policy](versioning.md).
- [ ] Rust: `cargo build --workspace` + `cargo test --workspace` pass.
- [ ] wasm32 builds pass (`cargo build --target wasm32-unknown-unknown -p pm-web-vj` and `-p pm-web-player`).
- [ ] TypeScript: `npx tsc --noEmit` clean.
- [ ] Production build: `npm run build` produces `web/dist/`.
- [ ] Browser regression: `node verify.mjs` — all checks true, 0 WebGPU errors,
      0 WASM panics. Record the total check count.
- [ ] Bundle sizes recorded (initial JS, wasm, lazy chunks) — see
      `docs/performance-baseline.md`.
- [ ] Known issues reviewed (`docs/known-limitations.md`).
- [ ] Release notes prepared.
- [ ] Production publish (if deploying): `scripts/deploy-cloudflare-pages.sh <tag> --dry-run`
      passes, then the real run reports the canonical alias serving the expected
      hashed asset on branch `main` with COOP/COEP (see `docs/deployment.md`).

### Manual smoke tests (mark which were actually done)

- [ ] Microphone input reacts.
- [ ] Tab/system audio capture.
- [ ] Real fullscreen enter/exit.
- [ ] Second-monitor projection (move the Output window to another display).
- [ ] Screen Wake Lock during a long session.
- [ ] A long (5–10 min) recording produces a valid file.
- [ ] Physical MIDI controller (see `docs/known-limitations.md` → MIDI smoke test).

Anything not performed must be reported as **Not yet verified** in the handoff.
