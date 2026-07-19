// Runtime browser-capability detection. Only WebGPU is a hard requirement; every
// other API is optional and degrades gracefully. Surfaced under Diagnostics and
// About so a user can see why a given feature (audio share, MIDI, recording,
// wake lock, …) is unavailable on their browser/platform.

export interface Capability {
  name: string;
  ok: boolean;
  note: string;
}

export function detectCapabilities(): Capability[] {
  const md = navigator.mediaDevices as MediaDevices | undefined;
  return [
    { name: 'WebGPU', ok: 'gpu' in navigator, note: 'required — the visualizer needs it' },
    { name: 'crossOriginIsolated', ok: self.crossOriginIsolated === true, note: 'SharedArrayBuffer audio (else postMessage fallback)' },
    { name: 'AudioWorklet', ok: typeof AudioWorkletNode !== 'undefined', note: 'low-latency audio capture' },
    { name: 'Microphone', ok: !!md?.getUserMedia, note: 'mic input' },
    { name: 'Tab / system audio', ok: !!md?.getDisplayMedia, note: 'capture other tabs / apps' },
    { name: 'Recording', ok: typeof MediaRecorder !== 'undefined', note: 'record to WebM' },
    { name: 'Web MIDI', ok: 'requestMIDIAccess' in navigator, note: 'hardware control (Chrome/Edge)' },
    { name: 'Wake Lock', ok: 'wakeLock' in navigator, note: 'keep screen awake' },
    { name: 'Fullscreen', ok: typeof document.documentElement.requestFullscreen === 'function', note: 'fullscreen output' },
    { name: 'Canvas capture', ok: 'captureStream' in HTMLCanvasElement.prototype, note: 'projection + recording' },
  ];
}
