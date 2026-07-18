// AudioWorklet capture processor. Runs on the browser's audio rendering thread.
// It does NO analysis — it only moves normalized PCM to the wasm consumer as
// cheaply as possible. Two transports:
//
//   shared === true  : write directly into a SharedArrayBuffer ring (preferred;
//                      requires crossOriginIsolated). Lock-free SPSC via Atomics.
//   shared === false : postMessage interleaved copies to the main thread, which
//                      writes them into an equivalent (non-shared) ring.
//
// Ring control slots (Int32): [0]=write [1]=read [2]=overruns [3]=underruns
// [4]=channels [5]=sampleRate. Indices are in samples.

class CaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = (options && options.processorOptions) || {};
    this.shared = !!o.shared;
    if (this.shared) {
      this.control = new Int32Array(o.control);
      this.data = new Float32Array(o.data);
      this.capacity = this.data.length;
      // sampleRate is a global in the AudioWorkletGlobalScope.
      Atomics.store(this.control, 5, sampleRate | 0);
    } else {
      this.capacity = o.capacity || 16384;
    }
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0 || !input[0]) return true;
    const channels = input.length;
    const frames = input[0].length;

    if (this.shared) {
      Atomics.store(this.control, 4, channels);
      const cap = this.capacity;
      let w = Atomics.load(this.control, 0);
      const r = Atomics.load(this.control, 1);
      for (let i = 0; i < frames; i++) {
        for (let c = 0; c < channels; c++) {
          const next = (w + 1) % cap;
          if (next === r) {
            // Ring full: consumer is behind. Count and drop the rest of block.
            Atomics.add(this.control, 2, 1);
            Atomics.store(this.control, 0, w);
            return true;
          }
          this.data[w] = input[c][i];
          w = next;
        }
      }
      Atomics.store(this.control, 0, w);
    } else {
      // Interleave into a fresh buffer and transfer it to the main thread.
      const buf = new Float32Array(frames * channels);
      for (let i = 0; i < frames; i++) {
        for (let c = 0; c < channels; c++) {
          buf[i * channels + c] = input[c][i];
        }
      }
      this.port.postMessage({ channels, sampleRate: sampleRate | 0, samples: buf }, [buf.buffer]);
    }
    return true;
  }
}

registerProcessor('pm-capture', CaptureProcessor);
