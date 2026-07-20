// About / Help overlay + first-run onboarding. A single dimmed overlay hosts a
// centered card; the same overlay serves the welcome flow and the About panel.
// Everything is local — no account, no server, no telemetry.

import { APP_VERSION, BUILD_MODE, GIT_COMMIT, RELEASE_TAG } from './version';
import { detectCapabilities } from './capabilities';

const ONBOARD_KEY = 'pm-web-onboarded-v1';

function esc(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c] || c);
}

function overlay(): { root: HTMLElement; card: HTMLElement } {
  let root = document.getElementById('pm-overlay');
  if (!root) {
    root = document.createElement('div');
    root.id = 'pm-overlay';
    root.innerHTML = '<div class="pm-ov-card"></div>';
    root.addEventListener('click', (e) => {
      if (e.target === root) hide();
    });
    document.body.appendChild(root);
  }
  return { root, card: root.querySelector('.pm-ov-card') as HTMLElement };
}

function show(): void {
  overlay().root.classList.add('show');
}
export function hide(): void {
  document.getElementById('pm-overlay')?.classList.remove('show');
}

function capabilityRows(): string {
  return detectCapabilities()
    .map((c) => `<div class="pm-cap"><span class="${c.ok ? 'ok' : 'no'}">${c.ok ? '✓' : '✗'}</span><b>${esc(c.name)}</b><span class="note">${esc(c.note)}</span></div>`)
    .join('');
}

/** The About / Help panel. */
export function showAbout(): void {
  const { card } = overlay();
  card.innerHTML = `
    <button class="pm-ov-x" title="Close">×</button>
    <h1>projectM-rs <span class="ver">${esc(APP_VERSION)} · ${esc(GIT_COMMIT)}${RELEASE_TAG ? '' : ' · ' + BUILD_MODE}</span></h1>
    <p>A browser music visualizer: Milkdrop presets, Shadertoy-style GLSL, a layer
       compositor, effects, tempo/beat reactivity, MIDI control, scene sharing,
       recording, and second-screen projection — rendered on WebGPU.</p>

    <h2>Requirements</h2>
    <p>A WebGPU browser (recent Chrome, Edge, Firefox, or Safari) with current GPU
       drivers. There is <strong>no WebGL fallback</strong>.</p>

    <h2>Quick start</h2>
    <ol>
      <li>Load an audio file or enable the microphone.</li>
      <li>Open <b>Layers</b> or the <b>Console</b> and pick a visual / shader.</li>
      <li>Use <b>Controls</b> to bind reactivity, tempo, and modulation.</li>
      <li>Go fullscreen, or open <b>Output</b> for a second screen / projector.</li>
    </ol>

    <h2>Keyboard</h2>
    <ul class="pm-keys">
      <li><kbd>Ctrl/Cmd</kbd>+<kbd>Enter</kbd> compile the active shader pass</li>
      <li><kbd>Esc</kbd> leave Clean Output</li>
      <li><kbd>d</kbd> toggle diagnostics in the output window</li>
    </ul>

    <h2 class="warn">⚠ Photosensitivity</h2>
    <p>This app generates <strong>flashing, rapidly-changing visuals</strong>. Some
       presets and effects (strobe, glitch, feedback, beat pulses) can flash
       quickly. If you are photosensitive, avoid strobe-like content and keep the
       window small.</p>

    <h2>Privacy</h2>
    <ul>
      <li>Audio (file, mic, tab) is analyzed <b>locally</b> — never uploaded.</li>
      <li>Shader source stays local unless you explicitly share or export it.</li>
      <li>Share URLs encode the scene in the URL <b>fragment</b> (client-side only).</li>
      <li>Recordings and MIDI stay on your machine. No account, no telemetry.</li>
    </ul>

    <h2>Notes</h2>
    <ul>
      <li><b>MIDI</b> — Chrome/Edge; enable it in the MIDI panel (gesture-gated, no SysEx).</li>
      <li><b>Projection</b> — opens a second window that mirrors the canvas; allow pop-ups.</li>
      <li><b>Recording</b> — WebM; the exact codec varies by browser.</li>
      <li><b>Shaders</b> — your source stays yours; the app never adds licensing claims. Bundled examples are original/project-owned.</li>
    </ul>

    <h2>Browser capabilities</h2>
    <div class="pm-caps">${capabilityRows()}</div>

    <div class="pm-ov-actions">
      <button id="pm-ov-welcome">Show welcome again</button>
      <button id="pm-ov-close" class="primary">Close</button>
    </div>`;
  card.querySelector('.pm-ov-x')!.addEventListener('click', hide);
  card.querySelector('#pm-ov-close')!.addEventListener('click', hide);
  card.querySelector('#pm-ov-welcome')!.addEventListener('click', () => {
    try {
      localStorage.removeItem(ONBOARD_KEY);
    } catch {
      /* ignore */
    }
    showOnboarding();
  });
  show();
}

/** First-run welcome (once, unless reopened from Help). */
export function maybeShowOnboarding(): void {
  let seen = false;
  try {
    seen = !!localStorage.getItem(ONBOARD_KEY);
  } catch {
    seen = false;
  }
  if (!seen) showOnboarding();
}

function showOnboarding(): void {
  const { card } = overlay();
  card.innerHTML = `
    <button class="pm-ov-x" title="Close">×</button>
    <h1>Welcome to projectM-rs</h1>
    <p>A WebGPU music visualizer. Here's the quick path:</p>
    <ol>
      <li><b>Load audio</b> or enable the <b>microphone</b>.</li>
      <li>Pick a <b>preset or shader</b> (Layers / Console).</li>
      <li>Try an <b>effect</b> and bind some <b>reactivity</b>.</li>
      <li>Go <b>fullscreen</b> or open a projection <b>Output</b>.</li>
    </ol>
    <p class="warn">⚠ Reactive visuals can flash rapidly — see Help if you are photosensitive.</p>
    <label class="pm-ov-dns"><input type="checkbox" id="pm-ov-dns" checked /> Don't show this again</label>
    <div class="pm-ov-actions">
      <button id="pm-ov-start" class="primary">Get started</button>
    </div>`;
  const finish = () => {
    const dns = (card.querySelector('#pm-ov-dns') as HTMLInputElement)?.checked;
    if (dns) {
      try {
        localStorage.setItem(ONBOARD_KEY, '1');
      } catch {
        /* ignore */
      }
    }
    hide();
  };
  card.querySelector('.pm-ov-x')!.addEventListener('click', finish);
  card.querySelector('#pm-ov-start')!.addEventListener('click', finish);
  show();
}
