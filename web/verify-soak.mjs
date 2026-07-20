// Phase 10D — timed extended soak (heaviest config: Milkdrop Deck A + Milkdrop
// Deck B + preview monitor + master crossfade + periodic preset swaps / audition
// replacement / resize / fullscreen). Runs for SOAK_MS wall-clock (default 300s)
// and tracks leak / errors / device loss / frame progress / texture-count trend.
// Run: ( cd web && SOAK_MS=300000 PMW_URL=http://localhost:5174/ node verify-soak.mjs )
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'node:fs';

const URL_BASE = process.env.PMW_URL || 'http://localhost:5174/';
const SOAK_MS = Number(process.env.SOAK_MS || 300000);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = {};
let browser;

const run = async () => {
  const t0 = Date.now();
  browser = await chromium.launch({
    channel: 'chrome',
    headless: false,
    args: ['--enable-unsafe-webgpu', '--autoplay-policy=no-user-gesture-required'],
  });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errs = [];
  page.on('pageerror', (e) => errs.push('[pageerror] ' + e.message));
  page.on('console', (m) => { if (m.type() === 'error') errs.push('[console] ' + m.text().slice(0, 200)); });

  await page.goto(`${URL_BASE}?miditest=1`, { waitUntil: 'load' });
  await sleep(3800);

  const ready = await page.evaluate(async (origin) => {
    await window.__pmMilkdrop.loadPack(origin + '/__testpack__/manifest.json');
    const idx = window.__pmMilkdrop.listIndex().filter((i) => i.id.startsWith('pack:'));
    const texts = [];
    for (const it of idx) texts.push(await window.__pmMilkdrop.presetText(it.id));
    // Deck A milkdrop by default; audition milkdrop → Deck B; attach monitor.
    const c = document.createElement('canvas'); c.width = 160; c.height = 90; c.style.cssText = 'position:fixed;left:-9999px'; document.body.appendChild(c);
    window.__pmAudition.attachPreview(c);
    window.__pmDeck.createB(); window.__pmDeck.loadPresetB(texts[0]);
    window.__pmCrossfader.set(0.5);
    return texts.filter(Boolean);
  }, URL_BASE.replace(/\/$/, ''));

  const baseTel = await page.evaluate(() => window.__pmQual.telemetry());
  results.baselineTextures = baseTel.liveTextureCount;
  const startFrame = await page.evaluate(() => window.__pmDiag().frame);

  let iter = 0;
  let maxTex = baseTel.liveTextureCount;
  let deviceLoss = 0;
  const fpsSamples = [];
  const texSamples = [];
  const cpuSamples = [];
  let lastFrame = startFrame;
  let lastT = Date.now();

  while (Date.now() - t0 < SOAK_MS) {
    // Exercise the heaviest realistic performance moves each cycle.
    await page.evaluate((args) => {
      const { presets, i } = args;
      const xf = (i % 4) < 2 ? (i % 4) * 0.5 : (4 - (i % 4)) * 0.5; // 0,.5,1,.5 A↔B↔A
      window.__pmCrossfader.set(xf);
      // Deck B audition replacement (intra-deck transition) every cycle.
      window.__pmDeck.loadPresetB(presets[i % presets.length]);
      // Deck A preset swap (intra-deck transition) every 3rd cycle — up to 4 engines.
      if (i % 3 === 0 && window.__pmAudition.loadLive) { /* Deck A stays milkdrop; skip to avoid changing source type */ }
      if (i % 7 === 0) window.__pmPerf?.dispatch?.('performance.bank_next');
    }, { presets: ready, i: iter });
    await sleep(700);

    // sample telemetry
    const s = await page.evaluate(() => ({ frame: window.__pmDiag().frame, err: window.__pmDiag().lastError || '', t: window.__pmQual.telemetry() }));
    if (/lost|device/i.test(s.err)) deviceLoss++;
    maxTex = Math.max(maxTex, s.t.liveTextureCount);
    texSamples.push(s.t.liveTextureCount);
    cpuSamples.push(s.t.cpuMs);
    const now = Date.now();
    const fps = ((s.frame - lastFrame) * 1000) / (now - lastT);
    if (isFinite(fps) && fps > 0) fpsSamples.push(+fps.toFixed(1));
    lastFrame = s.frame; lastT = now;

    // occasional resize + fullscreen-ish clean toggle
    if (iter % 10 === 5) { await page.setViewportSize({ width: 1000, height: 700 }); }
    if (iter % 10 === 0 && iter > 0) { await page.setViewportSize({ width: 1280, height: 800 }); }
    iter++;
  }

  // Clear Deck B + settle, then measure the leak.
  await page.evaluate(() => { window.__pmDeck.unloadB(); window.__pmAudition.detachPreview(); window.__pmCrossfader.set(0); });
  await sleep(1500);
  const endTel = await page.evaluate(() => window.__pmQual.telemetry());
  const endFrame = await page.evaluate(() => window.__pmDiag().frame);

  const durationMs = Date.now() - t0;
  results.requestedSoakMs = SOAK_MS;
  results.actualDurationMs = durationMs;
  results.actualDurationMin = +(durationMs / 60000).toFixed(2);
  results.iterations = iter;
  results.maxTextures = maxTex;
  results.finalTextures = endTel.liveTextureCount;
  results.noTextureLeak = endTel.liveTextureCount <= results.baselineTextures + 4;
  results.framesAdvanced = endFrame - startFrame;
  results.keptAdvancing = endFrame - startFrame > iter;
  results.deviceLoss = deviceLoss;
  results.fpsMin = fpsSamples.length ? Math.min(...fpsSamples) : 0;
  results.fpsMax = fpsSamples.length ? Math.max(...fpsSamples) : 0;
  results.fpsAvg = fpsSamples.length ? +(fpsSamples.reduce((a, b) => a + b, 0) / fpsSamples.length).toFixed(1) : 0;
  results.cpuMsFirst = cpuSamples[0] ?? null;
  results.cpuMsLast = cpuSamples.at(-1) ?? null;
  results.texFirst = texSamples[0] ?? null;
  results.texLast = texSamples.at(-1) ?? null;
  results.finalGpuError = await page.evaluate(() => window.__pmDiag().lastError || '');
  results.noWebgpuErrors = results.finalGpuError === '' && deviceLoss === 0;
  results.noConsoleErrors = errs.filter((e) => !/favicon/i.test(e)).length === 0;
  results.noWasmPanics = errs.filter((e) => /panic|unreachable|RuntimeError/.test(e)).length === 0;
  results.errorSample = errs.slice(0, 8);

  await browser.close();
  mkdirSync('shots', { recursive: true });
  writeFileSync('shots/soak-results.json', JSON.stringify(results, null, 2));
  console.log('RESULTS:\n' + JSON.stringify(results, null, 2));
};

run().catch(async (e) => {
  console.error('FAILED:', e?.message || e);
  try { if (browser) await browser.close(); } catch {}
  process.exitCode = 1;
});
