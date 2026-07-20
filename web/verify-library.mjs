// Phase 10A.1 — browser IndexedDB + content-library regression.
// Drives window.__pmLibrary (exposed under ?miditest / dev) through CRUD,
// type round-trip, favorites, recent, collections, migration, corrupt-record
// isolation, zero-corpus, and persistence-across-reload. Run:
//   ( cd web && PMW_URL=http://localhost:5174/ node verify-library.mjs )
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
  page.on('pageerror', (e) => errs.push('[pageerror] ' + e.message));
  page.on('console', (m) => {
    if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200));
  });
  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3500);

  // The app's real library initializes non-blocking; wait for a terminal status.
  results.appLibraryStatus = await page.evaluate(async () => {
    const L = window.__pmLibrary;
    for (let i = 0; i < 50 && L.status() === 'uninitialized'; i++) await new Promise((r) => setTimeout(r, 100));
    return L.status();
  });
  results.appLibraryReady = results.appLibraryStatus === 'ready';
  results.rendererStillRunning = (await page.evaluate(() => (window.__pmDiag().frame ?? 0) > 0)) === true;

  // Main in-page suite against an isolated test database.
  const r = await page.evaluate(async () => {
    const L = window.__pmLibrary;
    const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);
    const out = {};
    const NAME = 'pm-web-library-test';
    await L.deleteLibraryDB(NAME);

    const store = new L.LibraryStore(NAME, L.DB_VERSION);
    out.initReady = (await store.init()) === 'ready';

    // --- payloads ---
    const packPayload = { kind: 'pack', packId: 'cotc', shard: 'Geometric.ndjson.gz', path: 'cotc/Geometric/Geiss - Cauldron.milk' };
    const inlinePayload = { kind: 'inline', text: '[preset00]\nnWaveMode=0\n' };
    const shaderPayload = { source: 'void mainImage(){}', mode: 0, controls: [[0, 0, 0, 0]], mods: [], attribution: { author: 'me', license: 'LGPL-2.1' }, passes: [] };
    const scenePayload = { schema_version: 1, scene_id: 's1', name: 'Scene 1', layers: [], speed: 1, paused: false, bpm: 120, tempo_manual: false, subdivision: 1, global_effects: [] };

    const milkRef = L.makeItem({ type: 'milkdrop', name: 'Geiss Cauldron', origin: 'pack', id: L.StableId.pack('cotc', packPayload.path), author: 'Geiss', license: '', attribution: { author: 'Geiss' } });
    const milkInline = L.makeItem({ type: 'milkdrop', name: 'My Import', origin: 'imported' });
    const shaderItem = L.makeItem({ type: 'shader', name: 'Plasma', origin: 'builtin', id: L.StableId.builtin('shader', 'Animated plasma'), license: 'LGPL-2.1' });
    const sceneItem = L.makeItem({ type: 'scene', name: 'My Scene', origin: 'user' });

    // --- CRUD + type round-trip ---
    await store.put(milkRef, packPayload);
    await store.put(milkInline, inlinePayload);
    await store.put(shaderItem, shaderPayload);
    await store.put(sceneItem, scenePayload);
    out.insertCount = (await store.count()) === 4;

    const gotMilk = await store.getFull(milkRef.id);
    const gotShader = await store.getFull(shaderItem.id);
    const gotScene = await store.getFull(sceneItem.id);
    out.roundtripMilkdropRef = !!gotMilk && eq(gotMilk.payload, packPayload) && gotMilk.type === 'milkdrop';
    out.roundtripShader = !!gotShader && eq(gotShader.payload, shaderPayload);
    out.roundtripScene = !!gotScene && eq(gotScene.payload, scenePayload) && gotScene.payload.schema_version === 1;
    out.stableIdIsNotIndex = gotMilk.id.startsWith('pack:cotc:') && !/^\d+$/.test(gotMilk.id);

    // update (metadata-only) + delete
    const upd = await store.update(sceneItem.id, { name: 'Renamed Scene' });
    out.updateName = upd?.name === 'Renamed Scene' && (await store.getFull(sceneItem.id))?.payload && eq((await store.getFull(sceneItem.id)).payload, scenePayload); // payload untouched
    await store.delete(milkInline.id);
    out.deleteWorks = (await store.get(milkInline.id)) === null && (await store.count()) === 3;

    // --- type queries ---
    out.listByType = (await store.listByType('shader')).length === 1 && (await store.listByType('milkdrop')).length === 1;

    // --- favorites (persist across reopen) ---
    await store.setFavorite(shaderItem.id, true);
    store.close();
    const store2 = new L.LibraryStore(NAME, L.DB_VERSION);
    await store2.init();
    const favs = await store2.listFavorites();
    out.favoritePersists = favs.length === 1 && favs[0].id === shaderItem.id && favs[0].favorite === true;

    // --- recent usage ---
    await store2.recordUsage(milkRef.id);
    await new Promise((r) => setTimeout(r, 5));
    await store2.recordUsage(shaderItem.id); // most recent
    const recent = await store2.listRecent(10);
    const shaderMeta = await store2.get(shaderItem.id);
    out.recentOrder = recent.length >= 2 && recent[0].id === shaderItem.id;
    out.usageCount = shaderMeta.usageCount === 1 && typeof shaderMeta.lastUsed === 'number';

    // --- collections (multi-membership) ---
    const col = await store2.createCollection('High Energy');
    await store2.addToCollection(shaderItem.id, col.id);
    await store2.addToCollection(milkRef.id, col.id);
    const inCol = await store2.listByCollection(col.id);
    out.collectionMembership = inCol.length === 2 && (await store2.listCollections()).some((c) => c.id === col.id);
    await store2.removeFromCollection(milkRef.id, col.id);
    out.collectionRemove = (await store2.listByCollection(col.id)).length === 1;

    // --- preview bank storage primitive (refs only) ---
    await store2.setPreviewBank([shaderItem.id, milkRef.id]);
    const bank = await store2.getPreviewBank();
    out.previewBankRefs = eq(bank.itemIds, [shaderItem.id, milkRef.id]);

    // --- corrupt record is isolated, not fatal ---
    const rawDb = await L.openLibraryDB(NAME, L.DB_VERSION);
    await new Promise((resolve, reject) => {
      const tx = rawDb.transaction(['items'], 'readwrite');
      tx.objectStore('items').put({ id: 'corrupt-1', junk: true }); // missing required fields
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    rawDb.close();
    let getAllThrew = false;
    let all = [];
    try {
      all = await store2.getAll();
    } catch {
      getAllThrew = true;
    }
    out.corruptIsolated = !getAllThrew && !all.some((i) => i.id === 'corrupt-1') && all.length === 3;
    store2.close();

    // --- zero-corpus: shaders/scenes work with no milkdrop pack ---
    const ZNAME = 'pm-web-library-zero';
    await L.deleteLibraryDB(ZNAME);
    const zstore = new L.LibraryStore(ZNAME, L.DB_VERSION);
    out.zeroInit = (await zstore.init()) === 'ready';
    await zstore.put(L.makeItem({ type: 'shader', name: 'S', origin: 'builtin', id: L.StableId.builtin('shader', 'S') }), shaderPayload);
    await zstore.put(L.makeItem({ type: 'scene', name: 'Sc', origin: 'user' }), scenePayload);
    out.zeroCorpusWorks = (await zstore.listByType('milkdrop')).length === 0 && (await zstore.getAll()).length === 2;
    zstore.close();
    await L.deleteLibraryDB(ZNAME);
    await L.deleteLibraryDB(NAME);
    return out;
  });
  Object.assign(results, r);

  // --- migration harness: v1 data survives a non-destructive v(N+1) upgrade ---
  results.migrationNonDestructive = await page.evaluate(async () => {
    const L = window.__pmLibrary;
    const MNAME = 'pm-web-library-migtest';
    await L.deleteLibraryDB(MNAME);
    const s1 = new L.LibraryStore(MNAME, L.DB_VERSION);
    await s1.init();
    await s1.put(L.makeItem({ type: 'scene', name: 'Keep me', origin: 'user', id: 'keepme' }), { schema_version: 1, name: 'Keep me' });
    s1.close();
    // Reopen at DB_VERSION+1 with an upgrade that reuses applyMigrations AND adds
    // a new store — the existing 'items' data must survive (no destructive reset).
    const survived = await new Promise((resolve, reject) => {
      const req = indexedDB.open(MNAME, L.DB_VERSION + 1);
      req.onupgradeneeded = (ev) => {
        const db = req.result;
        L.applyMigrations(db, ev.oldVersion, req.transaction); // no-op for existing versions
        if (ev.oldVersion < L.DB_VERSION + 1 && !db.objectStoreNames.contains('v2test')) db.createObjectStore('v2test', { keyPath: 'id' });
      };
      req.onsuccess = () => {
        const db = req.result;
        const tx = db.transaction(['items'], 'readonly');
        const g = tx.objectStore('items').get('keepme');
        g.onsuccess = () => {
          const ok = !!g.result && g.result.id === 'keepme' && db.objectStoreNames.contains('v2test') && db.version === L.DB_VERSION + 1;
          db.close();
          resolve(ok);
        };
        g.onerror = () => reject(g.error);
      };
      req.onerror = () => reject(req.error);
    });
    await L.deleteLibraryDB(MNAME);
    return survived === true;
  });

  // --- persistence across a full page reload ---
  await page.evaluate(async () => {
    const L = window.__pmLibrary;
    await L.deleteLibraryDB('pm-web-library-reload');
    const s = new L.LibraryStore('pm-web-library-reload', L.DB_VERSION);
    await s.init();
    await s.put(L.makeItem({ type: 'scene', name: 'Persist', origin: 'user', id: 'persist-1' }), { schema_version: 1, name: 'Persist' });
    s.close();
  });
  await page.reload({ waitUntil: 'load' });
  await sleep(3000);
  results.persistsAcrossReload = await page.evaluate(async () => {
    const L = window.__pmLibrary;
    const s = new L.LibraryStore('pm-web-library-reload', L.DB_VERSION);
    await s.init();
    const got = await s.getFull('persist-1');
    s.close();
    await L.deleteLibraryDB('pm-web-library-reload');
    return !!got && got.name === 'Persist' && got.payload.name === 'Persist';
  });

  results.noConsoleErrors = errs.filter((e) => !/favicon|Download the React/i.test(e)).length === 0;
  results.errorSample = errs.slice(0, 5);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/library-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try {
    if (browser) await browser.close();
  } catch {}
  process.exitCode = 1;
});
