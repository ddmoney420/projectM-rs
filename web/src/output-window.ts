// The output (second-screen) window. It runs NO engine and NO WebGPU: it simply
// displays the MediaStreamTrack transferred from the control window (a live
// mirror of that window's rendered canvas). This makes every scene/shader/
// effect/MIDI/tempo/feedback/audio change appear here automatically, with
// perfect temporal fidelity and no per-frame serialization — and it means this
// window has no GPU device to lose, so it can never destabilize the controller.
//
// Lifecycle: on load (and periodically while disconnected) it sends `hello` to
// `window.opener`; the controller replies by transferring a fresh capture track.
// If the controller reloads, the old track ends, we detect it, and the retry
// hellos reconnect to the reloaded controller automatically.

import { makeMsg, parseMessage, newPeer } from './projection-protocol';

const video = document.getElementById('out') as HTMLVideoElement;
const ov = document.getElementById('ov')!;
const ovTitle = document.getElementById('ov-title')!;
const ovMsg = document.getElementById('ov-msg')!;
const fsBtn = document.getElementById('fsbtn') as HTMLButtonElement;
const diag = document.getElementById('diag')!;

// The control window shares its capture stream here (same-origin, by reference)
// because MediaStreamTrack postMessage-transfer isn't universally supported.
const STREAM_KEY = '__pmOutputStream';

const peer = newPeer();
let connected = false;
let stream: MediaStream | null = null;

function status(title: string, msg: string): void {
  ov.classList.remove('hidden');
  ovTitle.textContent = title;
  ovMsg.textContent = msg;
}
function refreshChrome(): void {
  // Center card only while not connected. Corner fullscreen button only while
  // connected and windowed. Fullscreen projection shows nothing.
  if (connected) ov.classList.add('hidden');
  fsBtn.classList.toggle('hidden', !connected || !!document.fullscreenElement);
}

function setStream(s: MediaStream): void {
  stream = s;
  video.srcObject = s;
  void video.play().catch(() => {});
  connected = true;
  refreshChrome();
}

function onDisconnect(): void {
  if (!connected) return;
  connected = false;
  stream = null;
  // The video keeps showing its last frame; just surface a subtle status.
  status('Controller disconnected', 'Waiting to reconnect…');
  refreshChrome();
}

function opener(): (Window & Record<string, unknown>) | null {
  return (window.opener as (Window & Record<string, unknown>) | null) ?? null;
}

/** Pick up the control window's shared mirror stream. Cross-realm same-origin
 *  reads are permitted; a stale/inactive stream is ignored. */
function pickUpStream(): void {
  try {
    const s = opener()?.[STREAM_KEY] as MediaStream | undefined;
    if (s && s.active && s !== stream) setStream(s);
  } catch {
    /* opener navigating — retry next tick */
  }
}

function sendHello(): void {
  const o = opener();
  if (!o) {
    status('No controller', 'Open this window from the main projectM-rs window.');
    return;
  }
  o.postMessage(makeMsg('hello', peer), location.origin);
}

window.addEventListener('message', (e) => {
  if (e.origin !== location.origin) return;
  const msg = parseMessage(e.data);
  if (!msg) return;
  if (msg.t === 'track') pickUpStream();
});

// Retry hello until connected (covers a late Open, or a controller reload);
// once connected, ping so the controller can show a live status. Also detect
// the controller going away (stream ended / opener closed) and reconnect.
sendHello();
setInterval(() => {
  const o = opener();
  if (connected && (!o || o.closed || !stream || !stream.active)) onDisconnect();
  if (!connected) sendHello();
  else o?.postMessage(makeMsg('ping', peer), location.origin);
}, 1000);

window.addEventListener('beforeunload', () => opener()?.postMessage(makeMsg('bye', peer), location.origin));

// Fullscreen (must be a user gesture).
fsBtn.addEventListener('click', async () => {
  try {
    await document.documentElement.requestFullscreen();
  } catch {
    /* denied — leave windowed */
  }
});
document.addEventListener('fullscreenchange', refreshChrome);

// Hide the cursor after inactivity; restore on movement.
let cursorTimer = 0;
const pokeCursor = () => {
  document.body.classList.remove('hide-cursor');
  clearTimeout(cursorTimer);
  cursorTimer = window.setTimeout(() => document.body.classList.add('hide-cursor'), 2500);
};
window.addEventListener('pointermove', pokeCursor);
pokeCursor();

// Resolution diagnostics (press 'd' to toggle).
setInterval(() => {
  diag.textContent = `src ${video.videoWidth}×${video.videoHeight} · win ${window.innerWidth}×${window.innerHeight} · dpr ${(window.devicePixelRatio || 1).toFixed(2)} · ${connected ? 'connected' : 'waiting'}`;
}, 500);
window.addEventListener('keydown', (e) => {
  if (e.key === 'd') diag.classList.toggle('hidden');
});

status('Waiting for controller…', 'Connecting to the main window…');
