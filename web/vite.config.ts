import { defineConfig, type Plugin } from 'vite';

// Cross-origin isolation (COOP/COEP) enables SharedArrayBuffer, which the
// AudioWorklet ring-buffer audio bridge will need in Phase 3. Enabling it now is
// harmless for the Phase 1 shell (everything is same-origin) and avoids a later
// header retrofit. Applied to both the dev server and `vite preview`.
function crossOriginIsolation(): Plugin {
  const headers = {
    'Cross-Origin-Opener-Policy': 'same-origin',
    'Cross-Origin-Embedder-Policy': 'require-corp',
  };
  const apply = (res: { setHeader(k: string, v: string): void }) => {
    for (const [k, v] of Object.entries(headers)) res.setHeader(k, v);
  };
  return {
    name: 'cross-origin-isolation',
    configureServer(server) {
      server.middlewares.use((_req, res, next) => {
        apply(res);
        next();
      });
    },
    configurePreviewServer(server) {
      server.middlewares.use((_req, res, next) => {
        apply(res);
        next();
      });
    },
  };
}

export default defineConfig({
  plugins: [crossOriginIsolation()],
  server: { port: 5173 },
  // The wasm-pack output is a hand-managed ESM package; don't pre-bundle it.
  optimizeDeps: { exclude: ['./src/pm_web/pm_web.js'] },
});
