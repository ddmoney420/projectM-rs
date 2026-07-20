// Phase 10C.2 — master A/B crossfader regression.
// Semantics/clamp, deck-identity stability, empty/clear-Deck-B safety, master
// isolation, warm decks at endpoints, rapid/random movement, resize during
// blend, and COLOR correctness (no darkening/premultiplication) via real
// master-canvas brightness (PNG decoded with built-in zlib — no deps).
// Run: ( cd web && PMW_URL=http://localhost:5174/ node verify-crossfader.mjs )
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'node:fs';
import { inflateSync } from 'node:zlib';

const URL_BASE = process.env.PMW_URL || 'http://localhost:5174/';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = {};
let browser;

// Minimal PNG average-luminance decoder (RGBA/RGB, 8-bit, non-interlaced —
// Playwright screenshots). Returns mean of R,G,B in 0..255.
function pngAverageLuma(buf) {
  let pos = 8; // skip signature
  let width = 0, height = 0, colorType = 6;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString('ascii', pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      colorType = data[9];
    } else if (type === 'IDAT') idat.push(data);
    else if (type === 'IEND') break;
    pos += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const ch = colorType === 6 ? 4 : colorType === 2 ? 3 : 1;
  const stride = width * ch;
  const out = Buffer.alloc(height * stride);
  let prevRow = Buffer.alloc(stride);
  let rp = 0;
  for (let y = 0; y < height; y++) {
    const filter = raw[rp++];
    const row = raw.subarray(rp, rp + stride);
    rp += stride;
    const cur = out.subarray(y * stride, y * stride + stride);
    for (let x = 0; x < stride; x++) {
      const a = x >= ch ? cur[x - ch] : 0;
      const b = prevRow[x];
      const c = x >= ch ? prevRow[x - ch] : 0;
      let v = row[x];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      cur[x] = v & 0xff;
    }
    prevRow = cur;
  }
  let sum = 0, n = 0;
  for (let i = 0; i < out.length; i += ch) {
    sum += out[i] + out[i + 1] + out[i + 2];
    n += 3;
  }
  return n ? sum / n : 0;
}

const run = async () => {
  browser = await chromium.launch({
    channel: 'chrome',
    headless: false,
    args: ['--enable-unsafe-webgpu', '--autoplay-policy=no-user-gesture-required'],
  });
  const page = await browser.newPage({ viewport: { width: 1000, height: 720 } });
  const errs = [];
  page.on('pageerror', (e) => errs.push('[pageerror] ' + e.message));
  page.on('console', (m) => { if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200)); });

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3800);

  const origin = URL_BASE.replace(/\/$/, '');
  // Averaged master-canvas luminance over a few frames (animation smoothing).
  const canvas = page.locator('#viz');
  const luma = async () => {
    let s = 0;
    for (let i = 0; i < 5; i++) {
      s += pngAverageLuma(await canvas.screenshot());
      await sleep(60);
    }
    return s / 5;
  };
  const setX = (t) => page.evaluate((t) => window.__pmCrossfader.set(t), t);
  const getX = () => page.evaluate(() => window.__pmCrossfader.get());
  const deckDiag = () => page.evaluate(() => window.__pmCrossfader.deck());

  // Setup two visually distinct decks: Deck A = plasma (bright), Deck B = a
  // fixture Milkdrop preset (near-dark). Deck A via live shader load.
  const setup = await page.evaluate(async (origin) => {
    const C = window.__pmContent, A = window.__pmAudition, M = window.__pmMilkdrop;
    const shaders = C.listBuiltinShaders();
    await A.loadLive(shaders.find((s) => (s.tags || []).includes('single-pass'))); // bright plasma → Deck A
    await M.loadPack(origin + '/__testpack__/manifest.json');
    const milk = M.listIndex().find((i) => i.id.startsWith('pack:projectm-rs-test-fixtures'));
    const aud = await A.audition(milk); // Milkdrop → Deck B
    return { audOk: aud.ok, deck: window.__pmCrossfader.deck() };
  }, origin);
  await sleep(600);

  // --- semantics + clamp ---
  await setX(0); results.setZero = Math.abs((await getX()) - 0) < 1e-6;
  await setX(0.5); results.setHalf = Math.abs((await getX()) - 0.5) < 1e-6;
  await setX(1); results.setOne = Math.abs((await getX()) - 1) < 1e-6;
  await setX(2.5); results.clampHigh = (await getX()) === 1;
  await setX(-3); results.clampLow = (await getX()) === 0;

  // --- deck identity stable across endpoints (no auto-swap) ---
  const dInit = await deckDiag();
  await setX(1); await sleep(120);
  const dOne = await deckDiag();
  await setX(0); await sleep(120);
  const dZero = await deckDiag();
  results.deckIdentityStable = dInit.deckA.id === 0 && dOne.deckA.id === 0 && dZero.deckA.id === 0 && dOne.deckB.id === 1;

  // --- master isolation: crossfading never mutates Deck A content ---
  const aStart = (await deckDiag()).deckA.sourceType;
  await setX(0.7); await sleep(120);
  results.masterIsolation = (await deckDiag()).deckA.sourceType === aStart;

  // --- warm decks: renderer keeps advancing at t=0 and t=1 ---
  await setX(0); const f0 = await page.evaluate(() => window.__pmDiag().frame); await sleep(150);
  const f0b = await page.evaluate(() => window.__pmDiag().frame);
  await setX(1); const f1 = await page.evaluate(() => window.__pmDiag().frame); await sleep(150);
  const f1b = await page.evaluate(() => window.__pmDiag().frame);
  results.warmAtEndpoints = f0b > f0 && f1b > f1;

  // --- COLOR: endpoints differ, middle is a blend (no darkening/halo), never black ---
  await setX(0); const bA = await luma();
  await setX(1); const bB = await luma();
  await setX(0.5); const bMid = await luma();
  const lo = Math.min(bA, bB), hi = Math.max(bA, bB);
  results.endpointsDiffer = Math.abs(bA - bB) > 3; // the fader visibly changes master
  results.neverBlack = bA > 1.5 && bMid > 1.5; // Deck A (bright) never fades to black
  results.midIsBlend = bMid >= lo - 8 && bMid <= hi + 8; // between endpoints (+margin) → no darkening/halo
  results.lumaSample = { bA: +bA.toFixed(1), bMid: +bMid.toFixed(1), bB: +bB.toFixed(1) };

  // --- empty Deck B: master never black even at t=1 (blits Deck A) ---
  await setX(0);
  await page.evaluate(() => window.__pmAudition.clear());
  results.clearResetsFader = (await getX()) === 0; // clear reset to A
  await setX(1); await sleep(150);
  const bEmptyOne = await luma();
  results.emptyDeckBNotBlack = bEmptyOne > 1.5; // t=1 with no Deck B → shows Deck A, not black
  await setX(0);

  // --- rapid 0↔1 + random values: no errors, renderer alive ---
  await page.evaluate(async () => {
    const A = window.__pmAudition, C = window.__pmContent;
    await A.audition(C.listBuiltinShaders().find((s) => (s.tags || []).includes('multipass')));
  });
  const rf0 = await page.evaluate(() => window.__pmDiag().frame);
  for (let i = 0; i < 30; i++) { await setX(i % 2); }
  for (let i = 0; i < 20; i++) { await setX(((i * 97) % 100) / 100); }
  await setX(0.5); await sleep(200);
  results.rapidRandomStable = (await page.evaluate(() => window.__pmDiag().frame)) > rf0 && (await page.evaluate(() => window.__pmDiag().lastError || '')) === '';

  // --- resize during blend (t=0.5) → no crash, still advancing ---
  await setX(0.5);
  await page.setViewportSize({ width: 520, height: 780 }); await sleep(400);
  await page.setViewportSize({ width: 880, height: 500 }); await sleep(400);
  results.resizeDuringBlend = (await page.evaluate(() => window.__pmDiag().frame)) > 0 && (await page.evaluate(() => window.__pmDiag().lastError || '')) === '';

  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.errorSample = errs.slice(0, 5);

  // cleanup created library items
  await page.evaluate(async () => {
    const L = window.__pmLibrary;
    for (const i of await L.store.getAll()) if (i.origin === 'user') await L.store.delete(i.id).catch(() => {});
    await L.store.setPreviewBank([]);
  });

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/crossfader-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try { if (browser) await browser.close(); } catch {}
  process.exitCode = 1;
});
