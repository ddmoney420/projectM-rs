// Control-window side of projection. Serves the output window a live mirror of
// the WebGPU canvas by capturing it (`canvas.captureStream()`) and TRANSFERRING
// the resulting MediaStreamTrack via postMessage. The output window shows it in
// a <video>; no scene/audio/clock is serialized (see projection-protocol.ts).
//
// Serving is driven by the output window's `hello` (not by our open() handle),
// so it also works after the controller reloads: the still-open output window
// keeps saying hello to its opener, and the reloaded controller answers with a
// fresh track — automatic reconnection with no server.

import { makeMsg, parseMessage, newPeer, type Peer } from './projection-protocol';

// The captured mirror stream is shared with the (same-origin) output window by
// reference on the control window's `window`, because MediaStreamTrack transfer
// via postMessage is not supported in all target browsers. The output window
// reads it as `window.opener.<KEY>` and assigns it to its <video>.srcObject.
const STREAM_KEY = '__pmOutputStream';

export class ProjectionManager {
  private canvas: HTMLCanvasElement;
  private handle: Window | null = null;
  private lastSource: Window | null = null;
  private peer = newPeer();
  private connectedPeer: Peer | null = null;
  private lastSeen = 0;
  private served = 0;
  private stream: MediaStream | null = null;
  /** Fired when open/connected status changes. */
  onStatus: (() => void) | null = null;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    window.addEventListener('message', (e) => this.onMessage(e));
  }

  private onMessage(e: MessageEvent): void {
    if (e.origin !== location.origin) return;
    const msg = parseMessage(e.data);
    if (!msg) return;
    this.lastSeen = Date.now();
    this.lastSource = (e.source as Window | null) ?? this.lastSource;
    if (msg.t === 'hello') {
      this.connectedPeer = msg.peer;
      this.serve(e.source as Window | null);
      this.onStatus?.();
    } else if (msg.t === 'ping') {
      this.connectedPeer = msg.peer;
      this.onStatus?.();
    } else if (msg.t === 'bye') {
      if (msg.peer === this.connectedPeer) {
        this.connectedPeer = null;
        this.onStatus?.();
      }
    }
  }

  /** Ensure a live capture stream exists and is shared for the output window. */
  private ensureStream(): MediaStream | null {
    try {
      if (!this.stream || !this.stream.active) {
        this.stream = (this.canvas as HTMLCanvasElement & { captureStream(fps?: number): MediaStream }).captureStream(30);
      }
      (window as unknown as Record<string, unknown>)[STREAM_KEY] = this.stream;
      return this.stream;
    } catch (err) {
      // Capture failure must not affect the main renderer — just log.
      console.warn('projection: captureStream failed', err);
      return null;
    }
  }

  /** Point `target` at the shared mirror stream. */
  private serve(target: Window | null): void {
    if (!target) return;
    if (!this.ensureStream()) return;
    target.postMessage(makeMsg('track', this.peer), location.origin);
    this.served++;
  }

  /** Open the single output window (must be called from a user gesture). */
  open(): 'opened' | 'blocked' | 'exists' {
    if (this.handle && !this.handle.closed) {
      this.handle.focus();
      return 'exists';
    }
    const url = new URL('output.html', location.href).href;
    const w = window.open(url, 'pm-output', 'width=960,height=540');
    if (!w) return 'blocked';
    this.handle = w;
    this.onStatus?.();
    return 'opened';
  }

  close(): void {
    try {
      this.handle?.close();
    } catch {
      /* already gone */
    }
    this.handle = null;
    this.connectedPeer = null;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = null;
    delete (window as unknown as Record<string, unknown>)[STREAM_KEY];
    this.onStatus?.();
  }

  /** Re-send a fresh track to the current output window. */
  resync(): void {
    this.serve(this.handle && !this.handle.closed ? this.handle : this.lastSource);
  }

  focusOutput(): void {
    try {
      (this.handle && !this.handle.closed ? this.handle : this.lastSource)?.focus();
    } catch {
      /* focus may be blocked by the browser */
    }
  }

  status(): { open: boolean; connected: boolean; served: number } {
    // If we opened the window and it has since closed, drop the connection.
    if (this.handle && this.handle.closed) {
      this.handle = null;
      this.connectedPeer = null;
    }
    const seenRecently = this.connectedPeer != null && Date.now() - this.lastSeen < 4000;
    const open = (!!this.handle && !this.handle.closed) || seenRecently;
    return { open, connected: seenRecently, served: this.served };
  }
}
