// Phase 10A.4 — unified library browser regression (views, search, filters,
// virtualization at 10k, favorites/recent/collections, load routing, import,
// empty states, mobile, privacy). Drives __pmBrowser/__pmContent/__pmMilkdrop.
// Run: ( cd web && PMW_URL=http://localhost:5174/ node verify-browser.mjs )
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
  const page = await browser.newPage({ viewport: { width: 1000, height: 760 } });
  const errs = [];
  const uploads = [];
  page.on('pageerror', (e) => errs.push('[pageerror] ' + e.message));
  page.on('console', (m) => { if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200)); });
  page.on('request', (req) => {
    const u = new URL(req.url());
    if (req.method() !== 'GET' && req.method() !== 'HEAD') uploads.push(`${req.method()} ${req.url()}`);
    else if (!URL_BASE.startsWith(u.origin) && u.protocol !== 'data:' && u.protocol !== 'blob:') uploads.push(`EXT ${req.url()}`);
  });

  // Serve a synthetic 10,000-item Milkdrop manifest (same-origin, main-thread
  // fetch → route-intercepted). Shards are never fetched during browse.
  const SYNTH = 10000;
  await page.route('**/synthpack/manifest.json', (route) => {
    const items = [];
    for (let i = 0; i < SYNTH; i++) items.push({ path: `synth/p${i}.milk`, name: `Synthetic Preset ${i}`, shard: 'synth.pack', author: i % 2 ? 'AlphaAuthor' : 'BetaAuthor', category: i % 3 ? 'Geometric' : 'Organic' });
    route.fulfill({ contentType: 'application/json', body: JSON.stringify({ packId: 'synth', name: 'Synthetic', version: '1', license: 'unknown', items }) });
  });

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3800);
  results.rendererRunning = (await page.evaluate(() => (window.__pmDiag().frame ?? 0) > 0)) === true;

  const r = await page.evaluate(async (origin) => {
    const B = window.__pmBrowser;
    const M = window.__pmMilkdrop;
    const C = window.__pmContent;
    const L = window.__pmLibrary;
    const out = {};
    const before = new Set((await L.store.getAll()).map((i) => i.id));

    await B.open();

    // --- views ---
    B.setView('all'); const allN = B.resultCount();
    B.setView('shader'); const shaderN = B.resultCount();
    B.setView('scene'); const sceneN = B.resultCount();
    B.setView('favorites'); const favN0 = B.resultCount();
    B.setView('recent'); const recN0 = B.resultCount();
    B.setView('collections');
    out.views = allN >= 13 && shaderN >= 13 && B.currentView() === 'collections';
    out.emptyFavorites = favN0 === 0 && B.renderedRowCount() === 0; // no favorites yet ⇒ empty

    // --- search (name / author / tag / zero-result) ---
    B.setView('shader');
    B.setSearch('plasma'); const byName = B.resultCount();
    B.setSearch('projectM-rs'); const byAuthor = B.resultCount();
    B.setSearch('multipass'); const byTag = B.resultCount();
    B.setSearch('zzz-nothing-matches'); const zero = B.resultCount();
    out.search = byName >= 1 && byAuthor >= 1 && byTag >= 1 && zero === 0 && B.renderedRowCount() === 0;
    B.setSearch('');

    // --- real pack load + working Milkdrop load ---
    await M.loadPack(origin + '/__testpack__/manifest.json');
    await B.refresh();
    B.setView('milkdrop');
    const milkReal = (await B.collect()).filter((i) => i.type === 'milkdrop' && i.id.startsWith('pack:projectm-rs-test-fixtures'));
    const loadReal = await B.loadItem(milkReal[0]);
    await new Promise((r) => setTimeout(r, 250));
    out.loadMilkdropReal = loadReal.ok === true;

    // --- virtualization: 10,000 synthetic entries, bounded DOM ---
    await M.loadPack(origin + '/synthpack/manifest.json');
    await B.refresh();
    B.setView('milkdrop');
    const milkCount = B.resultCount();
    const domA = B.renderedRowCount();
    B.scrollTo(150000);
    const domB = B.renderedRowCount();
    B.scrollTo(B.scrollHeight());
    const domC = B.renderedRowCount();
    out.virtualization = milkCount >= 10000 && domA > 0 && domA < 60 && domB < 60 && domC < 60;

    // --- search stays responsive over the large set ---
    B.setSearch('Preset 4242');
    out.largeSearch = B.resultCount() >= 1 && B.resultCount() < 50 && B.renderedRowCount() < 60;
    B.setSearch('');

    // --- pack unavailable (synthetic shard missing) → graceful, visual alive ---
    const synthItem = (await B.collect()).find((i) => i.id.startsWith('pack:synth:'));
    const f0 = window.__pmDiag().frame;
    const badLoad = await B.loadItem(synthItem);
    await new Promise((r) => setTimeout(r, 200));
    out.packUnavailableGraceful = badLoad.ok === false && window.__pmDiag().frame > f0;

    // --- favorites: favorite a built-in shader → Favorites view shows it ---
    const shaders = C.listBuiltinShaders();
    await C.setFavorite(shaders[0].id, true);
    await B.refresh();
    B.setView('favorites');
    out.favorites = B.resultCount() >= 1 && (await B.collect()).some((i) => i.id === shaders[0].id && i.favorite);

    // --- recent: loading a shader puts it in Recent ---
    await B.loadItem(shaders[1]);
    await B.refresh();
    B.setView('recent');
    out.recent = (await B.collect()).some((i) => i.id === shaders[1].id && typeof i.lastUsed === 'number') && B.resultCount() >= 1;

    // --- collections: create/add/view/remove/delete (items survive) ---
    const col = await L.store.createCollection('Set A');
    await C.addToCollection(shaders[0].id, col.id);
    await B.refresh();
    B.setView('collections');
    const inColCount = (await L.store.listByCollection(col.id)).length;
    await C.removeFromCollection(shaders[0].id, col.id);
    const afterRemove = (await L.store.listByCollection(col.id)).length;
    await L.store.deleteCollection(col.id);
    const itemSurvives = !!(await L.store.get(shaders[0].id)); // built-in upserted, still present after collection delete
    out.collections = inColCount === 1 && afterRemove === 0 && itemSurvives;

    // --- shader + scene load routing ---
    const loadShader = await B.loadItem(shaders[2]);
    const scene = await C.saveCurrentScene('BrowserScene');
    const loadScene = await B.loadItem({ id: scene.id, type: 'scene' });
    out.loadShaderScene = loadShader.ok === true && loadScene.ok === true;

    // cleanup test-created store items (leave pre-existing intact)
    const now = new Set((await L.store.getAll()).map((i) => i.id));
    for (const id of now) if (!before.has(id)) await L.store.delete(id).catch(() => {});
    // clear the synthetic/real pack index from the milkdrop service state via reload later
    return out;
  }, URL_BASE.replace(/\/$/, ''));
  Object.assign(results, r);

  // --- mobile viewport: usable, no horizontal overflow ---
  await page.setViewportSize({ width: 390, height: 720 });
  await sleep(300);
  results.mobile = await page.evaluate(async () => {
    const B = window.__pmBrowser;
    await B.open();
    B.setView('shader');
    B.setSearch('a');
    const noOverflow = document.documentElement.scrollWidth <= window.innerWidth + 2;
    return B.resultCount() >= 0 && B.renderedRowCount() >= 0 && noOverflow;
  });

  // --- favorite persistence across reload ---
  const favId = await page.evaluate(async () => {
    const C = window.__pmContent;
    const s = C.listBuiltinShaders()[0];
    await C.setFavorite(s.id, true);
    return s.id;
  });
  await page.reload({ waitUntil: 'load' });
  await sleep(3000);
  results.favoritePersists = await page.evaluate(async (favId) => {
    const B = window.__pmBrowser;
    const L = window.__pmLibrary;
    await B.open();
    B.setView('favorites');
    const ok = (await B.collect()).some((i) => i.id === favId && i.favorite) && B.resultCount() >= 1;
    await L.store.delete(favId).catch(() => {});
    return ok;
  }, favId);

  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.zeroUploads = uploads.length === 0;
  results.uploadSample = uploads.slice(0, 5);
  results.errorSample = errs.slice(0, 5);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/browser-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try { if (browser) await browser.close(); } catch {}
  process.exitCode = 1;
});
