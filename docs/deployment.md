# pm-web Deployment

The browser app is a **static site**. No backend is required. The only special
requirement is **cross-origin isolation** for the SharedArrayBuffer audio path.

## Required response headers

The app enables cross-origin isolation so the AudioWorklet ring buffer can use
`SharedArrayBuffer`. Serve every response (at least the HTML) with:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

(These are the exact values used by the dev/preview server — see
`web/vite.config.ts`.)

### Implications of `require-corp`
- Cross-origin **images, scripts, audio, and other subresources** must send
  `Cross-Origin-Resource-Policy` / proper CORS, or they will be blocked. The app
  bundles all of its assets, so this only matters if you add external resources.
- If you cannot set these headers, the app still runs — it falls back to a
  `postMessage` audio transport (no `SharedArrayBuffer`) with slightly higher
  latency. Rendering is unaffected. To ship without isolation, you may drop the
  headers, but keeping them is recommended.

## Build

```bash
cd web
npm install
npm run build     # wasm-pack (release) + vite build → web/dist/
```

`web/dist/` is the deployable static output. It contains `index.html`,
`output.html` (the projection window), the wasm module, and JS chunks
(`main` + lazy `shader-console` / `midi` chunks).

## Static hosts

Any static host that lets you set custom response headers works. Notes:

- **Cloudflare Pages** — add a `_headers` file to the deploy root:
  ```
  /*
    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp
  ```
- **Netlify** — add a `_headers` file (same content) or configure `netlify.toml`
  `[[headers]]`.
- **GitHub Pages** — does **not** support custom response headers, so
  cross-origin isolation cannot be enabled there; the app will run with the
  `postMessage` audio fallback. Prefer a host with header control for the best
  audio path.

Serve `index.html` as the entry; `output.html` is opened by the app as a pop-up
(same origin) and needs no special routing.
