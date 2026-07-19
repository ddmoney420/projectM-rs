// Web MIDI access layer. Owns the browser MIDIAccess, the live input list, and
// device connect/disconnect — then forwards every incoming message to the wasm
// `midi_handle`, which is the single handler shared with dev/test injection.
//
// Access is never requested automatically: `enable()` runs only from an
// explicit user gesture, never asks for SysEx, and failure here can't touch
// rendering/audio (callers catch it). Mappings identify a device by its stable
// name (not array position or the ephemeral per-session id).

import { midi_handle } from './pm_web/pm_web.js';

// Web MIDI types (MIDIAccess/MIDIInput/MIDIMessageEvent) come from lib.dom.

export interface MidiDevice {
  id: string;
  name: string;
  manufacturer: string;
  connected: boolean;
  selected: boolean;
}

export class MidiManager {
  private access: MIDIAccess | null = null;
  private inputs = new Map<string, MIDIInput>();
  enabled = false;
  allInputs = true;
  /** Selected input id when not in All-Inputs mode. */
  selectedId: string | null = null;
  /** Fired when the device list / enabled state changes. */
  onChange: (() => void) | null = null;

  get supported(): boolean {
    return typeof navigator !== 'undefined' && 'requestMIDIAccess' in navigator;
  }

  /** Request MIDI access (user gesture only, no SysEx). Throws on failure. */
  async enable(): Promise<void> {
    if (!this.supported) throw new Error('Web MIDI is not supported by this browser');
    const access = await navigator.requestMIDIAccess({ sysex: false });
    this.access = access;
    this.enabled = true;
    access.onstatechange = () => this.refresh();
    this.refresh();
  }

  /** Re-scan inputs and (re)attach message handlers. Safe to call repeatedly. */
  refresh(): void {
    if (!this.access) return;
    this.inputs.clear();
    for (const input of this.access.inputs.values()) {
      this.inputs.set(input.id, input);
      input.onmidimessage = (e) => this.onMessage(input.id, e);
    }
    this.onChange?.();
  }

  private onMessage(id: string, e: MIDIMessageEvent): void {
    if (!this.enabled) return;
    if (!this.allInputs && this.selectedId && id !== this.selectedId) return;
    const d = e.data;
    if (!d || d.length < 1) return;
    midi_handle(this.deviceKey(id), d[0], d[1] ?? 0, d[2] ?? 0);
  }

  /** Stable device key used for mappings: the device name (falls back to id).
   *  Names survive across sessions far better than the per-session port id. */
  deviceKey(id: string): string {
    return this.inputs.get(id)?.name || id;
  }

  devices(): MidiDevice[] {
    return [...this.inputs.values()].map((i) => ({
      id: i.id,
      name: i.name || i.id,
      manufacturer: i.manufacturer || '',
      connected: i.state === 'connected',
      selected: this.selectedId === i.id,
    }));
  }
  deviceCount(): number {
    return this.inputs.size;
  }

  setAllInputs(all: boolean): void {
    this.allInputs = all;
    this.onChange?.();
  }
  select(id: string | null): void {
    this.selectedId = id;
    this.allInputs = id === null;
    this.onChange?.();
  }
}
