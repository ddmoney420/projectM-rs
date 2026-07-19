// Versioned message protocol for the projection (second-screen) link between
// the control window and the output window.
//
// Architecture (Phase 8c): the output window is a MIRROR of the control
// window's rendered canvas — the control window captures its WebGPU canvas
// (`canvas.captureStream()`) and TRANSFERS the resulting MediaStreamTrack to
// the output window, which shows it in a <video>. So there is no scene/clock/
// audio to serialize per frame: the output is the exact same pixels, and every
// change (layers, shaders, effects, MIDI, tempo, feedback, audio-reactivity)
// propagates for free with perfect temporal fidelity. This protocol therefore
// only carries the small control-plane handshake: hello / track / bye / ping.
//
// The messages are transported by `postMessage` between the two same-origin
// windows (`window.opener` ↔ the popup). Each carries a protocol version (so a
// mismatched build fails clearly instead of corrupting state) and a peer id
// (so the design is ready for multiple outputs later, even though the UI
// currently allows one).

export const PROTOCOL_VERSION = 1;

export type Peer = string;
export type MsgType = 'hello' | 'track' | 'bye' | 'ping';

export interface ProjMsg {
  /** Discriminator so unrelated postMessages are ignored. */
  pm: 'proj';
  v: number;
  t: MsgType;
  peer: Peer;
}

const TYPES = new Set<MsgType>(['hello', 'track', 'bye', 'ping']);

/** A fresh, stable-per-window peer id. */
export function newPeer(): Peer {
  const c = globalThis.crypto as Crypto | undefined;
  if (c && 'randomUUID' in c) return c.randomUUID();
  // Fallback: time+counter (crypto.randomUUID is present in all target browsers).
  return `peer-${Date.now().toString(36)}-${(peerCounter++).toString(36)}`;
}
let peerCounter = 0;

export function makeMsg(t: MsgType, peer: Peer): ProjMsg {
  return { pm: 'proj', v: PROTOCOL_VERSION, t, peer };
}

/** Validate + narrow an incoming message. Returns null for anything that isn't
 *  a well-formed protocol message of the current version — an unknown/older
 *  version is rejected rather than acted on. */
export function parseMessage(data: unknown): ProjMsg | null {
  if (!data || typeof data !== 'object') return null;
  const m = data as Record<string, unknown>;
  if (m.pm !== 'proj') return null; // not one of ours
  if (m.v !== PROTOCOL_VERSION) return null; // version mismatch → reject
  if (typeof m.t !== 'string' || !TYPES.has(m.t as MsgType)) return null;
  if (typeof m.peer !== 'string' || m.peer.length === 0) return null;
  return { pm: 'proj', v: PROTOCOL_VERSION, t: m.t as MsgType, peer: m.peer };
}
