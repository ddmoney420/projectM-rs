// Phase 5 controls panel: visual-time, tempo/beat, LFO bank, waveform/spectrum
// overlays, and dynamically-built user shader controls with audio/LFO
// modulation. Lightweight local persistence (versioned) of the settings.

import {
  set_time_scale,
  set_paused,
  reset_time,
  tempo_tap,
  tempo_set_bpm,
  tempo_set_manual,
  tempo_half,
  tempo_double,
  tempo_reset_phase,
  tempo_set_subdivision,
  set_lfo,
  set_overlay,
  set_control,
  set_control_mod,
} from './pm_web/pm_web.js';

export interface ShaderControl {
  name: string;
  kind: string;
  min: number;
  max: number;
  slot: number;
  default: number[];
  options: string[];
}

const STORE_KEY = 'pm-web-phase5-v1';
const MOD_SOURCES = [
  'none', 'bass', 'mid', 'treb', 'vol',
  'bassAtt', 'midAtt', 'trebAtt', 'volAtt',
  'beatPulse', 'beatPhase', 'lfo0', 'lfo1', 'lfo2', 'lfo3',
];
const OVERLAY_MODES = ['Oscilloscope', 'Mirrored', 'Spectrum bars', 'Circular', 'Radial spectrum', 'Lissajous'];

const el = (html: string): HTMLElement => {
  const t = document.createElement('template');
  t.innerHTML = html.trim();
  return t.content.firstElementChild as HTMLElement;
};

export class ControlsPanel {
  private root: HTMLElement;
  private paused = false;
  private userHost!: HTMLElement;

  constructor(host: HTMLElement) {
    this.root = host;
    host.innerHTML = `
      <details open><summary>Time</summary>
        <div class="cp-row"><button id="cp-play">Pause</button><button id="cp-reset">Reset</button></div>
        <div class="cp-row"><label>speed <input id="cp-speed" type="range" min="0" max="4" step="0.05" value="1"></label><span id="cp-speed-v">1.00×</span></div>
        <div class="cp-row cp-presets">
          <button data-s="0.25">0.25×</button><button data-s="0.5">0.5×</button>
          <button data-s="1">1×</button><button data-s="2">2×</button><button data-s="4">4×</button>
        </div>
      </details>

      <details open><summary>Tempo</summary>
        <div class="cp-row"><span id="cp-bpm-read">— BPM</span><label><input id="cp-auto" type="checkbox" checked> auto</label></div>
        <div class="cp-row"><label>BPM <input id="cp-bpm" type="number" min="40" max="300" step="1" value="120"></label><button id="cp-tap">Tap</button></div>
        <div class="cp-row"><button id="cp-half">½×</button><button id="cp-double">2×</button><button id="cp-phase">Reset phase</button></div>
        <div class="cp-row"><label>subdiv <select id="cp-subdiv"><option value="1">1/1</option><option value="2">1/2</option><option value="4">1/4</option><option value="8">1/8</option></select></label></div>
      </details>

      <details><summary>LFOs</summary>
        <div id="cp-lfos"></div>
      </details>

      <details><summary>Overlay (selected layer)</summary>
        <div class="cp-row">
          <select id="ov-mode">${OVERLAY_MODES.map((m, i) => `<option value="${i}">${m}</option>`).join('')}</select>
          <select id="ov-chan"><option value="0">L</option><option value="1">R</option><option value="2">mono</option></select></div>
        <div class="cp-row"><label>color <input id="ov-color" type="color" value="#33f0a0"></label><label>opacity <input id="ov-op" type="range" min="0" max="1" step="0.01" value="0.9"></label></div>
        <div class="cp-row"><label>scale <input id="ov-scale" type="range" min="0" max="1" step="0.01" value="0.35"></label><label>thick <input id="ov-thick" type="range" min="0.001" max="0.05" step="0.001" value="0.006"></label></div>
        <div class="cp-row"><label>rot <input id="ov-rot" type="range" min="0" max="6.283" step="0.01" value="0"></label><label>points <input id="ov-points" type="range" min="16" max="256" step="1" value="128"></label></div>
        <div class="cp-row"><label><input id="ov-log" type="checkbox"> log freq</label></div>
      </details>

      <details open><summary>Shader controls</summary>
        <div id="cp-user"><span class="cp-hint">Compile a shader with <code>// @control</code> lines.</span></div>
      </details>`;

    this.userHost = host.querySelector('#cp-user')!;
    this.wireTime();
    this.wireTempo();
    this.wireLfos();
    this.wireOverlay();
    this.load();
  }

  private q<T extends HTMLElement>(sel: string): T {
    return this.root.querySelector(sel) as T;
  }

  // --- Time ---------------------------------------------------------------
  private wireTime(): void {
    const play = this.q<HTMLButtonElement>('#cp-play');
    play.addEventListener('click', () => {
      this.paused = !this.paused;
      set_paused(this.paused);
      play.textContent = this.paused ? 'Play' : 'Pause';
    });
    this.q('#cp-reset').addEventListener('click', () => reset_time());
    const speed = this.q<HTMLInputElement>('#cp-speed');
    const speedV = this.q('#cp-speed-v');
    const applySpeed = () => {
      const v = Number(speed.value);
      set_time_scale(v);
      speedV.textContent = `${v.toFixed(2)}×`;
      this.save();
    };
    speed.addEventListener('input', applySpeed);
    this.root.querySelectorAll<HTMLButtonElement>('.cp-presets button').forEach((b) =>
      b.addEventListener('click', () => {
        speed.value = b.dataset.s!;
        applySpeed();
      }),
    );
  }

  // --- Tempo --------------------------------------------------------------
  private wireTempo(): void {
    const auto = this.q<HTMLInputElement>('#cp-auto');
    const bpm = this.q<HTMLInputElement>('#cp-bpm');
    auto.addEventListener('change', () => {
      tempo_set_manual(!auto.checked);
      this.save();
    });
    bpm.addEventListener('change', () => {
      tempo_set_bpm(Number(bpm.value));
      auto.checked = false;
      this.save();
    });
    this.q('#cp-tap').addEventListener('click', () => {
      tempo_tap();
      auto.checked = false;
    });
    this.q('#cp-half').addEventListener('click', () => (tempo_half(), (auto.checked = false)));
    this.q('#cp-double').addEventListener('click', () => (tempo_double(), (auto.checked = false)));
    this.q('#cp-phase').addEventListener('click', () => tempo_reset_phase());
    this.q<HTMLSelectElement>('#cp-subdiv').addEventListener('change', (e) => {
      tempo_set_subdivision(Number((e.target as HTMLSelectElement).value));
      this.save();
    });
  }

  /** Called from the diagnostics tick to show detected BPM. */
  showBpm(bpm: number, manual: boolean): void {
    this.q('#cp-bpm-read').textContent = `${bpm.toFixed(0)} BPM ${manual ? '(manual)' : '(auto)'}`;
  }

  // --- LFOs ---------------------------------------------------------------
  private wireLfos(): void {
    const host = this.q('#cp-lfos');
    for (let i = 0; i < 2; i++) {
      const row = el(`<div class="cp-lfo">
        <b>LFO${i}</b>
        <select class="w"><option value="0">sine</option><option value="1">tri</option><option value="2">saw</option><option value="3">sqr</option></select>
        <label>rate <input class="r" type="range" min="0.05" max="8" step="0.05" value="1"></label>
        <label><input class="s" type="checkbox"> sync</label>
        <label>mult <input class="m" type="range" min="0.125" max="4" step="0.125" value="1"></label>
      </div>`);
      const apply = () => {
        set_lfo(
          i,
          Number((row.querySelector('.w') as HTMLSelectElement).value),
          Number((row.querySelector('.r') as HTMLInputElement).value),
          (row.querySelector('.s') as HTMLInputElement).checked,
          Number((row.querySelector('.m') as HTMLInputElement).value),
        );
        this.save();
      };
      row.querySelectorAll('select,input').forEach((c) => c.addEventListener('input', apply));
      host.appendChild(row);
    }
  }

  // --- Overlay ------------------------------------------------------------
  private wireOverlay(): void {
    const ids = ['#ov-mode', '#ov-chan', '#ov-color', '#ov-op', '#ov-scale', '#ov-thick', '#ov-rot', '#ov-points', '#ov-log'];
    const apply = () => {
      const hex = this.q<HTMLInputElement>('#ov-color').value;
      const r = parseInt(hex.slice(1, 3), 16) / 255;
      const g = parseInt(hex.slice(3, 5), 16) / 255;
      const b = parseInt(hex.slice(5, 7), 16) / 255;
      set_overlay(
        Number(this.q<HTMLSelectElement>('#ov-mode').value),
        Number(this.q<HTMLSelectElement>('#ov-chan').value),
        r, g, b,
        Number(this.q<HTMLInputElement>('#ov-op').value),
        Number(this.q<HTMLInputElement>('#ov-scale').value),
        Number(this.q<HTMLInputElement>('#ov-thick').value),
        Number(this.q<HTMLInputElement>('#ov-rot').value),
        Number(this.q<HTMLInputElement>('#ov-points').value),
        this.q<HTMLInputElement>('#ov-log').checked,
      );
      this.save();
    };
    ids.forEach((id) => this.q(id).addEventListener('input', apply));
  }

  // --- User shader controls (dynamic) ------------------------------------
  buildUserControls(controls: ShaderControl[]): void {
    this.userHost.innerHTML = '';
    if (controls.length === 0) {
      this.userHost.appendChild(el('<span class="cp-hint">No <code>// @control</code> declarations in this shader.</span>'));
      return;
    }
    for (const c of controls) {
      this.userHost.appendChild(this.buildOneControl(c));
    }
  }

  private buildOneControl(c: ShaderControl): HTMLElement {
    const row = el(`<div class="cp-ctl"><div class="cp-ctl-head"><b>${c.name}</b><span class="cp-kind">${c.kind}</span></div></div>`);
    const setSlot = (x: number, y = 0, z = 0, w = 0) => set_control(c.slot, x, y, z, w);

    if (c.kind === 'float' || c.kind === 'int' || c.kind === 'enum') {
      const step = c.kind === 'float' ? (c.max - c.min) / 200 || 0.01 : 1;
      const input = el(`<input type="range" min="${c.min}" max="${c.max}" step="${step}" value="${c.default[0]}">`) as HTMLInputElement;
      input.addEventListener('input', () => setSlot(Number(input.value)));
      row.appendChild(input);
      row.appendChild(this.buildModRow(c));
    } else if (c.kind === 'bool') {
      const input = el(`<input type="checkbox" ${c.default[0] > 0.5 ? 'checked' : ''}>`) as HTMLInputElement;
      input.addEventListener('change', () => setSlot(input.checked ? 1 : 0));
      row.appendChild(input);
    } else if (c.kind === 'color') {
      const hex = `#${[0, 1, 2].map((i) => Math.round(c.default[i] * 255).toString(16).padStart(2, '0')).join('')}`;
      const input = el(`<input type="color" value="${hex}">`) as HTMLInputElement;
      input.addEventListener('input', () => {
        const v = input.value;
        setSlot(parseInt(v.slice(1, 3), 16) / 255, parseInt(v.slice(3, 5), 16) / 255, parseInt(v.slice(5, 7), 16) / 255, 1);
      });
      row.appendChild(input);
    } else if (c.kind === 'vec2') {
      const mk = (idx: number) => {
        const inp = el(`<input type="range" min="${c.min}" max="${c.max}" step="0.01" value="${c.default[idx]}">`) as HTMLInputElement;
        inp.addEventListener('input', () => {
          const xs = row.querySelectorAll<HTMLInputElement>('input[type=range]');
          setSlot(Number(xs[0].value), Number(xs[1].value));
        });
        return inp;
      };
      row.appendChild(mk(0));
      row.appendChild(mk(1));
    } else if (c.kind === 'trigger') {
      const btn = el(`<button>${c.name}</button>`) as HTMLButtonElement;
      btn.addEventListener('pointerdown', () => setSlot(1));
      btn.addEventListener('pointerup', () => setSlot(0));
      row.appendChild(btn);
    }
    return row;
  }

  private buildModRow(c: ShaderControl): HTMLElement {
    const row = el(`<div class="cp-mod">
      <select class="src">${MOD_SOURCES.map((s) => `<option value="${s}">${s}</option>`).join('')}</select>
      <label>amt <input class="amt" type="range" min="-2" max="2" step="0.01" value="0"></label>
      <label>smooth <input class="sm" type="range" min="0" max="0.98" step="0.01" value="0"></label>
    </div>`);
    const apply = () => {
      set_control_mod(
        c.slot,
        (row.querySelector('.src') as HTMLSelectElement).value,
        Number((row.querySelector('.amt') as HTMLInputElement).value),
        Number((row.querySelector('.sm') as HTMLInputElement).value),
        'linear',
        false,
      );
    };
    row.querySelectorAll('select,input').forEach((c2) => c2.addEventListener('input', apply));
    return row;
  }

  // --- Persistence --------------------------------------------------------
  private save(): void {
    try {
      const data = {
        v: 1,
        speed: this.q<HTMLInputElement>('#cp-speed').value,
        auto: this.q<HTMLInputElement>('#cp-auto').checked,
        bpm: this.q<HTMLInputElement>('#cp-bpm').value,
        subdiv: this.q<HTMLSelectElement>('#cp-subdiv').value,
        overlay: {
          mode: this.q<HTMLSelectElement>('#ov-mode').value,
          chan: this.q<HTMLSelectElement>('#ov-chan').value,
          color: this.q<HTMLInputElement>('#ov-color').value,
          op: this.q<HTMLInputElement>('#ov-op').value,
          scale: this.q<HTMLInputElement>('#ov-scale').value,
          thick: this.q<HTMLInputElement>('#ov-thick').value,
        },
      };
      localStorage.setItem(STORE_KEY, JSON.stringify(data));
    } catch {
      /* storage unavailable */
    }
  }

  private load(): void {
    let data: Record<string, unknown> | null = null;
    try {
      const raw = localStorage.getItem(STORE_KEY);
      if (raw) data = JSON.parse(raw);
    } catch {
      data = null;
    }
    if (!data || data.v !== 1) return; // unknown/missing schema → defaults
    const set = (sel: string, val: unknown) => {
      const e = this.root.querySelector(sel) as HTMLInputElement | null;
      if (e && val != null) {
        if (e.type === 'checkbox') e.checked = Boolean(val);
        else e.value = String(val);
      }
    };
    set('#cp-speed', data.speed);
    set('#cp-auto', data.auto);
    set('#cp-bpm', data.bpm);
    set('#cp-subdiv', data.subdiv);
    const ov = data.overlay as Record<string, unknown> | undefined;
    if (ov) {
      set('#ov-mode', ov.mode);
      set('#ov-chan', ov.chan);
      set('#ov-color', ov.color);
      set('#ov-op', ov.op);
      set('#ov-scale', ov.scale);
      set('#ov-thick', ov.thick);
    }
    // Push restored values to the engine.
    this.q<HTMLInputElement>('#cp-speed').dispatchEvent(new Event('input'));
  }
}
