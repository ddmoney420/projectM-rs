// Phase 10A.2 — Milkdrop pack loader + shard worker + import browser regression.
// Drives window.__pmMilkdrop / __pmLibrary against project-owned CC0 fixtures
// served from /__testpack__/. Run:
//   ( cd web && PMW_URL=http://localhost:5174/ node verify-pack.mjs )
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
  const uploads = []; // non-GET or cross-origin requests carrying content
  page.on('pageerror', (e) => errs.push('[pageerror] ' + e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200));
  });
  page.on('request', (req) => {
    const method = req.method();
    const u = new URL(req.url());
    const sameOrigin = URL_BASE.startsWith(u.origin);
    if (method !== 'GET' && method !== 'HEAD') uploads.push(`${method} ${req.url()}`);
    else if (!sameOrigin && u.protocol !== 'data:' && u.protocol !== 'blob:') uploads.push(`EXT ${req.url()}`);
  });

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3500);
  results.rendererRunning = (await page.evaluate(() => (window.__pmDiag().frame ?? 0) > 0)) === true;

  // Main suite: fresh isolated store + service, real fixture URLs.
  const r = await page.evaluate(async () => {
    const L = window.__pmLibrary;
    const M = window.__pmMilkdrop;
    const origin = location.origin;
    const out = {};

    // isolated store + service
    await L.deleteLibraryDB('pm-web-packtest');
    const store = new L.LibraryStore('pm-web-packtest', L.DB_VERSION);
    await store.init();
    const svc = new M.MilkdropLibrary(store);

    // --- manifest validation ---
    out.invalidManifestRejected = M.validateManifest({}).ok === false && M.validateManifest({ packId: 'x', name: 'x', version: '1', license: 'CC0-1.0', items: 'nope' }).ok === false;

    // --- valid manifest load (index only, no shards downloaded) ---
    const load = await svc.loadPack(origin + '/__testpack__/manifest.json');
    out.manifestLoads = load.ok === true && load.count === 3 && svc.indexCount() === 3;
    out.licenseClassExplicit = load.licenseClass === 'explicitly-licensed'; // CC0-1.0

    // --- per-item license override ---
    const idx = svc.listIndex();
    const sine = idx.find((i) => i.name === 'Starter Sine');
    const pulse = idx.find((i) => i.name === 'Starter Pulse');
    out.perItemLicenseOverride = sine.license === 'CC0-1.0' && pulse.license === 'LGPL-2.1';
    out.packAuthorPreserved = sine.author === 'projectM-rs';

    // --- lazy shard fetch + gzip decompress + NDJSON parse + lookup (worker) ---
    const sineText = await svc.presetText(sine.id);
    out.workerShardLoad = typeof sineText === 'string' && sineText.includes('nWaveMode=0') && sineText.includes('per_frame_1');

    // --- recent/usage recorded on load (metadata-only, item now persisted) ---
    const sineMeta = await store.get(sine.id);
    out.usageRecorded = !!sineMeta && sineMeta.usageCount >= 1 && typeof sineMeta.lastUsed === 'number';

    // --- favorites (upsert pack item + persist) ---
    await svc.setFavorite(pulse.id, true);
    const favs = await store.listFavorites();
    out.favoritePackItem = favs.some((f) => f.id === pulse.id && f.favorite === true);

    // --- missing preset in shard → null (graceful) ---
    const sc = new M.ShardClient();
    out.missingPresetNull = (await sc.getPresetText(origin + '/__testpack__/basic.ndjson.pack', 'nope/nope.milk')) === null;

    // --- main-thread decompression fallback (worker-failure recovery proof) ---
    const scMain = new M.ShardClient({ forceMainThread: true });
    const mainText = await scMain.getPresetText(origin + '/__testpack__/basic.ndjson.pack', 'projectm-rs-starter/Starter Grid.milk');
    out.mainThreadFallback = typeof mainText === 'string' && mainText.includes('nWaveMode=3');

    // --- shard unavailable → service degrades to null, no throw ---
    const svcBad = new M.MilkdropLibrary(store);
    await svcBad.loadPack(origin + '/__testpack__/manifest-badshard.json');
    let badThrew = false;
    let badVal;
    try {
      badVal = await svcBad.presetText(svcBad.listIndex()[0].id);
    } catch {
      badThrew = true;
    }
    out.badShardGraceful = !badThrew && badVal === null && (await window.__pmDiag()).frame > 0;

    // --- manifest fetch failure → ok:false, no throw ---
    const missing = await svc.loadPack(origin + '/__testpack__/does-not-exist.json');
    out.manifestFetchFailGraceful = missing.ok === false && typeof missing.error === 'string';

    // --- navigation over the 3-item index (deterministic) ---
    const first = svc.nextId();
    const second = svc.nextId(first);
    const wrap = svc.prevId(first);
    out.navNextPrev = first === idx[0].id && second === idx[1].id && wrap === idx[2].id; // prev of first wraps to last
    out.navRandomInSet = idx.map((i) => i.id).includes(svc.randomId());

    // --- import single + multiple .milk (stays local, filename author heuristic) ---
    const one = await svc.importTexts([{ name: 'Aphex Twin - Windowlicker.milk', text: '[preset00]\nnWaveMode=1\n' }]);
    out.importSingle = one.length === 1 && one[0].name === 'Windowlicker' && one[0].author === 'Aphex Twin' && one[0].origin === 'imported';
    const importedText = await svc.presetText(one[0].id);
    out.importInlineLoads = typeof importedText === 'string' && importedText.includes('nWaveMode=1');
    const many = await svc.importTexts([
      { name: 'nosep.milk', text: '[preset00]\n' },
      { name: 'Rovastar - Fracture Blur.milk', text: '[preset00]\n' },
    ]);
    // no-author when the filename has no clear "Author - Title" convention;
    // author parsed only when the convention clearly matches (never fabricated).
    out.importMultiple = many.length === 2 && many[0].author === undefined && many[1].author === 'Rovastar';

    // --- zero-pack: fresh service works with no pack; navigation is null ---
    await L.deleteLibraryDB('pm-web-zeropack');
    const zstore = new L.LibraryStore('pm-web-zeropack', L.DB_VERSION);
    await zstore.init();
    const zsvc = new M.MilkdropLibrary(zstore);
    out.zeroPackNav = zsvc.indexCount() === 0 && zsvc.nextId() === null && zsvc.prevId() === null && zsvc.randomId() === null;
    const zi = await zsvc.importTexts([{ name: 'Local.milk', text: '[preset00]\n' }]);
    out.zeroPackImportWorks = zi.length === 1 && (await zstore.getAll()).length === 1;
    zstore.close();
    await L.deleteLibraryDB('pm-web-zeropack');

    store.close();
    svc.dispose();
    await L.deleteLibraryDB('pm-web-packtest');
    return out;
  });
  Object.assign(results, r);

  // --- import persists across a full reload ---
  await page.evaluate(async () => {
    const L = window.__pmLibrary;
    const M = window.__pmMilkdrop;
    await L.deleteLibraryDB('pm-web-import-reload');
    const store = new L.LibraryStore('pm-web-import-reload', L.DB_VERSION);
    await store.init();
    const svc = new M.MilkdropLibrary(store);
    const items = await svc.importTexts([{ name: 'Persisted - Preset.milk', text: '[preset00]\nnWaveMode=7\n' }]);
    window.__packReloadId = items[0].id;
    store.close();
  });
  const reloadId = await page.evaluate(() => window.__packReloadId);
  await page.reload({ waitUntil: 'load' });
  await sleep(3000);
  results.importPersistsReload = await page.evaluate(async (id) => {
    const L = window.__pmLibrary;
    const store = new L.LibraryStore('pm-web-import-reload', L.DB_VERSION);
    await store.init();
    const got = await store.getFull(id);
    store.close();
    await L.deleteLibraryDB('pm-web-import-reload');
    return !!got && got.origin === 'imported' && got.payload.kind === 'inline' && got.payload.text.includes('nWaveMode=7');
  }, reloadId);

  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.zeroUploads = uploads.length === 0;
  results.uploadSample = uploads.slice(0, 5);
  results.errorSample = errs.slice(0, 5);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/pack-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try {
    if (browser) await browser.close();
  } catch {}
  process.exitCode = 1;
});
