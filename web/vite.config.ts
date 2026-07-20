import { defineConfig, type Plugin } from 'vite';
// Node built-in (esbuild provides it when Vite loads this config); no
// @types/node dependency, so tell tsc to accept the bare import.
// @ts-expect-error node builtin resolved at build time
import { execSync } from 'node:child_process';
declare const process: { env: Record<string, string | undefined> };

// Build-time release identity (Phase 10 versioning). The deploy tooling passes
// APP_RELEASE_TAG + APP_GIT_COMMIT for the EXACT release being built (resolved
// in an isolated worktree — never current main). For a local/dev build we fall
// back to the working-tree git commit; the release tag stays empty so the app
// honestly shows "<base>-dev". No network calls are made.
function gitCommit(): string {
  if (process.env.APP_GIT_COMMIT) return process.env.APP_GIT_COMMIT;
  try {
    return execSync('git rev-parse HEAD', { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
  } catch {
    return '';
  }
}
const APP_RELEASE_TAG = process.env.APP_RELEASE_TAG ?? '';
const APP_GIT_COMMIT = gitCommit();

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
  // Inject release identity at build time (compile-time constants; the source
  // declares them via `declare const`).
  define: {
    __APP_RELEASE_TAG__: JSON.stringify(APP_RELEASE_TAG),
    __APP_GIT_COMMIT__: JSON.stringify(APP_GIT_COMMIT),
  },
  server: { port: 5173 },
  // Two pages: the control app (index.html) and the projection output window
  // (output.html) opened as a popup for a second screen / projector.
  build: {
    rollupOptions: {
      // Relative to the project root; avoids a node types dependency here.
      input: { main: 'index.html', output: 'output.html' },
    },
  },
  // The wasm-pack output is a hand-managed ESM package; don't pre-bundle it.
  optimizeDeps: { exclude: ['./src/pm_web/pm_web.js'] },
});
