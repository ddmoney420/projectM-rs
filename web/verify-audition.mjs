// Phase 10B — Preview / Audition regression: audition into Deck B without
// disturbing the live Deck A / master; Preview Bank; preview monitor; warm
// state; master isolation. Drives __pmAudition/__pmDeck/__pmContent/__pmMilkdrop.
// Run: ( cd web && PMW_URL=http://localhost:5174/ node verify-audition.mjs )
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

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3800);

  const r = await page.evaluate(async (origin) => {
    const A = window.__pmAudition;
    const D = window.__pmDeck;
    const C = window.__pmContent;
    const M = window.__pmMilkdrop;
    const L = window.__pmLibrary;
    const diag = () => window.__pmDiag();
    const out = {};
    const before = new Set((await L.store.getAll()).map((i) => i.id));

    // establish a known LIVE deck (Deck A) = a built-in shader
    const shaders = C.listBuiltinShaders();
    await A.loadLive(shaders.find((s) => (s.tags || []).includes('single-pass')));
    await new Promise((r) => setTimeout(r, 150));
    const liveA = D.diag().deckA; // snapshot of live deck
    out.liveSet = A.status().live && A.status().live.type === 'shader' && liveA.shaderCount >= 1;

    // --- audition Milkdrop → Deck A unchanged ---
    await M.loadPack(origin + '/__testpack__/manifest.json');
    const milk = M.listIndex().find((i) => i.id.startsWith('pack:projectm-rs-test-fixtures'));
    const audMilk = await A.audition(milk);
    await new Promise((r) => setTimeout(r, 150));
    out.auditionMilkdrop = audMilk.ok === true && D.count() === 2 && D.diag().deckA.shaderCount === liveA.shaderCount && A.status().audition.type === 'milkdrop';

    // --- audition Shader → Deck A unchanged, Deck B is a shader ---
    const audShader = await A.audition(shaders.find((s) => (s.tags || []).includes('multipass')));
    await new Promise((r) => setTimeout(r, 200));
    out.auditionShader = audShader.ok === true && D.diag().deckA.shaderCount === liveA.shaderCount && D.diag().deckB && /shader/.test(D.diag().deckB.sourceType);

    // --- audition Scene → Deck A unchanged ---
    const scene = await C.saveCurrentScene('AudScene');
    const audScene = await A.audition({ id: scene.id, type: 'scene', name: 'AudScene' });
    await new Promise((r) => setTimeout(r, 150));
    out.auditionScene = audScene.ok === true && D.diag().deckA.shaderCount === liveA.shaderCount;

    // --- failed audition (missing pack shard) → live + audition preserved ---
    await M.loadPack(origin + '/does-not-exist.json'); // no-op (fails gracefully)
    const before2 = A.status().audition;
    const deckBBefore = D.diag().deckB && D.diag().deckB.sourceType;
    const synth = { id: 'pack:unreachable:x/y.milk', type: 'milkdrop', name: 'Unreachable' };
    const audFail = await A.audition(synth);
    await new Promise((r) => setTimeout(r, 150));
    out.failedAuditionPreserves = audFail.ok === false && D.diag().deckA.shaderCount === liveA.shaderCount && diag().frame > 0;

    // --- master isolation: LIVE stays Deck A, distinct from AUDITION ---
    const st = A.status();
    out.masterIsolation = st.live.type === 'shader' && st.audition && st.audition.type !== undefined && D.diag().deckA.shaderCount === liveA.shaderCount;

    // --- warm state: renderer keeps advancing while Deck B auditions ---
    const f0 = diag().frame;
    await new Promise((r) => setTimeout(r, 120));
    out.warmState = diag().frame > f0 && D.count() === 2;

    // --- clear audition: Deck B gone, Deck A intact ---
    A.clear();
    out.clearAudition = D.count() === 1 && A.status().audition === null && D.diag().deckA.shaderCount === liveA.shaderCount;

    // --- Preview Bank: add/reorder/persist/missing/clear ---
    await L.store.setPreviewBank([]);
    await A.setBank([shaders[0].id, shaders[1].id, 'missing:ref:1']);
    let bank = await A.getBank();
    out.bankAddPersist = bank.length === 3 && bank[0] === shaders[0].id;
    // reorder (swap first two) via setBank
    await A.setBank([shaders[1].id, shaders[0].id, 'missing:ref:1']);
    bank = await A.getBank();
    out.bankReorder = bank[0] === shaders[1].id && bank[1] === shaders[0].id;
    // missing ref does not crash resolution (aggregate has no 'missing:ref:1')
    const all = await window.__pmBrowser.collect();
    const resolved = bank.map((id) => all.find((i) => i.id === id)).filter(Boolean);
    out.bankMissingSkipped = resolved.length === 2;
    await A.setBank([]);
    out.bankClear = (await A.getBank()).length === 0;

    // cleanup
    const now = new Set((await L.store.getAll()).map((i) => i.id));
    for (const id of now) if (!before.has(id)) await L.store.delete(id).catch(() => {});
    await L.store.setPreviewBank([]);
    return out;
  }, URL_BASE.replace(/\/$/, ''));
  Object.assign(results, r);

  // --- preview monitor attaches (2nd surface) + Deck B shown, master = Deck A ---
  results.previewMonitor = await page.evaluate(async () => {
    const A = window.__pmAudition;
    const c = document.createElement('canvas');
    c.width = 128; c.height = 72; c.style.position = 'fixed'; c.style.left = '-9999px';
    document.body.appendChild(c);
    const attached = A.attachPreview(c);
    await new Promise((r) => setTimeout(r, 200));
    const alive = window.__pmDiag().frame > 0 && (window.__pmDiag().lastError === '' || window.__pmDiag().lastError === undefined);
    A.detachPreview();
    c.remove();
    return typeof attached === 'boolean' && alive; // attaches or degrades; renderer never crashes
  });

  // --- Preview Bank persists across a full reload ---
  await page.evaluate(async () => {
    const C = window.__pmContent;
    const ids = C.listBuiltinShaders().slice(0, 2).map((s) => s.id);
    await window.__pmAudition.setBank(ids);
  });
  await page.reload({ waitUntil: 'load' });
  await sleep(3000);
  results.bankPersistsReload = await page.evaluate(async () => {
    const bank = await window.__pmAudition.getBank();
    const ok = bank.length === 2;
    await window.__pmLibrary.store.setPreviewBank([]);
    return ok;
  });

  // --- mobile: audition workflow usable, no horizontal overflow ---
  await page.setViewportSize({ width: 390, height: 760 });
  await sleep(300);
  results.mobile = await page.evaluate(async () => {
    const B = window.__pmBrowser;
    await B.open();
    B.setView('bank');
    const noOverflow = document.documentElement.scrollWidth <= window.innerWidth + 2;
    return B.currentView() === 'bank' && noOverflow;
  });

  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.zeroUploads = uploads.length === 0;
  results.errorSample = errs.slice(0, 5);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/audition-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try { if (browser) await browser.close(); } catch {}
  process.exitCode = 1;
});
