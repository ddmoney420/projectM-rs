// Effect-rack panel: a reorderable chain for the selected layer or the global
// scene output. Add/remove/duplicate/reorder/enable/select; the selected effect
// shows its parameter sliders, each with an audio/LFO modulation binding.

import {
  add_effect,
  remove_effect,
  duplicate_effect,
  move_effect,
  select_effect,
  set_effect_enabled,
  set_effect_param,
  set_effect_param_mod,
  reset_feedback,
  effects_json,
  add_effect_preset,
} from './pm_web/pm_web.js';

const TYPES: Array<[string, string]> = [
  ['brightness', 'Brightness'], ['contrast', 'Contrast'], ['saturation', 'Saturation'],
  ['hue', 'Hue rotate'], ['invert', 'Invert'], ['posterize', 'Posterize'],
  ['mirrorh', 'Mirror H'], ['mirrorv', 'Mirror V'], ['kaleidoscope', 'Kaleidoscope'],
  ['radial', 'Radial symmetry'], ['pixelate', 'Pixelate'], ['blur', 'Blur'],
  ['sharpen', 'Sharpen'], ['edge', 'Edge detect'], ['vignette', 'Vignette'],
  ['noise', 'Noise'], ['scanlines', 'Scanlines'], ['chromatic', 'Chromatic aberration'],
  ['rgbsplit', 'RGB split'], ['glitch', 'Glitch'], ['feedback', 'Feedback'], ['bloom', 'Bloom'],
];
const PRESETS: Array<[string, string]> = [['dreamy', 'Dreamy'], ['vhs', 'VHS'], ['tunnel', 'Tunnel'], ['acid', 'Acid']];
const MOD_SOURCES = ['none', 'bass', 'mid', 'treb', 'vol', 'beatPulse', 'beatPhase', 'lfo0', 'lfo1'];

interface EParam { name: string; min: number; max: number; base: number; source: string; amount: number }
interface EInfo { id: number; name: string; type: string; enabled: boolean; selected: boolean; params: EParam[] }

export class EffectRack {
  private host: HTMLElement;
  private mode: 'layer' | 'global' = 'global';
  private getLayer: () => number | null;
  onChanged: (() => void) | null = null;

  constructor(host: HTMLElement, getLayer: () => number | null) {
    this.host = host;
    this.getLayer = getLayer;
    host.innerHTML = `
      <div class="fx-bar">
        <span class="fx-tabs"><button id="fx-global" class="on">Global</button><button id="fx-layer">Layer</button></span>
      </div>
      <div class="fx-add">
        <select id="fx-type">${TYPES.map(([v, l]) => `<option value="${v}">${l}</option>`).join('')}</select>
        <button id="fx-add">Add</button>
      </div>
      <div class="fx-presets">${PRESETS.map(([v, l]) => `<button data-p="${v}">${l}</button>`).join('')}</div>
      <div id="fx-list"></div>
      <div id="fx-params"></div>`;

    host.querySelector('#fx-global')!.addEventListener('click', () => this.setMode('global'));
    host.querySelector('#fx-layer')!.addEventListener('click', () => this.setMode('layer'));
    host.querySelector('#fx-add')!.addEventListener('click', () => {
      add_effect(this.target(), (host.querySelector('#fx-type') as HTMLSelectElement).value);
      this.refresh();
      this.changed();
    });
    host.querySelectorAll<HTMLButtonElement>('.fx-presets button').forEach((b) =>
      b.addEventListener('click', () => {
        add_effect_preset(this.target(), b.dataset.p!);
        this.refresh();
        this.changed();
      }),
    );
    this.refresh();
  }

  private setMode(m: 'layer' | 'global'): void {
    this.mode = m;
    this.host.querySelector('#fx-global')!.classList.toggle('on', m === 'global');
    this.host.querySelector('#fx-layer')!.classList.toggle('on', m === 'layer');
    this.refresh();
  }

  private target(): number {
    return this.mode === 'global' ? 0 : this.getLayer() ?? 0;
  }

  private list(): EInfo[] {
    try {
      return JSON.parse(effects_json(this.target())).effects ?? [];
    } catch {
      return [];
    }
  }

  /** Refresh when layer selection changes and Layer mode is active. */
  refresh(): void {
    const effects = this.list();
    const listEl = this.host.querySelector('#fx-list')!;
    listEl.innerHTML = '';
    [...effects].reverse().forEach((e) => listEl.appendChild(this.row(e)));
    const sel = effects.find((e) => e.selected);
    this.renderParams(sel);
  }

  private changed(): void {
    this.onChanged?.();
  }

  /** Reflect live param base values (e.g. MIDI-driven) into the selected
   *  effect's sliders in place, skipping any the user is dragging. */
  syncValues(): void {
    const sel = this.list().find((e) => e.selected);
    if (!sel) return;
    const inputs = this.host.querySelectorAll('#fx-params .fx-param .v');
    sel.params.forEach((p, i) => {
      const inp = inputs[i] as HTMLInputElement | undefined;
      if (inp && document.activeElement !== inp) inp.value = String(p.base);
    });
  }

  private row(e: EInfo): HTMLElement {
    const row = document.createElement('div');
    row.className = 'fx-row' + (e.selected ? ' sel' : '');
    row.innerHTML = `
      <input class="en" type="checkbox" ${e.enabled ? 'checked' : ''} />
      <span class="nm">${e.name}</span>
      <span class="fx-btns"><button class="up">▲</button><button class="dn">▼</button><button class="dup">⧉</button><button class="rm">✕</button></span>`;
    const t = this.target();
    row.querySelector('.nm')!.addEventListener('click', () => { select_effect(t, e.id); this.refresh(); });
    (row.querySelector('.en') as HTMLInputElement).addEventListener('change', (ev) => {
      set_effect_enabled(t, e.id, (ev.target as HTMLInputElement).checked);
      this.changed();
    });
    (row.querySelector('.up') as HTMLButtonElement).addEventListener('click', () => { move_effect(t, e.id, false); this.refresh(); this.changed(); });
    (row.querySelector('.dn') as HTMLButtonElement).addEventListener('click', () => { move_effect(t, e.id, true); this.refresh(); this.changed(); });
    (row.querySelector('.dup') as HTMLButtonElement).addEventListener('click', () => { duplicate_effect(t, e.id); this.refresh(); this.changed(); });
    (row.querySelector('.rm') as HTMLButtonElement).addEventListener('click', () => { remove_effect(t, e.id); this.refresh(); this.changed(); });
    return row;
  }

  private renderParams(e: EInfo | undefined): void {
    const host = this.host.querySelector('#fx-params')!;
    host.innerHTML = '';
    if (!e) {
      return;
    }
    const t = this.target();
    if (e.type === 'feedback') {
      const btn = document.createElement('button');
      btn.textContent = 'Reset feedback';
      btn.addEventListener('click', () => reset_feedback(t));
      host.appendChild(btn);
    }
    e.params.forEach((p, i) => {
      const step = (p.max - p.min) / 200 || 0.01;
      const wrap = document.createElement('div');
      wrap.className = 'fx-param';
      wrap.innerHTML = `<label>${p.name} <input class="v" type="range" min="${p.min}" max="${p.max}" step="${step}" value="${p.base}" /></label>
        <div class="fx-mod"><select class="src">${MOD_SOURCES.map((s) => `<option value="${s}" ${s === p.source ? 'selected' : ''}>${s}</option>`).join('')}</select>
        <label>amt <input class="amt" type="range" min="-2" max="2" step="0.01" value="${p.amount}" /></label></div>`;
      (wrap.querySelector('.v') as HTMLInputElement).addEventListener('input', (ev) => {
        set_effect_param(t, e.id, i, Number((ev.target as HTMLInputElement).value));
        this.changed();
      });
      const applyMod = () => {
        set_effect_param_mod(
          t, e.id, i,
          (wrap.querySelector('.src') as HTMLSelectElement).value,
          Number((wrap.querySelector('.amt') as HTMLInputElement).value),
          0, 'linear', false,
        );
        this.changed();
      };
      wrap.querySelector('.src')!.addEventListener('change', applyMod);
      wrap.querySelector('.amt')!.addEventListener('input', applyMod);
      host.appendChild(wrap);
    });
  }
}
