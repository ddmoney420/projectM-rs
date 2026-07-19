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
  // Share URL copies to the clipboard; grant so copyShareUrl takes the happy path.
  try { await page.context().grantPermissions(['clipboard-read', 'clipboard-write']); } catch {}

  // Console + Layers are mutually-exclusive left panels; open the right one.
  const openLayers = async () => { if (!(await page.locator('#layers.open').count())) await page.click('#layers-btn'); await sleep(250); };
  const openConsole = async () => { if (!(await page.locator('#console.open').count())) await page.click('#console-btn'); await sleep(250); };
  const rows = () => page.locator('#lp-list .lp-row').count();
  // Range inputs need value + input event (Playwright fill() rejects type=range).
  const setRange = (sel, val) => page.$eval(sel, (e, v) => { e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); }, String(val));

  // ?miditest exposes window.__pmMidi (synthetic MIDI injection) in the prod build.
  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3500);
  await shot(page, 'p6-01-default'); // default scene: Milkdrop + Waveform

  await page.setInputFiles('#file-input', wav);
  await sleep(2000);

  // Default stack is Milkdrop + Waveform.
  await openLayers();
  await shot(page, 'p6-02-layers-default');
  results.defaultRows = await rows();

  // Add a Shader layer. Before compiling anything it must be transparent (an
  // un-compiled shader layer must NOT black out the stack).
  await page.click('.lp-add button[data-k="1"]');
  await sleep(500);
  await shot(page, 'p7-04-empty-shader-nonblack');
  // Then make it semi-transparent so Milkdrop shows through.
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

  // Phase 6 regression: duplicate a layer.
  const rowsBeforeDup = await rows();
  await page.locator('#lp-list .lp-row').first().locator('.dup').click();
  await sleep(400);
  results.dupLayerAdded = (await rows()) === rowsBeforeDup + 1;

  // --- Phase 7: effects ---------------------------------------------------
  await page.click('#effects-btn'); // right panel, Global mode by default
  await sleep(300);
  await page.selectOption('#fx-type', 'bloom');
  await page.click('#fx-add');
  await sleep(400);
  await page.selectOption('#fx-type', 'feedback');
  await page.click('#fx-add');
  await sleep(900);
  results.globalEffects = await page.locator('#fx-list .fx-row').count();
  await shot(page, 'p7-01-global-bloom-feedback');
  await sleep(700);
  await shot(page, 'p7-02-feedback-trail'); // temporal — should differ from p7-01

  // Per-layer effect: select a layer, switch Effects to the Layer tab, add one.
  await openLayers();
  await page.locator('#lp-list .lp-row').first().locator('.nm').click();
  await sleep(200);
  await page.click('#fx-layer');
  await sleep(200);
  await page.selectOption('#fx-type', 'kaleidoscope');
  await page.click('#fx-add');
  await sleep(500);
  results.layerEffects = await page.locator('#fx-list .fx-row').count();
  await shot(page, 'p7-03-layer-effect');

  // Effects serialize into the persisted scene.
  const sceneEff = await page.evaluate(() => localStorage.getItem('pm-web-scene-v1'));
  const se = sceneEff ? JSON.parse(sceneEff) : {};
  results.sceneGlobalEffects = (se.global_effects || []).length;
  results.sceneLayerHasEffects = (se.layers || []).some((l) => (l.effects || []).length > 0);

  // --- Per-layer transform (Phase 6 follow-up) ----------------------------
  await openLayers();
  // Select the Waveform layer (duplicable, visible) and transform it.
  await page.locator('#lp-list .lp-row').filter({ has: page.locator('.nm[title="waveform"]') }).first().locator('.nm').click();
  await sleep(300);
  await setRange('#lp-transform .tx', 0.3);
  await setRange('#lp-transform .sx', 0.6);
  await setRange('#lp-transform .sy', 0.6);
  await setRange('#lp-transform .rot', 0.5);
  await sleep(400);
  await shot(page, 'p6-10-transformed');
  {
    const scn = JSON.parse((await page.evaluate(() => localStorage.getItem('pm-web-scene-v1'))) || '{}');
    const wf = (scn.layers || []).find((l) => l.source.kind === 'waveform');
    results.transformSaved = !!wf && Math.abs(wf.transform.pos[0] - 0.3) < 0.03 && Math.abs(wf.transform.scale[0] - 0.6) < 0.03;
  }
  // Duplicate the transformed layer → the copy inherits the transform.
  await page.locator('#lp-list .lp-row.sel .dup').click();
  await sleep(400);
  {
    const scn = JSON.parse((await page.evaluate(() => localStorage.getItem('pm-web-scene-v1'))) || '{}');
    const transformedWf = (scn.layers || []).filter((l) => l.source.kind === 'waveform' && Math.abs(l.transform.pos[0] - 0.3) < 0.03);
    results.dupInheritsTransform = transformedWf.length >= 2;
  }
  // Reset transform on the current (duplicate) selection → identity.
  await page.locator('#lp-transform .reset').click();
  await sleep(300);
  results.resetTransform = (await page.locator('#lp-transform .tx').inputValue()) === '0';

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

  // --- Phase 8: output & sharing ------------------------------------------

  // Recording: start, let a couple of frames capture, stop → a WebM download.
  const recBtn = page.locator('#rec-btn');
  const dlPromise = page.waitForEvent('download', { timeout: 8000 }).catch(() => null);
  await recBtn.click();
  await sleep(300);
  results.recStarted = (await recBtn.textContent())?.includes('Stop') === true;
  await sleep(1500);
  await recBtn.click(); // stop → triggers download
  const dl = await dlPromise;
  if (dl) {
    const recPath = p('recording.webm');
    await dl.saveAs(recPath);
    results.recFilename = dl.suggestedFilename();
    results.recBytes = readFileSync(recPath).length;
  }
  results.recDownloaded = !!dl && results.recBytes > 0;

  // Share URL round-trip: capture the current scene's layer signature, build a
  // share URL, open it in a fresh page, and confirm the scene is restored.
  const sig = (scn) => (scn.layers || []).map((l) => l.source.kind).join(',');
  const beforeScene = JSON.parse((await page.evaluate(() => localStorage.getItem('pm-web-scene-v1'))) || '{}');
  results.shareBeforeSig = sig(beforeScene);
  await page.locator('#share-btn').click();
  await sleep(500);
  const shareUrl = await page.evaluate(() => location.href);
  results.shareUrlHasScene = shareUrl.includes('#s=');

  // Fresh page loads the shared URL. Clear localStorage first so a restored
  // scene can only come from the URL fragment, not prior persistence.
  const page2 = await browser.newPage({ viewport: { width: 1000, height: 700 } });
  page2.on('console', (m) => logs.push(`[p2:${m.type()}] ${m.text()}`));
  page2.on('pageerror', (e) => logs.push(`[p2:pageerror] ${e.message}`));
  await page2.goto(URL_BASE, { waitUntil: 'load' });
  await page2.evaluate(() => localStorage.removeItem('pm-web-scene-v1'));
  await page2.goto(shareUrl, { waitUntil: 'load' });
  await sleep(3500);
  const afterScene = JSON.parse((await page2.evaluate(() => localStorage.getItem('pm-web-scene-v1'))) || '{}');
  results.shareAfterSig = sig(afterScene);
  results.shareRoundTrip = results.shareBeforeSig.length > 0 && results.shareAfterSig === results.shareBeforeSig;
  await shot(page2, 'p8-01-shared-scene-restored');
  await page2.close();

  // Fullscreen last (it puts the canvas in the top layer): a Playwright click is
  // a trusted gesture so requestFullscreen should take. Exit programmatically —
  // the Escape key is unreliable under automation.
  await page.locator('#full-btn').click();
  await sleep(400);
  results.fullscreenEntered = await page.evaluate(() => document.fullscreenElement != null);
  await page.evaluate(() => document.exitFullscreen?.());
  await sleep(300);

  // --- Phase 8b: Web MIDI control (synthetic injection) -------------------
  // Real hardware is unavailable to Playwright, so we drive the SAME handler
  // real Web-MIDI uses via the ?miditest hook (window.__pmMidi).
  const CH0 = 0xb0, CH3 = 0xb3, NOTE_ON = 0x90, NOTE_OFF = 0x80;
  const inject = (dev, s, d1, d2) => page.evaluate(([a, b, c, e]) => window.__pmMidi.inject(a, b, c, e), [dev, s, d1, d2]);
  const registry = async () => JSON.parse(await page.evaluate(() => window.__pmMidi.registry()));
  const mappings = async () => JSON.parse(await page.evaluate(() => window.__pmMidi.mappings()));
  const value = async (p) => page.evaluate((pp) => window.__pmMidi.value(pp), p); // hook returns an object
  const setField = (id, f, v) => page.evaluate(([i, ff, vv]) => window.__pmMidi.setField(i, ff, vv), [id, f, String(v)]);
  const mapById = async (id) => (await mappings()).find((m) => m.id === id);
  const openMidi = async () => { if (!(await page.locator('#midi.open').count())) await page.click('#midi-btn'); await sleep(150); };
  const openEffects = async () => { if (!(await page.locator('#effects.open').count())) await page.click('#effects-btn'); await sleep(150); };
  // Learn a mapping via the panel workflow; returns the new mapping id.
  const learn = async (target, s, d1, d2, dev = 'testdev') => {
    await openMidi();
    await page.waitForFunction((p) => { const el = document.getElementById('midi-target'); return !!el && Array.from(el.options).some((o) => o.value === p); }, target, { timeout: 3000 });
    const before = (await mappings()).map((m) => m.id);
    await page.selectOption('#midi-target', target);
    await page.click('#midi-learn-btn');
    await sleep(120);
    await inject(dev, s, d1, d2);
    await sleep(200);
    const fresh = (await mappings()).find((m) => !before.includes(m.id));
    return fresh ? fresh.id : null;
  };

  results.midiSupported = await page.evaluate(() => 'requestMIDIAccess' in navigator);
  results.midiHookPresent = await page.evaluate(() => typeof window.__pmMidi?.inject === 'function');

  // Give the shader layer a known continuous control (deterministic target).
  await openLayers();
  await page.locator('#lp-list .lp-row').filter({ has: page.locator('.nm[title="shader"]') }).first().locator('.nm').click();
  await sleep(200);
  await page.evaluate(() =>
    window.__pmMidi.compileSelected(0, '// @control gain float 0.0 2.0 1.0\nvoid mainImage(out vec4 o, in vec2 f){ o = vec4(gain*0.5, 0.2, 0.5, 1.0); }'),
  );
  await sleep(400);

  const reg = await registry();
  const pick = (re, kind) => reg.find((t) => re.test(t.path) && (!kind || t.kind === kind));
  const opacityT = pick(/^layer\.\d+\.opacity$/);
  const controlT = pick(/^layer\.\d+\.control\.\d+$/, 'continuous');
  const effectT = pick(/effect\.\d+\.param\.0$/, 'continuous');
  const enabledT = pick(/^layer\.\d+\.enabled$/);
  const visibleT = pick(/^layer\.\d+\.visible$/);
  results.midiRegistry = { opacity: !!opacityT, control: !!controlT, effect: !!effectT, enabled: !!enabledT, count: reg.length };
  const opLayerId = Number(opacityT.path.split('.')[1]);

  // (1) MIDI Learn creates a mapping.
  const opId = await learn(opacityT.path, CH0, 20, 100);
  results.midiLearn = opId != null && (await mapById(opId)).target === opacityT.path;
  await setField(opId, 'pickup', 'false');
  await sleep(50);

  // (2) CC → opacity, and the UI reflects it. Open the layers panel first so
  //     the value-reflection tick (midiTick → syncValues) sees the update.
  await openLayers();
  await sleep(100);
  await inject('testdev', CH0, 20, 127);
  await sleep(300); // ≥2 midiTicks: drain update feed → syncValues
  results.midiCcOpacity = Math.abs((await value(opacityT.path)).value - 1.0) < 0.02;
  results.midiUiReflects = Math.abs(Number(await page.locator(`#lp-list .lp-row[data-id="${opLayerId}"] .op`).inputValue()) - 1.0) < 0.03;
  await inject('testdev', CH0, 20, 0);
  await sleep(120);
  results.midiCcOpacityZero = (await value(opacityT.path)).value < 0.02;

  // (3) Shader control + invert + range mapping.
  const ctlId = await learn(controlT.path, CH0, 21, 64);
  await setField(ctlId, 'pickup', 'false');
  await sleep(40);
  await inject('testdev', CH0, 21, 127);
  await sleep(120);
  results.midiShaderControl = Math.abs((await value(controlT.path)).value - controlT.max) < 0.05;
  await setField(ctlId, 'invert', 'true');
  await sleep(40);
  await inject('testdev', CH0, 21, 0);
  await sleep(120);
  results.midiInvert = Math.abs((await value(controlT.path)).value - controlT.max) < 0.05;
  await setField(ctlId, 'invert', 'false');
  await setField(ctlId, 'out_min', '0');
  await setField(ctlId, 'out_max', '0.5');
  await sleep(40);
  await inject('testdev', CH0, 21, 127);
  await sleep(120);
  results.midiRange = Math.abs((await value(controlT.path)).value - 0.5) < 0.03;

  // (4) Effect parameter via MIDI.
  const effId = await learn(effectT.path, CH0, 22, 100);
  await setField(effId, 'pickup', 'false');
  await sleep(40);
  await inject('testdev', CH0, 22, 127);
  await sleep(120);
  results.midiEffectParam = Math.abs((await value(effectT.path)).value - effectT.max) < (effectT.max - effectT.min) * 0.05 + 0.01;

  // (5) Toggle: Note-On flips layer.enabled each press.
  const tglId = await learn(enabledT.path, NOTE_ON, 48, 100);
  const enBefore = (await value(enabledT.path)).bool;
  await inject('testdev', NOTE_ON, 48, 100);
  await sleep(100);
  const enAfter1 = (await value(enabledT.path)).bool;
  await inject('testdev', NOTE_ON, 48, 100);
  await sleep(100);
  const enAfter2 = (await value(enabledT.path)).bool;
  results.midiToggle = enAfter1 === !enBefore && enAfter2 === enBefore;
  void tglId;

  // (6) Momentary: Note-On = on, Note-Off = off (on layer.visible).
  const momId = await learn(visibleT.path, NOTE_ON, 49, 100);
  await setField(momId, 'mode', 'momentary');
  await sleep(40);
  await inject('testdev', NOTE_OFF, 49, 0);
  await sleep(100);
  const momOff = (await value(visibleT.path)).bool;
  await inject('testdev', NOTE_ON, 49, 100);
  await sleep(100);
  const momOn = (await value(visibleT.path)).bool;
  results.midiMomentary = momOff === false && momOn === true;

  // (7) Trigger → app action (record start), via the decoupled action queue.
  await learn('app.record.toggle', NOTE_ON, 50, 100);
  await inject('testdev', NOTE_ON, 50, 100);
  await sleep(300); // action queue drained by midiTick (~100ms) → recorder starts
  results.midiTriggerAction = await page.locator('#rec-btn.on').count() > 0;
  if (results.midiTriggerAction) {
    await page.click('#rec-btn'); // stop the recording we just started
    await sleep(300);
  }

  // (8) Channel filter: a ch3 mapping ignores the same CC on ch0.
  const chId = await learn(opacityT.path, CH3, 26, 100);
  await setField(chId, 'pickup', 'false');
  await sleep(40);
  await inject('testdev', CH0, 20, 64); // opId sets opacity ≈0.5
  await sleep(80);
  await inject('testdev', CH0, 26, 127); // wrong channel for chId → ignored
  await sleep(100);
  const wrongChan = (await value(opacityT.path)).value;
  await inject('testdev', CH3, 26, 127); // right channel → applies
  await sleep(100);
  const rightChan = (await value(opacityT.path)).value;
  results.midiChannelFilter = Math.abs(wrongChan - 0.5) < 0.03 && Math.abs(rightChan - 1.0) < 0.02;

  // (9) An unmapped CC is ignored.
  await inject('testdev', CH0, 20, 38); // opacity ≈0.3
  await sleep(80);
  await inject('testdev', CH0, 77, 127); // CC77 unmapped
  await sleep(100);
  results.midiWrongCcIgnored = Math.abs((await value(opacityT.path)).value - 0.3) < 0.03;

  // (10) Soft takeover (pickup): a far value waits, a near value engages.
  await inject('testdev', CH0, 20, 102); // opacity ≈0.8 via opId
  await sleep(80);
  const puId = await learn(opacityT.path, CH0, 27, 100); // pickup on by default
  await inject('testdev', CH0, 27, 10); // ≈0.08 — far from 0.8 → wait
  await sleep(100);
  const puWait = (await value(opacityT.path)).value;
  const puEngagedBefore = (await mapById(puId)).engaged;
  await inject('testdev', CH0, 27, 100); // ≈0.79 — within threshold of 0.8 → engage
  await sleep(120);
  const puEngagedAfter = (await mapById(puId)).engaged;
  results.midiPickup = Math.abs(puWait - 0.8) < 0.03 && puEngagedBefore === false && puEngagedAfter === true;

  // (11) MIDI base composes with an audio modulation on the same param.
  await openEffects();
  await page.click('#fx-global'); // ensure global chain (Phase 7 left it on Layer)
  await sleep(150);
  await page.selectOption('#fx-type', 'brightness');
  await page.click('#fx-add');
  await sleep(300);
  await page.locator('#fx-params .fx-param').first().locator('.src').selectOption('bass');
  await sleep(250); // effects onChanged → scene saved (mod persisted)
  const reg2 = await registry();
  const brightT = reg2.filter((t) => t.path.startsWith('global.effect') && /Brightness/.test(t.label) && /param\.0$/.test(t.path)).pop();
  const brId = await learn(brightT.path, CH0, 28, 100);
  await setField(brId, 'pickup', 'false');
  await sleep(40);
  await inject('testdev', CH0, 28, 127);
  await sleep(120);
  const brBase = (await value(brightT.path)).value;
  const scnCompose = JSON.parse((await page.evaluate(() => localStorage.getItem('pm-web-scene-v1'))) || '{}');
  const brEff = (scnCompose.global_effects || []).find((e) => e.effect_type === 'brightness');
  results.midiComposeBase = brBase;
  results.midiComposeSource = brEff?.params?.[0]?.source ?? null;
  results.midiComposeWithMod = Math.abs(brBase - 1.0) < 0.05 && brEff?.params?.[0]?.source === 'bass';

  // (12) Mapping survives a layer reorder (stable id).
  await openLayers();
  await page.locator(`#lp-list .lp-row[data-id="${opLayerId}"] .up`).click();
  await sleep(200);
  await inject('testdev', CH0, 20, 127);
  await sleep(120);
  results.midiSurvivesReorder = Math.abs((await value(opacityT.path)).value - 1.0) < 0.02 && (await mapById(opId)).resolved === true;

  // (13) A deleted target becomes unresolved (no crash on later events).
  await openLayers();
  const wfRow = page.locator('#lp-list .lp-row').filter({ has: page.locator('.nm[title="waveform"]') }).first();
  const wfId = await wfRow.getAttribute('data-id');
  const delId = await learn(`layer.${wfId}.opacity`, CH0, 30, 100);
  const delResolvedBefore = (await mapById(delId)).resolved;
  await openLayers();
  await page.locator(`#lp-list .lp-row[data-id="${wfId}"] .rm`).click();
  await sleep(300);
  const delResolvedAfter = (await mapById(delId)).resolved;
  await inject('testdev', CH0, 30, 127); // must not throw
  await sleep(100);
  results.midiDeletedTargetUnresolved = delResolvedBefore === true && delResolvedAfter === false;

  // (14) Mappings persist across reload. Navigate explicitly with ?miditest —
  //      the earlier Share test's history.replaceState dropped the query, so a
  //      bare reload would lose the injection hook.
  await sleep(450); // flush debounced MIDI save
  const beforeReload = await mappings();
  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3500);
  const afterReload = await mappings();
  results.midiSurvivesReload = afterReload.length === beforeReload.length && afterReload.some((m) => m.target === opacityT.path);
  await page.click('#midi-btn'); // open the panel so the screenshot shows restored mappings
  await sleep(300);
  await shot(page, 'p8b-01-midi');

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
