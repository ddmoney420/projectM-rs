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
  rename_layer,
  layers_json,
  selected_controls_json,
  export_scene,
  import_scene,
  reset_scene,
} from './pm_web/pm_web.js';

const SCENE_KEY = 'pm-web-scene-v1';
const BLEND = ['normal', 'add', 'screen', 'multiply', 'difference', 'lighten', 'darken'];

interface LayerInfo {
  id: number;
  name: string;
  kind: string;
  enabled: boolean;
  visible: boolean;
  opacity: number;
  blend: number;
  selected: boolean;
}
export interface SelectedShader {
  source: string;
  mode: number;
  controls: Array<{ name: string; kind: string; min: number; max: number; slot: number; default: number[]; options: string[] }>;
}

export class LayerPanel {
  private host: HTMLElement;
  private list!: HTMLElement;

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
      <div id="lp-list"></div>
      <div class="lp-scene">
        <button id="lp-export">Export</button>
        <button id="lp-import">Import</button>
        <button id="lp-reset">Reset</button>
        <input id="lp-file" type="file" accept="application/json" hidden />
      </div>`;
    this.list = host.querySelector('#lp-list')!;

    host.querySelectorAll<HTMLButtonElement>('.lp-add button').forEach((b) =>
      b.addEventListener('click', () => {
        add_layer(Number(b.dataset.k));
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
  }

  private row(l: LayerInfo): HTMLElement {
    const row = document.createElement('div');
    row.className = 'lp-row' + (l.selected ? ' sel' : '');
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
