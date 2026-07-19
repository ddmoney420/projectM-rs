# pm-web UI

Browser front-end for the projectM-rs WebGPU/WASM visualizer. See
[`../docs/pm-web-architecture.md`](../docs/pm-web-architecture.md) for the design.

## Prerequisites

- Rust with the `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/): `cargo install wasm-pack`
- Node.js 18+ and npm
- A **WebGPU-capable browser** (recent Chrome/Edge/Firefox/Safari). There is no
  WebGL fallback — unsupported browsers get an explanatory page.

## Develop

```sh
cd web
npm install
npm run dev      # builds the wasm (wasm-pack) then starts Vite on :5173
```

`npm run dev` runs `wasm:dev` first, which compiles `crates/pm-web` with
`wasm-pack` into `web/src/pm_web/` (git-ignored), then launches Vite.

## Build

```sh
npm run build    # release wasm + vite build → web/dist
npm run preview  # serve the production build locally
```

## Status

Phase 1 shell: canvas + async WebGPU init, `requestAnimationFrame` loop, resize
and surface-lost handling, WebGPU detection with a fallback page, and one known
visual (an animated gradient). The engine (`pm-core`) is wired in from Phase 2.

The Rust side is verified to compile (`cargo build --target
wasm32-unknown-unknown -p pm-web`). The Vite/browser build and live WebGPU render
require the tools above and a browser; run the commands here to verify locally.
