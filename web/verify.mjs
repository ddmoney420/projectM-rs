// Phase 5 browser verification: drives the real installed Chrome (headed, real
// GPU so WebGPU works), exercises preset/shader/audio/tempo/controls/overlay,
// and saves screenshots + the console log for review. Not committed.
import { chromium } from 'playwright';
import { mkdirSync, writeFileSync } from 'node:fs';

const OUT = new URL('./shots/', import.meta.url);
mkdirSync(OUT, { recursive: true });
const shot = (page, name) => page.screenshot({ path: new URL(`${name}.png`, OUT).pathname.replace(/^\//, '') });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A 4 s stereo WAV: 120 BPM bass thumps + a mid tone, so bass/mid/treb and the
// tempo detector all have something to chew on.
function testWav() {
  const sr = 44100, secs = 4, n = sr * secs;
  const buf = Buffer.alloc(44 + n * 4);
  buf.write('RIFF', 0); buf.writeUInt32LE(36 + n * 4, 4); buf.write('WAVE', 8);
  buf.write('fmt ', 12); buf.writeUInt32LE(16, 16); buf.writeUInt16LE(1, 20);
  buf.writeUInt16LE(2, 22); buf.writeUInt32LE(sr, 24); buf.writeUInt32LE(sr * 4, 28);
  buf.writeUInt16LE(4, 32); buf.writeUInt16LE(16, 34);
  buf.write('data', 36); buf.writeUInt32LE(n * 4, 40);
  for (let i = 0; i < n; i++) {
    const t = i / sr;
    const beat = t % 0.5;                       // 120 BPM
    const kick = Math.exp(-beat * 30) * Math.sin(2 * Math.PI * 60 * t) * 0.9;
    const mid = Math.sin(2 * Math.PI * 440 * t) * 0.15;
    const hat = (beat < 0.02 ? Math.sin(2 * Math.PI * 8000 * t) * 0.1 : 0);
    const s = Math.max(-1, Math.min(1, kick + mid + hat));
    const v = (s * 32767) | 0;
    buf.writeInt16LE(v, 44 + i * 4);
    buf.writeInt16LE(v, 44 + i * 4 + 2);
  }
  const p = new URL('./test.wav', OUT).pathname.replace(/^\//, '');
  writeFileSync(p, buf);
  return p;
}

const logs = [];
async function pickExample(page, label) {
  await page.selectOption('#sc-example', { label });
  await sleep(400);
}

let browser;
const run = async () => {
  const wav = testWav();
  browser = await chromium.launch({
    channel: 'chrome',
    headless: false,
    args: ['--enable-unsafe-webgpu', '--autoplay-policy=no-user-gesture-required'],
  });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  page.on('console', (m) => logs.push(`[${m.type()}] ${m.text()}`));
  page.on('pageerror', (e) => logs.push(`[pageerror] ${e.message}`));

  await page.goto('http://localhost:5174/', { waitUntil: 'load' });
  await sleep(3500); // wait for WebGPU init + render loop
  await shot(page, '01-preset');

  // Audio: load the test WAV (trusted change event → AudioContext resumes).
  await page.setInputFiles('#file-input', wav);
  await sleep(2500);
  await shot(page, '02-audio-milkdrop'); // preset + panel bars moving

  // Live shader console.
  await page.click('#console-btn');
  await sleep(400);
  await pickExample(page, 'Radial spectrum');
  await page.click('#sc-compile');
  await sleep(1500);
  await shot(page, '03-shader-radial');

  await pickExample(page, 'Beat-pulsing plasma');
  await page.click('#sc-compile');
  await sleep(1500);
  await shot(page, '04-beat-plasma');

  // User controls (parses @control → sliders in the Controls panel).
  await pickExample(page, 'Bass color shift (controls)');
  await page.click('#sc-compile');
  await sleep(800);
  await page.click('#controls-btn');
  await page.evaluate(() => document.querySelectorAll('#controls details').forEach((d) => (d.open = true)));
  await sleep(400);
  await shot(page, '05-user-controls');

  // Overlay on (over the shader).
  await page.check('#ov-en');
  await page.selectOption('#ov-mode', '0'); // oscilloscope
  await sleep(1200);
  await shot(page, '06-overlay-osc');
  await page.selectOption('#ov-mode', '5'); // lissajous
  await sleep(1000);
  await shot(page, '07-overlay-lissajous');

  // Invalid shader → last-known-good must remain; audio must keep flowing.
  // (Console panel is still open from earlier.)
  await page.click('.cm-content');
  await page.keyboard.press('Control+A');
  await page.keyboard.type('this is not glsl @@@');
  await page.keyboard.press('Control+Enter');
  await sleep(1000);
  await shot(page, '08-invalid-kept');

  // Back to Milkdrop.
  await page.click('#src-preset');
  await sleep(800);
  await shot(page, '09-back-to-milkdrop');

  // Grab the diagnostics panel text for the record.
  const diag = await page.evaluate(() => document.getElementById('diag-body')?.innerText ?? '(none)');
  logs.push('--- diag-body ---\n' + diag);

  await browser.close();
  writeFileSync(new URL('./console.log', OUT).pathname.replace(/^\//, ''), logs.join('\n'));
  console.log('DONE. screenshots + console.log in web/shots/');
};

run().catch(async (e) => {
  logs.push('[script-error] ' + (e?.stack || e));
  try { writeFileSync(new URL('./console.log', OUT).pathname.replace(/^\//, ''), logs.join('\n')); } catch {}
  try { if (browser) await browser.close(); } catch {}
  console.error('FAILED:', e?.message || e);
  process.exit(1);
});
