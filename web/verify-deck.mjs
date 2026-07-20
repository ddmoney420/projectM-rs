// Phase 10C.1 — performance-deck abstraction regression.
// Deck A equivalence, Deck B isolation/lifecycle, source switching, output
// contract, resize resilience, mobile-failure handling, privacy.
// Run: ( cd web && PMW_URL=http://localhost:5174/ node verify-deck.mjs )
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'node:fs';

const URL_BASE = process.env.PMW_URL || 'http://localhost:5174/';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = {};
let browser;

const run = async () => {
  browser = await chromium.launch({
    channel: 'chrome',
    headless: false,
    args: ['--enable-unsafe-webgpu', '--autoplay-policy=no-user-gesture-required'],
  });
  const page = await browser.newPage({ viewport: { width: 1000, height: 720 } });
  const errs = [];
  const uploads = [];
  page.on('pageerror', (e) => errs.push('[pageerror] ' + e.message));
  page.on('console', (m) => { if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200)); });
  page.on('request', (req) => {
    const u = new URL(req.url());
    if (req.method() !== 'GET' && req.method() !== 'HEAD') uploads.push(`${req.method()} ${req.url()}`);
    else if (!URL_BASE.startsWith(u.origin) && u.protocol !== 'data:' && u.protocol !== 'blob:') uploads.push(`EXT ${req.url()}`);
  });

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3800);

  const r = await page.evaluate(async (origin) => {
    const D = window.__pmDeck;
    const C = window.__pmContent;
    const M = window.__pmMilkdrop;
    const L = window.__pmLibrary;
    const diag = () => window.__pmDiag();
    const out = {};
    const before = new Set((await L.store.getAll()).map((i) => i.id));

    // --- Deck A equivalence: one deck, Milkdrop default, valid output ---
    const d0 = D.diag();
    out.deckAOnly = D.count() === 1 && d0.deckB === null && d0.deckA.sourceType === 'milkdrop';
    out.deckAOutputContract = d0.deckA.width > 0 && d0.deckA.height > 0 && typeof d0.deckA.format === 'string' && d0.deckA.format.length > 0;
    out.rendererAlive = diag().frame > 0;

    // --- Deck A source switching (transactional; failure preserves) ---
    const shaders = C.listBuiltinShaders();
    const rs = await C.loadShader(shaders.find((s) => (s.tags || []).includes('single-pass')).id);
    await new Promise((r) => setTimeout(r, 200));
    out.switchToShader = rs.ok === true && D.diag().deckA.shaderCount >= 1;
    // invalid shader load must preserve the deck (structurally-invalid payload)
    const badId = 'user:shader:deck-bad';
    await L.store.put({ id: badId, type: 'shader', name: 'bad', origin: 'user', favorite: false, dateAdded: Date.now(), usageCount: 0, collections: [], schemaVersion: 1 }, { passes: 'x', source: 1 });
    const shaderCountBefore = D.diag().deckA.shaderCount;
    const layerBefore = D.diag().deckA.layerCount;
    const badSwitch = await C.loadShader(badId);
    out.failedSwitchPreserves = badSwitch.ok === false && D.diag().deckA.shaderCount === shaderCountBefore;
    // switch to a scene (transactional import)
    const scene = await C.saveCurrentScene('DeckScene');
    const rsc = await C.loadScene(scene.id);
    out.switchToScene = rsc.ok === true && diag().frame > 0;

    // --- real preset text for Deck B ---
    await M.loadPack(origin + '/__testpack__/manifest.json');
    const milkItem = M.listIndex().find((i) => i.id.startsWith('pack:projectm-rs-test-fixtures'));
    const milkText = await M.presetText(milkItem.id);

    // --- Deck B isolation: create/load/unload never touch Deck A ---
    const a0 = D.diag().deckA;
    const created = D.createB();
    const a1 = D.diag().deckA;
    out.deckBCreateIsolated = created === true && D.count() === 2 && a1.shaderCount === a0.shaderCount && a1.layerCount === a0.layerCount;

    const loadB = D.loadPresetB(milkText);
    await new Promise((r) => setTimeout(r, 150));
    const a2 = D.diag();
    out.deckBLoadIsolated = loadB.ok === true && a2.deckB && a2.deckB.loaded === 'milkdrop' && a2.deckA.shaderCount === a0.shaderCount && a2.deckA.layerCount === a0.layerCount;

    // The .milk parser is lenient (odd text parses as a near-empty preset), so
    // the guarantee under test is ISOLATION + no crash regardless of the parse
    // outcome: an odd Deck B load never alters Deck A or kills the renderer.
    const badB = D.loadPresetB('not a real .milk preset {{{');
    out.deckBBadLoadGraceful = typeof badB.ok === 'boolean' && D.count() === 2 && D.diag().deckA.shaderCount === a0.shaderCount && diag().frame > 0;

    // --- output contract: both decks share format + size ---
    const dc = D.diag();
    out.deckOutputsCompatible = dc.deckB && dc.deckA.width === dc.deckB.width && dc.deckA.height === dc.deckB.height && dc.deckA.format === dc.deckB.format;

    // --- Deck B unload restores single deck; Deck A intact ---
    D.unloadB();
    out.deckBUnloadIsolated = D.count() === 1 && D.diag().deckB === null && D.diag().deckA.shaderCount === a0.shaderCount;

    // cleanup
    const now = new Set((await L.store.getAll()).map((i) => i.id));
    for (const id of now) if (!before.has(id)) await L.store.delete(id).catch(() => {});
    return out;
  }, URL_BASE.replace(/\/$/, ''));
  Object.assign(results, r);

  // --- resize with Deck B present: no crash, both decks survive, Deck A alive ---
  results.resizeWithDeckB = await page.evaluate(() => window.__pmDeck.createB()) === true;
  const f0 = await page.evaluate(() => window.__pmDiag().frame);
  await page.setViewportSize({ width: 500, height: 780 }); // portrait
  await sleep(500);
  await page.setViewportSize({ width: 900, height: 500 }); // landscape
  await sleep(500);
  const afterResize = await page.evaluate(() => ({ frame: window.__pmDiag().frame, count: window.__pmDeck.count(), gpuErr: window.__pmDiag().lastError }));
  results.resizeSurvives = afterResize.count === 2 && afterResize.frame > f0 && (afterResize.gpuErr === '' || afterResize.gpuErr === undefined);
  await page.evaluate(() => window.__pmDeck.unloadB());

  // --- mobile-failure handling: create returns a boolean, Deck A stays alive ---
  await page.setViewportSize({ width: 390, height: 720 });
  await sleep(300);
  results.mobileDeckGraceful = await page.evaluate(async () => {
    const D = window.__pmDeck;
    const created = D.createB();
    await new Promise((r) => setTimeout(r, 200));
    const alive = window.__pmDiag().frame > 0 && (window.__pmDiag().lastError === '' || window.__pmDiag().lastError === undefined);
    D.unloadB();
    return typeof created === 'boolean' && alive; // graceful either way; renderer never crashes
  });

  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.zeroUploads = uploads.length === 0;
  results.errorSample = errs.slice(0, 5);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/deck-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try { if (browser) await browser.close(); } catch {}
  process.exitCode = 1;
});
