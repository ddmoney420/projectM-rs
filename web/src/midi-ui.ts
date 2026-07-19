// MIDI panel: enable Web MIDI, manage input devices, MIDI-Learn a mapping onto
// any registry target, and inspect/edit/remove all mappings with live values.
//
// The panel reads the target registry and mapping list from wasm as JSON and
// edits mappings through `midi_set_mapping_field`. It never applies MIDI values
// itself — that happens in Rust on the shared handler. `tick()` (driven at a
// low rate by main) refreshes the live value bars + diagnostics, and rebuilds
// the mapping rows only when the set actually changes (so editing a range field
// doesn't fight the refresh).

import { MidiManager } from './midi';
import {
  midi_learn_start,
  midi_learn_cancel,
  midi_is_learning,
  midi_clear_mapping,
  midi_clear_all,
  midi_set_mapping_field,
  midi_mappings_json,
  midi_registry_json,
  midi_diag_json,
} from './pm_web/pm_web.js';

interface Target {
  path: string;
  label: string;
  group: string;
  kind: string;
  min: number;
  max: number;
}
interface Mapping {
  id: number;
  target: string;
  resolved: boolean;
  device: string;
  channel: string | number;
  messageType: string;
  selector: number;
  mode: string;
  outMin: number;
  outMax: number;
  invert: boolean;
  pickup: boolean;
  curve: string;
  smoothing: number;
  engaged: boolean;
  value: number | null;
}

const MODES = ['absolute', 'toggle', 'momentary', 'trigger'];

const el = (html: string): HTMLElement => {
  const t = document.createElement('template');
  t.innerHTML = html.trim();
  return t.content.firstElementChild as HTMLElement;
};
const esc = (s: string): string =>
  s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c] || c);

export class MidiPanel {
  private host: HTMLElement;
  private midi: MidiManager;
  /** Persist hook (called after a mapping is learned/edited/removed). */
  onChange: (() => void) | null = null;

  private maps!: HTMLElement;
  private diag!: HTMLElement;
  private targetSel!: HTMLSelectElement;
  private lastSig = '';
  private lastRegistry = '';
  private wasLearning = false;

  constructor(host: HTMLElement, midi: MidiManager) {
    this.host = host;
    this.midi = midi;
    host.innerHTML = `
      <div class="midi-head">
        <div class="midi-enable"></div>
        <div class="midi-devices"></div>
      </div>
      <div class="midi-learn">
        <select id="midi-target" title="Target to map"></select>
        <button id="midi-learn-btn">MIDI Learn</button>
        <span id="midi-learn-state"></span>
      </div>
      <div class="midi-maps-head"><b>Mappings</b> <button id="midi-clear-all" title="Remove all mappings">Clear all</button></div>
      <div id="midi-maps"></div>
      <div id="midi-diag" class="midi-diag"></div>`;
    this.maps = host.querySelector('#midi-maps')!;
    this.diag = host.querySelector('#midi-diag')!;
    this.targetSel = host.querySelector('#midi-target') as HTMLSelectElement;

    host.querySelector('#midi-learn-btn')!.addEventListener('click', () => this.toggleLearn());
    host.querySelector('#midi-clear-all')!.addEventListener('click', () => {
      midi_clear_all();
      this.onChange?.();
      this.renderMaps(this.readMaps());
    });

    this.midi.onChange = () => this.renderHead();
    this.renderHead();
    this.refreshTargets();
    this.renderMaps(this.readMaps());
  }

  // --- Header: enable + devices ------------------------------------------

  private renderHead(): void {
    const enableBox = this.host.querySelector('.midi-enable')!;
    const devBox = this.host.querySelector('.midi-devices')!;
    enableBox.innerHTML = '';
    devBox.innerHTML = '';

    if (!this.midi.supported) {
      enableBox.appendChild(el('<div class="midi-warn">Web MIDI is unavailable in this browser. Use an up-to-date Chrome or Edge.</div>'));
      return;
    }
    if (!this.midi.enabled) {
      const btn = el('<button id="midi-enable-btn">Enable MIDI</button>');
      btn.addEventListener('click', async () => {
        btn.textContent = 'Requesting…';
        try {
          await this.midi.enable();
        } catch (e) {
          enableBox.appendChild(el(`<div class="midi-warn">MIDI permission denied or failed: ${esc(e instanceof Error ? e.message : String(e))}</div>`));
          btn.textContent = 'Enable MIDI';
        }
      });
      enableBox.appendChild(btn);
      return;
    }

    enableBox.appendChild(el('<span class="midi-on">MIDI enabled</span>'));
    const devices = this.midi.devices();
    const all = el(`<label class="midi-all"><input type="checkbox" ${this.midi.allInputs ? 'checked' : ''}/> All inputs</label>`);
    (all.querySelector('input') as HTMLInputElement).addEventListener('change', (e) => {
      const on = (e.target as HTMLInputElement).checked;
      this.midi.setAllInputs(on);
      if (on) this.midi.selectedId = null;
    });
    devBox.appendChild(all);

    if (devices.length === 0) {
      devBox.appendChild(el('<div class="midi-hint">No MIDI inputs detected. Connect a controller.</div>'));
    }
    const sel = el('<select class="midi-devsel"></select>') as HTMLSelectElement;
    sel.disabled = this.midi.allInputs;
    sel.appendChild(el('<option value="">— select input —</option>'));
    for (const d of devices) {
      const o = el(`<option value="${esc(d.id)}" ${d.selected ? 'selected' : ''}>${esc(d.name)}${d.connected ? '' : ' (disconnected)'}</option>`);
      sel.appendChild(o);
    }
    sel.addEventListener('change', () => this.midi.select(sel.value || null));
    devBox.appendChild(sel);
  }

  // --- Target picker ------------------------------------------------------

  /** Rebuild the target dropdown from the registry, preserving the selection.
   *  Called from tick() when the registry changes (and never while it's open). */
  refreshTargets(): void {
    const json = midi_registry_json();
    if (json === this.lastRegistry) return;
    this.lastRegistry = json;
    if (document.activeElement === this.targetSel) return; // don't yank it open
    const prev = this.targetSel.value;
    let targets: Target[] = [];
    try {
      targets = JSON.parse(json);
    } catch {
      /* ignore */
    }
    const groups = new Map<string, Target[]>();
    for (const t of targets) {
      if (!groups.has(t.group)) groups.set(t.group, []);
      groups.get(t.group)!.push(t);
    }
    this.targetSel.innerHTML = '';
    for (const [group, items] of groups) {
      const og = document.createElement('optgroup');
      og.label = group;
      for (const t of items) {
        const o = document.createElement('option');
        o.value = t.path;
        o.textContent = `${t.label} [${t.kind[0]}]`;
        og.appendChild(o);
      }
      this.targetSel.appendChild(og);
    }
    if (prev) this.targetSel.value = prev;
  }

  // --- Learn --------------------------------------------------------------

  private toggleLearn(): void {
    if (midi_is_learning()) {
      midi_learn_cancel();
    } else {
      const path = this.targetSel.value;
      if (path) midi_learn_start(path);
    }
    this.renderLearnState();
  }

  private renderLearnState(): void {
    const btn = this.host.querySelector('#midi-learn-btn') as HTMLButtonElement;
    const state = this.host.querySelector('#midi-learn-state')!;
    const learning = midi_is_learning();
    btn.classList.toggle('on', learning);
    btn.textContent = learning ? 'Cancel' : 'MIDI Learn';
    state.textContent = learning ? 'move a knob / press a pad…' : '';
    // A learn that just completed (bound a control) is a mapping change → persist.
    if (this.wasLearning && !learning) this.onChange?.();
    this.wasLearning = learning;
  }

  // --- Mappings -----------------------------------------------------------

  private readMaps(): Mapping[] {
    try {
      return JSON.parse(midi_mappings_json());
    } catch {
      return [];
    }
  }

  private renderMaps(maps: Mapping[]): void {
    this.maps.innerHTML = '';
    if (maps.length === 0) {
      this.maps.appendChild(el('<div class="midi-hint">No mappings yet. Pick a target, click MIDI Learn, then move a control.</div>'));
      return;
    }
    for (const m of maps) this.maps.appendChild(this.mapRow(m));
  }

  private mapRow(m: Mapping): HTMLElement {
    const chan = m.channel === 'any' ? 'omni' : `ch${Number(m.channel) + 1}`;
    const src = m.messageType === 'note' ? `note ${m.selector}` : m.messageType === 'cc' ? `CC ${m.selector}` : 'bend';
    const row = el(`<div class="midi-map${m.resolved ? '' : ' unresolved'}" data-id="${m.id}">
      <div class="mm-top">
        <span class="mm-target" title="${esc(m.target)}">${esc(m.target)}${m.resolved ? '' : ' · target missing'}</span>
        <button class="mm-del" title="Remove mapping">✕</button>
      </div>
      <div class="mm-info">
        <span class="mm-dev">${esc(m.device || 'any device')}</span> · ${chan} · ${src}
        <select class="mm-mode">${MODES.map((o) => `<option value="${o}" ${o === m.mode ? 'selected' : ''}>${o}</option>`).join('')}</select>
      </div>
      <div class="mm-range">
        <label>min <input class="mm-min" type="number" step="0.01" value="${m.outMin}"></label>
        <label>max <input class="mm-max" type="number" step="0.01" value="${m.outMax}"></label>
        <label><input class="mm-inv" type="checkbox" ${m.invert ? 'checked' : ''}> inv</label>
        <label><input class="mm-pickup" type="checkbox" ${m.pickup ? 'checked' : ''}> pickup</label>
      </div>
      <div class="mm-val"><span class="mm-bar"><span></span></span> <span class="mm-num"></span></div>
    </div>`);

    const set = (field: string, value: string) => {
      midi_set_mapping_field(m.id, field, value);
      this.onChange?.();
    };
    row.querySelector('.mm-del')!.addEventListener('click', () => {
      midi_clear_mapping(m.id);
      this.onChange?.();
      this.renderMaps(this.readMaps());
    });
    (row.querySelector('.mm-mode') as HTMLSelectElement).addEventListener('change', (e) => {
      set('mode', (e.target as HTMLSelectElement).value);
      this.lastSig = ''; // mode change alters row semantics → force rebuild next tick
    });
    (row.querySelector('.mm-min') as HTMLInputElement).addEventListener('change', (e) => set('out_min', (e.target as HTMLInputElement).value));
    (row.querySelector('.mm-max') as HTMLInputElement).addEventListener('change', (e) => set('out_max', (e.target as HTMLInputElement).value));
    (row.querySelector('.mm-inv') as HTMLInputElement).addEventListener('change', (e) => set('invert', String((e.target as HTMLInputElement).checked)));
    (row.querySelector('.mm-pickup') as HTMLInputElement).addEventListener('change', (e) => set('pickup', String((e.target as HTMLInputElement).checked)));
    this.updateRow(row, m);
    return row;
  }

  private updateRow(row: HTMLElement, m: Mapping): void {
    const bar = row.querySelector('.mm-bar > span') as HTMLElement;
    const num = row.querySelector('.mm-num') as HTMLElement;
    if (m.value == null) {
      bar.style.width = '0%';
      num.textContent = m.pickup && !m.engaged ? 'pickup…' : '–';
      return;
    }
    const lo = Math.min(m.outMin, m.outMax);
    const hi = Math.max(m.outMin, m.outMax);
    const pct = hi > lo ? ((m.value - lo) / (hi - lo)) * 100 : 0;
    bar.style.width = `${Math.max(0, Math.min(100, pct)).toFixed(0)}%`;
    num.textContent = m.value.toFixed(3);
  }

  private mapSig(maps: Mapping[]): string {
    // Rebuild only when the set/shape changes — not when a value or range edits.
    return maps.map((m) => `${m.id}:${m.mode}:${m.target}:${m.resolved}:${m.invert}:${m.pickup}`).join('|');
  }

  // --- Diagnostics --------------------------------------------------------

  private renderDiag(): void {
    let d: Record<string, unknown> = {};
    try {
      d = JSON.parse(midi_diag_json(this.midi.enabled, this.midi.deviceCount()));
    } catch {
      /* ignore */
    }
    const last =
      d.lastType && d.events
        ? `${d.lastType} ${Number(d.lastChannel) >= 0 ? 'ch' + (Number(d.lastChannel) + 1) : ''} ${Number(d.lastSelector) >= 0 ? '#' + d.lastSelector : ''} = ${d.lastValue} (${Number(d.lastNorm).toFixed(2)})`
        : '—';
    this.diag.innerHTML = `
      <div class="row"><span class="k">enabled</span><span class="v">${d.enabled}</span></div>
      <div class="row"><span class="k">devices</span><span class="v">${d.deviceCount}</span></div>
      <div class="row"><span class="k">learn</span><span class="v">${d.learning ? esc(String(d.learnTarget)) : 'idle'}</span></div>
      <div class="row"><span class="k">last msg</span><span class="v">${esc(last)}</span></div>
      <div class="row"><span class="k">events</span><span class="v">${d.events} · ${d.ignored} ignored</span></div>
      <div class="row"><span class="k">applied</span><span class="v">${d.applied}</span></div>`;
  }

  /** Low-rate refresh: value bars, diagnostics, learn state, and (when the
   *  registry changed) the target list. Cheap; safe to call ~5–10×/s. */
  tick(): void {
    this.refreshTargets();
    this.renderLearnState();
    this.renderDiag();
    const maps = this.readMaps();
    const sig = this.mapSig(maps);
    if (sig !== this.lastSig) {
      this.renderMaps(maps);
      this.lastSig = sig;
    } else {
      for (const m of maps) {
        const row = this.maps.querySelector(`.midi-map[data-id="${m.id}"]`) as HTMLElement | null;
        if (row) this.updateRow(row, m);
      }
    }
  }
}
