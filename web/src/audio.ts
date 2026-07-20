// Browser audio bridge. Manages the AudioContext, the capture AudioWorklet, the
// PCM ring, and the three sources (file, mic, tab/display) behind one
// source-management abstraction. All active sources are mixed into a common
// graph and fed to the worklet, which hands PCM to the Rust analyzer via the
// ring. Analysis (FFT/beat/waveform) stays in Rust — this file never analyzes.
//
// Graph:
//   file:    <audio> → MediaElementSource → gain → captureBus → worklet   (analysis)
//                                            gain → destination            (audible)
//   mic:     getUserMedia → MediaStreamSource → gain → captureBus          (no monitoring)
//   display: getDisplayMedia → MediaStreamSource → gain → captureBus       (no echo)
//   captureBus(masterGain) → worklet → zeroGain(0) → destination           (keeps worklet pulled)

import { set_audio_ring } from './pm_web/pm_web.js';

const RING_CAPACITY = 16384; // samples (interleaved)

export type SourceKind = 'file' | 'mic' | 'display';

interface Source {
  kind: SourceKind;
  node: AudioNode;
  gain: GainNode;
  baseGain: number;
  muted: boolean;
  stream?: MediaStream;
  media?: HTMLAudioElement;
}

export interface AudioStatus {
  contextState: string;
  sampleRate: number;
  shared: boolean;
  sources: SourceKind[];
}

export class AudioEngine {
  private ctx: AudioContext | null = null;
  private worklet: AudioWorkletNode | null = null;
  private captureBus: GainNode | null = null;
  private zeroGain: GainNode | null = null;
  private control: Int32Array | null = null;
  private data: Float32Array | null = null;
  private shared = false;
  private sources = new Map<SourceKind, Source>();

  /** Callback fired when a source ends/errors externally, so the UI can update. */
  onSourcesChanged: (() => void) | null = null;

  // iOS mutes (and sometimes ends) the getUserMedia track on interruptions —
  // orientation change, backgrounding, audio-session changes — and does not
  // reliably auto-restore it, leaving a "live" but silent mic (AudioContext
  // still running, ring fill ~0%). We re-acquire on demand; these hold the
  // constraints + a re-entrancy guard for that path.
  private readonly micConstraints: MediaStreamConstraints = {
    audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false },
  };
  private reacquiringMic = false;

  /** Lazily create the AudioContext + worklet + ring on a user gesture. */
  private async ensureContext(): Promise<void> {
    if (this.ctx) {
      if (this.ctx.state === 'suspended') await this.ctx.resume();
      return;
    }
    const ctx = new AudioContext();
    // iOS Safari suspends the AudioContext on tab hide, orientation change, and
    // audio-session interruptions, which silently freezes audio reactivity while
    // rendering continues. Auto-resume whenever it flips to suspended (a no-op
    // without a recent gesture; harmless otherwise). Gesture-driven resumes are
    // wired in main.ts.
    ctx.addEventListener('statechange', () => {
      if (ctx.state === 'suspended') void ctx.resume().catch(() => {});
    });
    await ctx.audioWorklet.addModule(new URL('./audio-worklet.js', import.meta.url).href);

    this.shared = self.crossOriginIsolated === true;
    let processorOptions: Record<string, unknown>;
    if (this.shared) {
      const controlSAB = new SharedArrayBuffer(6 * 4);
      const dataSAB = new SharedArrayBuffer(RING_CAPACITY * 4);
      this.control = new Int32Array(controlSAB);
      this.data = new Float32Array(dataSAB);
      processorOptions = { shared: true, control: controlSAB, data: dataSAB };
    } else {
      // Fallback: non-shared ring; the worklet posts samples we write here.
      this.control = new Int32Array(new ArrayBuffer(6 * 4));
      this.data = new Float32Array(new ArrayBuffer(RING_CAPACITY * 4));
      this.control[4] = 2;
      this.control[5] = ctx.sampleRate | 0;
      processorOptions = { shared: false, capacity: RING_CAPACITY };
    }

    const worklet = new AudioWorkletNode(ctx, 'pm-capture', {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      processorOptions,
    });
    if (!this.shared) {
      worklet.port.onmessage = (e) => this.onWorkletSamples(e.data);
    }

    const captureBus = ctx.createGain();
    captureBus.gain.value = 1.0;
    captureBus.connect(worklet);

    // Worklet must reach the destination to be pulled, but stays inaudible.
    const zeroGain = ctx.createGain();
    zeroGain.gain.value = 0.0;
    worklet.connect(zeroGain);
    zeroGain.connect(ctx.destination);

    this.ctx = ctx;
    this.worklet = worklet;
    this.captureBus = captureBus;
    this.zeroGain = zeroGain;

    set_audio_ring(this.control, this.data, RING_CAPACITY);
  }

  /** Fallback producer: write posted samples into the non-shared ring. */
  private onWorkletSamples(msg: { channels: number; samples: Float32Array }): void {
    const control = this.control;
    const data = this.data;
    if (!control || !data) return;
    control[4] = msg.channels;
    const cap = RING_CAPACITY;
    let w = Atomics.load(control, 0);
    const r = Atomics.load(control, 1);
    const s = msg.samples;
    for (let k = 0; k < s.length; k++) {
      const next = (w + 1) % cap;
      if (next === r) {
        Atomics.add(control, 2, 1); // overrun
        break;
      }
      data[w] = s[k];
      w = next;
    }
    Atomics.store(control, 0, w);
  }

  // --- Sources ------------------------------------------------------------

  /** Load and play a local audio file, routed through the analyzer graph. */
  async addFile(file: File): Promise<HTMLAudioElement> {
    await this.ensureContext();
    this.removeSource('file');
    const el = new Audio();
    el.src = URL.createObjectURL(file);
    el.loop = true;
    const node = this.ctx!.createMediaElementSource(el);
    const gain = this.ctx!.createGain();
    node.connect(gain);
    gain.connect(this.captureBus!); // analysis
    gain.connect(this.ctx!.destination); // audible
    this.sources.set('file', { kind: 'file', node, gain, baseGain: 1, muted: false, media: el });
    await el.play().catch(() => {});
    this.notify();
    return el;
  }

  fileElement(): HTMLAudioElement | null {
    return this.sources.get('file')?.media ?? null;
  }

  /** Enable the microphone (analysis only — never monitored back to output). */
  async enableMic(): Promise<void> {
    if (typeof navigator.mediaDevices?.getUserMedia !== 'function') {
      throw new Error('Microphone input is not available in this browser.');
    }
    await this.ensureContext();
    this.removeSource('mic');
    const stream = await navigator.mediaDevices.getUserMedia(this.micConstraints);
    const node = this.ctx!.createMediaStreamSource(stream);
    const gain = this.ctx!.createGain();
    node.connect(gain);
    gain.connect(this.captureBus!); // analysis only — no destination
    this.sources.set('mic', { kind: 'mic', node, gain, baseGain: 1, muted: false, stream });
    this.attachMicTrackHandlers(stream);
    this.notify();
  }

  /** iOS fires 'mute' on the mic track when interrupted (and sometimes 'ended');
   *  both leave the source silent, so try to recover a fresh stream rather than
   *  silently dying. A short settle delay lets the interruption (e.g. rotation)
   *  finish, otherwise iOS may hand back another already-muted track. */
  private attachMicTrackHandlers(stream: MediaStream): void {
    stream.getAudioTracks().forEach((t) => {
      t.addEventListener('mute', () => setTimeout(() => void this.recoverMicIfStalled(), 400));
      t.addEventListener('ended', () => void this.recoverMicIfStalled());
    });
  }

  /** Re-acquire the microphone if its track has gone muted/ended (iOS after an
   *  orientation change / interruption), swapping a fresh stream into the graph
   *  while preserving the source's gain/mute and its captureBus wiring. No-op
   *  when the mic is healthy, absent, or a re-acquire is already in flight — so
   *  it is safe to call liberally (gestures, orientation, visibility). */
  async recoverMicIfStalled(): Promise<void> {
    const ctx = this.ctx;
    const s = this.sources.get('mic');
    if (!ctx || !s || this.reacquiringMic) return;
    const track = s.stream?.getAudioTracks()[0];
    if (track && track.readyState === 'live' && !track.muted) return; // healthy
    this.reacquiringMic = true;
    try {
      const stream = await navigator.mediaDevices.getUserMedia(this.micConstraints);
      try {
        s.node.disconnect();
      } catch {
        /* already disconnected */
      }
      s.stream?.getTracks().forEach((t) => t.stop());
      const node = ctx.createMediaStreamSource(stream);
      node.connect(s.gain); // reuse the existing gain → captureBus wiring
      s.node = node;
      s.stream = stream;
      this.attachMicTrackHandlers(stream);
      this.notify();
    } catch {
      /* couldn't re-acquire (permission/interruption still active) — leave as-is */
    } finally {
      this.reacquiringMic = false;
    }
  }

  /** Capture tab/system audio (analysis only, to avoid echo). Video is dropped. */
  async enableDisplay(): Promise<void> {
    if (typeof navigator.mediaDevices?.getDisplayMedia !== 'function') {
      throw new Error('Tab/system audio capture is not supported in this browser (e.g. iOS Safari).');
    }
    await this.ensureContext();
    this.removeSource('display');
    const stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: true });
    const audioTracks = stream.getAudioTracks();
    if (audioTracks.length === 0) {
      stream.getTracks().forEach((t) => t.stop());
      throw new Error('No audio track captured. Pick a tab and enable "Share tab audio".');
    }
    stream.getVideoTracks().forEach((t) => t.stop()); // audio only
    const node = this.ctx!.createMediaStreamSource(stream);
    const gain = this.ctx!.createGain();
    node.connect(gain);
    gain.connect(this.captureBus!);
    audioTracks[0].addEventListener('ended', () => this.removeSource('display'));
    this.sources.set('display', { kind: 'display', node, gain, baseGain: 1, muted: false, stream });
    this.notify();
  }

  /** Disable a source and release its resources (tracks, media, nodes). */
  removeSource(kind: SourceKind): void {
    const s = this.sources.get(kind);
    if (!s) return;
    try {
      s.node.disconnect();
      s.gain.disconnect();
    } catch {
      /* already disconnected */
    }
    if (s.media) {
      s.media.pause();
      const src = s.media.src;
      s.media.src = '';
      if (src.startsWith('blob:')) URL.revokeObjectURL(src);
    }
    if (s.stream) s.stream.getTracks().forEach((t) => t.stop());
    this.sources.delete(kind);
    this.notify();
  }

  // --- Mixing controls ----------------------------------------------------

  setSourceGain(kind: SourceKind, v: number): void {
    const s = this.sources.get(kind);
    if (!s) return;
    s.baseGain = v;
    s.gain.gain.value = s.muted ? 0 : v;
  }

  setSourceMute(kind: SourceKind, muted: boolean): void {
    const s = this.sources.get(kind);
    if (!s) return;
    s.muted = muted;
    s.gain.gain.value = muted ? 0 : s.baseGain;
  }

  setMasterGain(v: number): void {
    if (this.captureBus) this.captureBus.gain.value = v;
  }

  hasSource(kind: SourceKind): boolean {
    return this.sources.has(kind);
  }

  async resume(): Promise<void> {
    if (this.ctx && this.ctx.state === 'suspended') await this.ctx.resume();
  }

  /** A MediaStream of the mixed program audio (post master gain), for recording.
   *  Taps the capture bus so it mirrors what the analyzer hears without altering
   *  the audible graph. Returns null if no audio context exists yet. */
  private recordDest: MediaStreamAudioDestinationNode | null = null;
  captureAudioStream(): MediaStream | null {
    if (!this.ctx || !this.captureBus) return null;
    if (!this.recordDest) {
      this.recordDest = this.ctx.createMediaStreamDestination();
      this.captureBus.connect(this.recordDest);
    }
    return this.recordDest.stream;
  }

  status(): AudioStatus {
    return {
      contextState: this.ctx?.state ?? 'none',
      sampleRate: this.ctx?.sampleRate ?? 0,
      shared: this.shared,
      sources: [...this.sources.keys()],
    };
  }

  private notify(): void {
    this.onSourcesChanged?.();
  }
}
