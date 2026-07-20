// Phase 10A.3 — shader + scene content-library browser regression.
// Drives window.__pmContent (bound to the real engine + app library store).
// Run: ( cd web && PMW_URL=http://localhost:5174/ node verify-content.mjs )
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
  page.on('console', (m) => {
    if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200));
  });
  page.on('request', (req) => {
    const u = new URL(req.url());
    if (req.method() !== 'GET' && req.method() !== 'HEAD') uploads.push(`${req.method()} ${req.url()}`);
    else if (!URL_BASE.startsWith(u.origin) && u.protocol !== 'data:' && u.protocol !== 'blob:') uploads.push(`EXT ${req.url()}`);
  });

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3800);

  const r = await page.evaluate(async () => {
    const C = window.__pmContent;
    const L = window.__pmLibrary;
    const diag = () => window.__pmDiag();
    const out = {};
    const created = new Set(); // ids to clean up
    const before = new Set((await L.store.getAll()).map((i) => i.id));
    const track = (it) => {
      if (it && it.id) created.add(it.id);
      return it;
    };

    // --- built-in enumeration / stable ids / metadata ---
    const builtins = C.listBuiltinShaders();
    out.builtinEnumerated = builtins.length >= 10;
    out.builtinStableIds = builtins.every((b) => b.id.startsWith('builtin:shader:') && b.origin === 'builtin' && b.license === 'LGPL-2.1');
    out.builtinTags = builtins.some((b) => (b.tags || []).includes('single-pass')) && builtins.some((b) => (b.tags || []).includes('multipass'));
    const single = builtins.find((b) => (b.tags || []).includes('single-pass'));
    const multi = builtins.find((b) => (b.tags || []).includes('multipass'));

    // --- built-in single-pass round-trip (transactional load) ---
    const rs1 = await C.loadShader(single.id);
    await new Promise((r) => setTimeout(r, 200));
    out.builtinSinglePassLoads = rs1.ok === true && diag().shaderCount >= 1;

    // --- built-in multipass round-trip ---
    const rs2 = await C.loadShader(multi.id);
    await new Promise((r) => setTimeout(r, 300));
    out.builtinMultipassLoads = rs2.ok === true && diag().shaderCount >= 1 && diag().shaderPasses >= 2;

    // --- save current shader → user entry → load back ---
    const savedShader = track(await C.saveCurrentShader('Test Shader'));
    out.saveShader = !!savedShader && savedShader.origin === 'user' && savedShader.id.startsWith('user:shader:');
    const savedFull = await C.getFull(savedShader.id);
    out.shaderPayloadPreserved = !!savedFull && typeof savedFull.payload.source === 'string' && Array.isArray(savedFull.payload.passes);
    const reload = await C.loadShader(savedShader.id);
    out.userShaderLoads = reload.ok === true;

    // --- rename / duplicate / delete (built-ins protected) ---
    const renamed = await C.rename(savedShader.id, 'Renamed Shader');
    out.renameUser = renamed?.name === 'Renamed Shader';
    out.renameBuiltinBlocked = (await C.rename(single.id, 'nope')) === null;
    const dup = track(await C.duplicate(single.id));
    out.duplicateBuiltin = !!dup && dup.origin === 'user' && dup.type === 'shader';
    out.deleteBuiltinBlocked = (await C.delete(single.id)) === false && C.listBuiltinShaders().some((b) => b.id === single.id);
    const delId = dup.id;
    out.deleteUser = (await C.delete(delId)) === true && (await C.getFull(delId)) === null;

    // --- favorites (persist; built-in upserted) ---
    await C.setFavorite(single.id, true);
    created.add(single.id);
    out.favoriteBuiltin = (await C.listFavorites()).some((f) => f.id === single.id && f.favorite === true);

    // --- invalid shader load → rejected, active visual retained ---
    const badShaderId = 'user:shader:corrupt-test';
    await L.store.put({ id: badShaderId, type: 'shader', name: 'bad', origin: 'user', favorite: false, dateAdded: Date.now(), usageCount: 0, collections: [], schemaVersion: 1 }, { passes: 'not-an-array', source: 123 });
    created.add(badShaderId);
    const layersBefore = diag().layerCount;
    const frameBefore = diag().frame;
    const badRes = await C.loadShader(badShaderId);
    await new Promise((r) => setTimeout(r, 200));
    out.invalidShaderRejected = badRes.ok === false && diag().layerCount === layersBefore && diag().frame > frameBefore;

    // --- scene save / load ---
    const savedScene = track(await C.saveCurrentScene('Test Scene'));
    out.saveScene = !!savedScene && savedScene.type === 'scene' && savedScene.origin === 'user';
    const sceneFull = await C.getFull(savedScene.id);
    out.scenePayloadVerbatim = !!sceneFull && typeof sceneFull.payload.schema_version === 'number';
    const sceneLoad = await C.loadScene(savedScene.id);
    out.sceneLoads = sceneLoad.ok === true;

    // --- invalid scene load → rejected, current scene intact ---
    const badSceneId = 'user:scene:corrupt-test';
    await L.store.put({ id: badSceneId, type: 'scene', name: 'bad', origin: 'user', favorite: false, dateAdded: Date.now(), usageCount: 0, collections: [], schemaVersion: 1 }, { not: 'a scene', schema_version: 999 });
    created.add(badSceneId);
    const layers2 = diag().layerCount;
    const frame2 = diag().frame;
    const badScene = await C.loadScene(badSceneId);
    await new Promise((r) => setTimeout(r, 200));
    out.invalidSceneRejected = badScene.ok === false && diag().layerCount === layers2 && diag().frame > frame2;

    // --- collections (shader + scene membership) ---
    const col = await C.createCollection('Set 1');
    await C.addToCollection(savedShader.id, col.id);
    await C.addToCollection(savedScene.id, col.id);
    const inCol = await C.listByCollection(col.id);
    out.collections = inCol.length === 2 && inCol.some((i) => i.type === 'shader') && inCol.some((i) => i.type === 'scene');
    await C.removeFromCollection(savedShader.id, col.id);
    out.collectionRemove = (await C.listByCollection(col.id)).length === 1;

    // --- recent / usage (shader + scene) ---
    const shaderMeta = await L.store.get(savedShader.id);
    const sceneMeta = await L.store.get(savedScene.id);
    out.recentUsage = shaderMeta.usageCount >= 1 && sceneMeta.usageCount >= 1 && (await C.listRecent(20)).length >= 2;

    // --- zero-Milkdrop: shader+scene library fully functional (no pack loaded) ---
    out.zeroMilkdropLibrary = window.__pmMilkdrop.indexCount() === 0 && C.listBuiltinShaders().length >= 10 && (await C.listByType('shader')).length >= 1 && (await C.listByType('scene')).length >= 1;

    // cleanup test-created items (leave any pre-existing user data intact)
    for (const id of created) if (!before.has(id)) await L.store.delete(id).catch(() => {});
    return out;
  });
  Object.assign(results, r);

  // --- reload persistence: saved shader + scene survive a full reload ---
  const ids = await page.evaluate(async () => {
    const C = window.__pmContent;
    const sh = await C.saveCurrentShader('Persist Shader');
    const sc = await C.saveCurrentScene('Persist Scene');
    return { sh: sh?.id, sc: sc?.id };
  });
  await page.reload({ waitUntil: 'load' });
  await sleep(3200);
  results.persistsAcrossReload = await page.evaluate(async (ids) => {
    const C = window.__pmContent;
    const L = window.__pmLibrary;
    const sh = await C.getFull(ids.sh);
    const sc = await C.getFull(ids.sc);
    const ok = !!sh && sh.origin === 'user' && !!sc && sc.type === 'scene' && typeof sc.payload.schema_version === 'number';
    await L.store.delete(ids.sh).catch(() => {});
    await L.store.delete(ids.sc).catch(() => {});
    return ok;
  }, ids);

  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.zeroUploads = uploads.length === 0;
  results.uploadSample = uploads.slice(0, 5);
  results.errorSample = errs.slice(0, 5);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/content-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try {
    if (browser) await browser.close();
  } catch {}
  process.exitCode = 1;
});
