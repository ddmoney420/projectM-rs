// Browser verification harness. Drives the installed Chrome headed (real GPU so
// WebGPU works), exercises the layer compositor / scene flows (Phase 6) on top
// of audio + shaders, and saves screenshots + console log to web/shots/
// (git-ignored). Not part of the shipped app.
import { chromium } from 'playwright';
import { mkdirSync, writeFileSync, readFileSync } from 'node:fs';

const OUT = new URL('./shots/', import.meta.url);
mkdirSync(OUT, { recursive: true });
const p = (name) => new URL(name, OUT).pathname.replace(/^\/([A-Za-z]:)/, '$1');
const shot = (page, name) => page.screenshot({ path: p(`${name}.png`) });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const URL_BASE = process.env.PMW_URL || 'http://localhost:5174/';

function testWav() {
  const sr = 44100, secs = 4, n = sr * secs;
  const buf = Buffer.alloc(44 + n * 4);
  buf.write('RIFF', 0); buf.writeUInt32LE(36 + n * 4, 4); buf.write('WAVE', 8);
  buf.write('fmt ', 12); buf.writeUInt32LE(16, 16); buf.writeUInt16LE(1, 20);
  buf.writeUInt16LE(2, 22); buf.writeUInt32LE(sr, 24); buf.writeUInt32LE(sr * 4, 28);
  buf.writeUInt16LE(4, 32); buf.writeUInt16LE(16, 34);
  buf.write('data', 36); buf.writeUInt32LE(n * 4, 40);
  for (let i = 0; i < n; i++) {
    const t = i / sr, beat = t % 0.5;
    const s = Math.max(-1, Math.min(1, Math.exp(-beat * 30) * Math.sin(2 * Math.PI * 60 * t) * 0.9 + Math.sin(2 * Math.PI * 440 * t) * 0.15));
    const v = (s * 32767) | 0;
    buf.writeInt16LE(v, 44 + i * 4); buf.writeInt16LE(v, 44 + i * 4 + 2);
  }
  const path = p('test.wav');
  writeFileSync(path, buf);
  return path;
}

const logs = [];
const results = {};
let browser;

const run = async () => {
  const wav = testWav();
  browser = await chromium.launch({ channel: 'chrome', headless: false, args: ['--enable-unsafe-webgpu', '--autoplay-policy=no-user-gesture-required'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  page.on('console', (m) => logs.push(`[${m.type()}] ${m.text()}`));
  page.on('pageerror', (e) => logs.push(`[pageerror] ${e.message}`));

  // Console + Layers are mutually-exclusive left panels; open the right one.
  const openLayers = async () => { if (!(await page.locator('#layers.open').count())) await page.click('#layers-btn'); await sleep(250); };
  const openConsole = async () => { if (!(await page.locator('#console.open').count())) await page.click('#console-btn'); await sleep(250); };
  const rows = () => page.locator('#lp-list .lp-row').count();

  await page.goto(URL_BASE, { waitUntil: 'load' });
  await sleep(3500);
  await shot(page, 'p6-01-default'); // default scene: Milkdrop + Waveform

  await page.setInputFiles('#file-input', wav);
  await sleep(2000);

  // Default stack is Milkdrop + Waveform.
  await openLayers();
  await shot(page, 'p6-02-layers-default');
  results.defaultRows = await rows();

  // Add a Shader layer, make it semi-transparent so Milkdrop shows through.
  await page.click('.lp-add button[data-k="1"]');
  await sleep(300);
  await page.locator('#lp-list .lp-row').first().locator('.op').fill('0.55');
  await page.locator('#lp-list .lp-row').first().locator('.bl').selectOption('2'); // screen
  await sleep(200);
  results.afterShaderRows = await rows();

  // Compile a shader into the selected layer via the console.
  await openConsole();
  await page.selectOption('#sc-example', { label: 'Beat-pulsing plasma' });
  await page.click('#sc-compile');
  await sleep(1200);
  await shot(page, 'p6-03-milkdrop+shader+waveform');

  // Add a Spectrum layer.
  await openLayers();
  await page.click('.lp-add button[data-k="3"]');
  await sleep(600);
  await shot(page, 'p6-04-plus-spectrum');
  results.afterSpectrumRows = await rows();

  // Reorder: move the top row down.
  await page.locator('#lp-list .lp-row').first().locator('.dn').click();
  await sleep(500);
  await shot(page, 'p6-05-reordered');

  // Toggle a layer's enable off, then on.
  const enFirst = page.locator('#lp-list .lp-row').first().locator('.en');
  await enFirst.uncheck();
  await sleep(400);
  await shot(page, 'p6-06-layer-disabled');
  await enFirst.check();
  await sleep(300);

  // Invalid shader in the shader layer: select it, break it — others persist.
  const shaderRow = page.locator('#lp-list .lp-row').filter({ has: page.locator('.nm[title="shader"]') }).first();
  await shaderRow.locator('.nm').click();
  await sleep(300);
  await openConsole();
  await page.click('.cm-content');
  await page.keyboard.press('Control+A');
  await page.keyboard.type('not valid glsl @@@');
  await page.keyboard.press('Control+Enter');
  await sleep(800);
  await shot(page, 'p6-07-invalid-shader-others-ok');
  results.errorPanel = await page.locator('#sc-errors').innerText().catch(() => '');

  // Scene export via persistence (localStorage) then import round-trip.
  await openLayers();
  const sceneJson = await page.evaluate(() => localStorage.getItem('pm-web-scene-v1'));
  results.sceneHasLayers = !!sceneJson && JSON.parse(sceneJson).layers?.length;
  results.sceneOrder = sceneJson ? JSON.parse(sceneJson).layers.map((l) => l.source.kind) : [];
  const scenePath = p('scene.json');
  writeFileSync(scenePath, sceneJson || '{}');

  // Reset to default, then import the saved scene back.
  await page.click('#lp-reset');
  await sleep(600);
  results.rowsAfterReset = await page.locator('#lp-list .lp-row').count();
  await page.setInputFiles('#lp-file', scenePath);
  await sleep(800);
  results.rowsAfterImport = await page.locator('#lp-list .lp-row').count();
  await shot(page, 'p6-08-after-import');

  // Invalid scene import must be rejected (keep current).
  const badPath = p('bad-scene.json');
  writeFileSync(badPath, '{"schema_version":999,"layers":[]}');
  const rowsBeforeBad = await page.locator('#lp-list .lp-row').count();
  await page.setInputFiles('#lp-file', badPath);
  await sleep(500);
  results.rowsAfterBadImport = await page.locator('#lp-list .lp-row').count();
  results.badImportKept = results.rowsAfterBadImport === rowsBeforeBad;

  // Resize.
  await page.setViewportSize({ width: 900, height: 600 });
  await sleep(800);
  await shot(page, 'p6-09-resized');

  results.consolePanics = logs.filter((l) => /panicked|RuntimeError|unreachable/.test(l)).length;
  results.consoleErrors = logs.filter((l) => l.startsWith('[error]') || l.startsWith('[pageerror]')).length;

  await browser.close();
  writeFileSync(p('console.log'), logs.join('\n'));
  writeFileSync(p('results.json'), JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
  console.log('DONE — screenshots + logs in web/shots/');
};

run().catch(async (e) => {
  logs.push('[script-error] ' + (e?.stack || e));
  try { writeFileSync(p('console.log'), logs.join('\n')); } catch {}
  try { writeFileSync(p('results.json'), JSON.stringify(results, null, 2)); } catch {}
  try { if (browser) await browser.close(); } catch {}
  console.error('FAILED:', e?.message || e);
  console.error('partial results:', JSON.stringify(results));
  process.exit(1);
});
