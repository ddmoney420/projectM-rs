// Output utilities: record the canvas to a downloadable WebM (with the mixed
// program audio when available), toggle fullscreen on the visualizer, and hold
// a screen wake lock so long sessions don't dim/sleep. All browser-only APIs;
// each degrades gracefully when unsupported.

/** Records the live canvas (canvas.captureStream) plus an optional audio track
 *  to a WebM blob, then downloads it. Nothing is uploaded anywhere. */
export class Recorder {
  private rec: MediaRecorder | null = null;
  private chunks: Blob[] = [];
  /** Fired on start/stop so the UI can reflect state. */
  onState: ((recording: boolean) => void) | null = null;

  get recording(): boolean {
    return this.rec !== null;
  }

  /** Start recording, or stop if already running. `audio` is an optional
   *  MediaStream whose first audio track is muxed into the recording. */
  toggle(canvas: HTMLCanvasElement, audio: MediaStream | null): void {
    if (this.rec) {
      this.stop();
      return;
    }
    // 60 fps capture; the browser caps to the actual present rate.
    const stream = (canvas as HTMLCanvasElement & { captureStream(fps?: number): MediaStream }).captureStream(60);
    const track = audio?.getAudioTracks()[0];
    if (track) stream.addTrack(track);

    const mime = pickMime();
    this.rec = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
    this.chunks = [];
    this.rec.ondataavailable = (e) => {
      if (e.data.size) this.chunks.push(e.data);
    };
    this.rec.onstop = () => {
      const blob = new Blob(this.chunks, { type: this.chunks[0]?.type || 'video/webm' });
      this.chunks = [];
      download(blob, `projectm-${stamp()}.webm`);
      this.rec = null;
      this.onState?.(false);
    };
    this.rec.start(1000); // gather in 1s chunks so a crash still leaves data
    this.onState?.(true);
  }

  stop(): void {
    // onstop finalizes; guard double-stop.
    if (this.rec && this.rec.state !== 'inactive') this.rec.stop();
  }
}

function pickMime(): string {
  const candidates = ['video/webm;codecs=vp9,opus', 'video/webm;codecs=vp8,opus', 'video/webm'];
  const ok = (m: string) =>
    typeof MediaRecorder !== 'undefined' && MediaRecorder.isTypeSupported && MediaRecorder.isTypeSupported(m);
  return candidates.find(ok) || '';
}

function download(blob: Blob, name: string): void {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

// A monotonic-ish filename stamp. Date.now()-free path isn't required in the
// browser, but keep it simple and dependency-light.
function stamp(): string {
  return new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
}

/** Toggle fullscreen on the given element (the canvas). */
export async function toggleFullscreen(target: Element): Promise<void> {
  if (document.fullscreenElement) {
    await document.exitFullscreen().catch(() => {});
  } else {
    await target.requestFullscreen?.().catch(() => {});
  }
}

/** Holds a screen wake lock while active, re-acquiring after tab visibility
 *  changes (the OS releases it when the page is hidden). */
export class WakeLock {
  private sentinel: WakeLockSentinel | null = null;
  private want = false;
  onState: ((held: boolean) => void) | null = null;

  get supported(): boolean {
    return 'wakeLock' in navigator;
  }

  constructor() {
    document.addEventListener('visibilitychange', () => {
      if (this.want && !document.hidden) void this.acquire();
    });
  }

  async toggle(): Promise<void> {
    if (this.want) {
      this.want = false;
      await this.sentinel?.release().catch(() => {});
      this.sentinel = null;
      this.onState?.(false);
    } else {
      this.want = true;
      await this.acquire();
    }
  }

  private async acquire(): Promise<void> {
    if (!this.supported || this.sentinel) return;
    try {
      this.sentinel = await navigator.wakeLock.request('screen');
      this.sentinel.addEventListener('release', () => {
        this.sentinel = null;
        this.onState?.(this.want && !document.hidden ? true : false);
      });
      this.onState?.(true);
    } catch {
      this.onState?.(false);
    }
  }
}
