// Layer-stack panel: add/remove/duplicate/reorder/enable/opacity/blend per
// layer, plus scene export/import/reset with local persistence. Selecting a
// layer routes its source controls to the console (shaders) or overlay section.

import {
  add_layer,
  remove_layer,
  duplicate_layer,
  move_layer,
  select_layer,
  set_layer_enabled,
  set_layer_visible,
  set_layer_opacity,
  set_layer_blend,
  set_layer_transform,
  rename_layer,
  layers_json,
  selected_controls_json,
  export_scene,
  import_scene,
  reset_scene,
} from './pm_web/pm_web.js';

const SCENE_KEY = 'pm-web-scene-v1';
const BLEND = ['normal', 'add', 'screen', 'multiply', 'difference', 'lighten', 'darken'];

const el = (html: string): HTMLElement => {
  const t = document.createElement('template');
  t.innerHTML = html.trim();
  return t.content.firstElementChild as HTMLElement;
};

interface LayerInfo {
  id: number;
  name: string;
  kind: string;
  enabled: boolean;
  visible: boolean;
  opacity: number;
  blend: number;
  selected: boolean;
  tx: number;
  ty: number;
  sx: number;
  sy: number;
  rot: number;
}
export interface SelectedShader {
  source: string;
  mode: number;
  controls: Array<{ name: string; kind: string; min: number; max: number; slot: number; default: number[]; options: string[] }>;
}

export class LayerPanel {
  private host: HTMLElement;
  private list!: HTMLElement;
  private noteTimer: ReturnType<typeof setTimeout> | undefined;

  /** Show a transient message in the panel (e.g. why an add was rejected). */
  private note(msg: string): void {
    const el = this.host.querySelector('#lp-note') as HTMLElement | null;
    if (!el) return;
    el.textContent = msg;
    el.classList.add('show');
    clearTimeout(this.noteTimer);
    this.noteTimer = setTimeout(() => el.classList.remove('show'), 3200);
  }

  /** Fired when the selected layer changes (kind + its shader state + id). */
  onSelect: ((kind: string, shader: SelectedShader, layerId: number) => void) | null = null;

  constructor(host: HTMLElement) {
    this.host = host;
    host.innerHTML = `
      <div class="lp-add">
        <button data-k="0">+ Milkdrop</button>
        <button data-k="1">+ Shader</button>
        <button data-k="2">+ Waveform</button>
        <button data-k="3">+ Spectrum</button>
      </div>
      <div id="lp-note" class="lp-note"></div>
      <div id="lp-list"></div>
      <div id="lp-transform"></div>
      <div class="lp-scene">
        <button id="lp-export">Export</button>
        <button id="lp-import">Import</button>
        <button id="lp-reset">Reset</button>
        <input id="lp-file" type="file" accept="application/json" hidden />
      </div>`;
    this.list = host.querySelector('#lp-list')!;

    host.querySelectorAll<HTMLButtonElement>('.lp-add button').forEach((b) =>
      b.addEventListener('click', () => {
        const kind = Number(b.dataset.k);
        // add_layer returns the new id, or -1 when rejected (a second Milkdrop
        // is not supported, or a layer limit was hit). Without feedback this
        // reads as "the button is broken" — surface why.
        if (add_layer(kind) < 0) {
          this.note(
            kind === 0
              ? 'Only one Milkdrop layer is supported.'
              : 'Layer limit reached — remove a layer first.',
          );
          return;
        }
        this.refresh();
        this.save();
      }),
    );
    host.querySelector('#lp-export')!.addEventListener('click', () => this.exportScene());
    host.querySelector('#lp-import')!.addEventListener('click', () => (host.querySelector('#lp-file') as HTMLInputElement).click());
    (host.querySelector('#lp-file') as HTMLInputElement).addEventListener('change', (e) => this.importFile(e));
    host.querySelector('#lp-reset')!.addEventListener('click', () => {
      reset_scene();
      this.refresh();
      this.emitSelect();
      this.save();
    });

    this.restore();
    this.refresh();
    this.emitSelect();
  }

  private layers(): LayerInfo[] {
    try {
      return JSON.parse(layers_json());
    } catch {
      return [];
    }
  }

  refresh(): void {
    const layers = this.layers();
    this.list.innerHTML = '';
    // Render top-of-stack first (last composited = visually on top).
    [...layers].reverse().forEach((l) => this.list.appendChild(this.row(l)));
    this.renderTransform(layers.find((l) => l.selected));
  }

  /** Transform sub-panel for the selected layer (position/scale/rotation).
   *  Applies live via set_layer_transform — never recompiles a pipeline. Works
   *  the same for every layer type (Milkdrop included: the transform is applied
   *  to the layer's texture at composite time). */
  private renderTransform(l: LayerInfo | undefined): void {
    const host = this.host.querySelector('#lp-transform')!;
    host.innerHTML = '';
    if (!l) return;
    const box = el(`<div class="lp-xform">
      <div class="lp-xhead"><b>Transform</b> <span>${escapeHtml(l.name)}</span></div>
      <label>X <input class="tx" type="range" min="-1" max="1" step="0.005" value="${l.tx}"></label>
      <label>Y <input class="ty" type="range" min="-1" max="1" step="0.005" value="${l.ty}"></label>
      <label><input class="lock" type="checkbox"> uniform scale</label>
      <label>scaleX <input class="sx" type="range" min="0.1" max="4" step="0.01" value="${l.sx}"></label>
      <label>scaleY <input class="sy" type="range" min="0.1" max="4" step="0.01" value="${l.sy}"></label>
      <label>rot <input class="rot" type="range" min="-3.1416" max="3.1416" step="0.01" value="${l.rot}"></label>
      <button class="reset">Reset transform</button>
    </div>`);
    const inp = (c: string) => box.querySelector(`.${c}`) as HTMLInputElement;
    const apply = () => {
      const lock = inp('lock').checked;
      if (lock) inp('sy').value = inp('sx').value;
      set_layer_transform(l.id, Number(inp('tx').value), Number(inp('ty').value), Number(inp('sx').value), Number(inp('sy').value), Number(inp('rot').value));
      this.save();
    };
    ['tx', 'ty', 'sx', 'sy', 'rot'].forEach((c) => inp(c).addEventListener('input', apply));
    inp('lock').addEventListener('change', apply);
    box.querySelector('.reset')!.addEventListener('click', () => {
      set_layer_transform(l.id, 0, 0, 1, 1, 0);
      this.refresh();
      this.save();
    });
    host.appendChild(box);
  }

  private row(l: LayerInfo): HTMLElement {
    const row = document.createElement('div');
    row.className = 'lp-row' + (l.selected ? ' sel' : '');
    row.dataset.id = String(l.id);
    row.innerHTML = `
      <div class="lp-head">
        <input class="en" type="checkbox" ${l.enabled ? 'checked' : ''} title="enabled" />
        <span class="nm" title="${l.kind}">${escapeHtml(l.name)}</span>
        <span class="lp-btns">
          <button class="up" title="up">▲</button><button class="dn" title="down">▼</button>
          <button class="dup" title="duplicate">⧉</button><button class="rm" title="remove">✕</button>
        </span>
      </div>
      <div class="lp-body">
        <label>op <input class="op" type="range" min="0" max="1" step="0.01" value="${l.opacity}" /></label>
        <select class="bl">${BLEND.map((b, i) => `<option value="${i}" ${i === l.blend ? 'selected' : ''}>${b}</option>`).join('')}</select>
      </div>`;

    row.querySelector('.nm')!.addEventListener('click', () => {
      select_layer(l.id);
      this.refresh();
      this.emitSelect();
    });
    (row.querySelector('.en') as HTMLInputElement).addEventListener('change', (e) => {
      set_layer_enabled(l.id, (e.target as HTMLInputElement).checked);
      set_layer_visible(l.id, (e.target as HTMLInputElement).checked);
      this.save();
    });
    (row.querySelector('.op') as HTMLInputElement).addEventListener('input', (e) => {
      set_layer_opacity(l.id, Number((e.target as HTMLInputElement).value));
      this.save();
    });
    (row.querySelector('.bl') as HTMLSelectElement).addEventListener('change', (e) => {
      set_layer_blend(l.id, Number((e.target as HTMLSelectElement).value));
      this.save();
    });
    (row.querySelector('.up') as HTMLButtonElement).addEventListener('click', () => {
      move_layer(l.id, false); // visual "up" = later in composite = move toward end
      this.refresh();
      this.save();
    });
    (row.querySelector('.dn') as HTMLButtonElement).addEventListener('click', () => {
      move_layer(l.id, true);
      this.refresh();
      this.save();
    });
    (row.querySelector('.dup') as HTMLButtonElement).addEventListener('click', () => {
      duplicate_layer(l.id);
      this.refresh();
      this.emitSelect();
      this.save();
    });
    (row.querySelector('.rm') as HTMLButtonElement).addEventListener('click', () => {
      remove_layer(l.id);
      this.refresh();
      this.emitSelect();
      this.save();
    });
    // Double-click name to rename.
    row.querySelector('.nm')!.addEventListener('dblclick', () => {
      const name = prompt('Layer name', l.name);
      if (name != null) {
        rename_layer(l.id, name);
        this.refresh();
        this.save();
      }
    });
    return row;
  }

  /** Reflect live engine values (opacity + selected-layer transform) into the
   *  existing sliders without a full rebuild — so MIDI-driven changes show up.
   *  Skips any input the user is actively dragging. */
  syncValues(): void {
    const layers = this.layers();
    for (const l of layers) {
      const op = this.list.querySelector(`.lp-row[data-id="${l.id}"] .op`) as HTMLInputElement | null;
      if (op && document.activeElement !== op) op.value = String(l.opacity);
    }
    const sel = layers.find((l) => l.selected);
    if (sel) {
      const setv = (c: string, v: number) => {
        const e = this.host.querySelector(`#lp-transform .${c}`) as HTMLInputElement | null;
        if (e && document.activeElement !== e) e.value = String(v);
      };
      setv('tx', sel.tx);
      setv('ty', sel.ty);
      setv('sx', sel.sx);
      setv('sy', sel.sy);
      setv('rot', sel.rot);
    }
  }

  /** Notify listeners of the current selection + its shader state. */
  emitSelect(): void {
    const sel = this.layers().find((l) => l.selected);
    if (!sel) return;
    let shader: SelectedShader = { source: '', mode: 0, controls: [] };
    try {
      shader = JSON.parse(selected_controls_json());
    } catch {
      /* ignore */
    }
    this.onSelect?.(sel.kind, shader, sel.id);
  }

  // --- Scenes -------------------------------------------------------------

  private exportScene(): void {
    const json = export_scene();
    const blob = new Blob([json], { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'scene.json';
    a.click();
    URL.revokeObjectURL(a.href);
  }

  private async importFile(e: Event): Promise<void> {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (!file) return;
    const text = await file.text();
    this.importJson(text);
  }

  importJson(text: string): boolean {
    let res: { ok: boolean; error?: string };
    try {
      res = JSON.parse(import_scene(text));
    } catch {
      res = { ok: false, error: 'import failed' };
    }
    if (res.ok) {
      this.refresh();
      this.emitSelect();
      this.save();
      return true;
    }
    console.warn('scene import rejected:', res.error);
    return false;
  }

  save(): void {
    try {
      localStorage.setItem(SCENE_KEY, export_scene());
    } catch {
      /* storage unavailable */
    }
  }

  private restore(): void {
    let saved: string | null = null;
    try {
      saved = localStorage.getItem(SCENE_KEY);
    } catch {
      saved = null;
    }
    if (saved) {
      // Transactional in Rust: a bad saved scene leaves the default in place.
      this.importJson(saved);
    }
  }
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c] || c);
}
