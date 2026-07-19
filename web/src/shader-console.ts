// Live GLSL shader console with multipass support (Phase 8d). A CodeMirror 6
// editor edits one pass at a time (Image + Buffer A–D); compilation is
// synchronous in Rust (naga validates, then swaps that pass's pipeline), so a
// failed compile just returns diagnostics and the pass's last-known-good keeps
// rendering. Per-pass source is cached client-side so switching tabs never
// loses uncompiled edits. `iChannel0–3` sources are configured per pass.

import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter } from '@codemirror/view';
import { EditorState } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, indentOnInput, syntaxHighlighting, defaultHighlightStyle } from '@codemirror/language';
import { search, searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { lintGutter, setDiagnostics, type Diagnostic as CmDiagnostic } from '@codemirror/lint';
import { cpp } from '@codemirror/lang-cpp';
import { oneDark } from '@codemirror/theme-one-dark';
import {
  set_pass_source,
  add_buffer_pass,
  remove_buffer_pass,
  set_pass_channel,
  reset_shader_buffers,
  project_json,
} from './pm_web/pm_web.js';
import { EXAMPLES } from './examples';
import { MULTIPASS_EXAMPLES } from './multipass-examples';

interface RustDiagnostic {
  line: number;
  column: number;
  message: string;
}
interface ShaderControl {
  name: string;
  kind: string;
  min: number;
  max: number;
  slot: number;
  default: number[];
  options: string[];
}
interface CompileReport {
  ok: boolean;
  compileMs: number;
  diagnostics: RustDiagnostic[];
  controls: ShaderControl[];
}
interface PassInfo {
  type: string;
  index: number;
  enabled: boolean;
  mode: number;
  compiled: boolean;
  source: string;
  channels: string[];
  diagnostics: RustDiagnostic[];
}
interface Project {
  passes: PassInfo[];
  conflicts: string[];
}

type Mode = 'shadertoy' | 'raw';

const IMAGE = 4;
const PASS_LABEL = ['Buffer A', 'Buffer B', 'Buffer C', 'Buffer D', 'Image'];
const CHANNEL_OPTS: Array<[string, string]> = [
  ['none', 'None'],
  ['audio', 'Audio'],
  ['buffera', 'Buffer A'],
  ['bufferb', 'Buffer B'],
  ['bufferc', 'Buffer C'],
  ['bufferd', 'Buffer D'],
  ['self', 'Self (prev)'],
];

export class ShaderConsole {
  private view: EditorView;
  private mode: Mode = 'shadertoy';
  private auto = false;
  private autoTimer: number | undefined;
  private hostEl: HTMLElement;
  private status: HTMLElement;
  private errorPanel: HTMLElement;

  /** Active pass being edited (0–3 = Buffer A–D, 4 = Image). */
  private activePass = IMAGE;
  /** Client-side per-pass source cache (preserves uncompiled edits). */
  private cache = new Map<number, string>();

  /** Called after a compile with the project's merged controls. */
  onControls: ((controls: ShaderControl[]) => void) | null = null;

  constructor(host: HTMLElement) {
    this.hostEl = host;
    host.innerHTML = `
      <div class="sc-bar">
        <select id="sc-example" title="Single-pass example"></select>
        <select id="sc-multi" title="Multipass example"><option value="">Multipass…</option></select>
        <select id="sc-mode" title="Shader dialect">
          <option value="shadertoy">Shadertoy</option>
          <option value="raw">Raw GLSL</option>
        </select>
        <button id="sc-compile" title="Compile this pass (Ctrl/Cmd+Enter)">Compile ▸</button>
        <button id="sc-compile-all" title="Compile every pass">All</button>
        <button id="sc-reset-buffers" title="Clear buffer history (feedback restart)">Reset buffers</button>
        <label class="sc-auto"><input id="sc-auto" type="checkbox" /> auto</label>
        <span id="sc-status">ready</span>
      </div>
      <div class="sc-tabs" id="sc-tabs"></div>
      <div class="sc-channels" id="sc-channels"></div>
      <div id="sc-editor"></div>
      <pre id="sc-errors"></pre>`;

    this.status = host.querySelector('#sc-status')!;
    this.errorPanel = host.querySelector('#sc-errors')!;

    const exampleSel = host.querySelector('#sc-example') as HTMLSelectElement;
    EXAMPLES.forEach((ex, i) => {
      const o = document.createElement('option');
      o.value = String(i);
      o.textContent = ex.name;
      exampleSel.appendChild(o);
    });
    const multiSel = host.querySelector('#sc-multi') as HTMLSelectElement;
    MULTIPASS_EXAMPLES.forEach((ex, i) => {
      const o = document.createElement('option');
      o.value = String(i);
      o.textContent = ex.name;
      multiSel.appendChild(o);
    });

    this.view = new EditorView({
      parent: host.querySelector('#sc-editor')!,
      state: EditorState.create({
        doc: EXAMPLES[0].source,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          history(),
          bracketMatching(),
          indentOnInput(),
          highlightSelectionMatches(),
          search({ top: true }),
          syntaxHighlighting(defaultHighlightStyle),
          cpp(),
          oneDark,
          lintGutter(),
          keymap.of([
            { key: 'Mod-Enter', preventDefault: true, run: () => (this.compile(), true) },
            indentWithTab,
            ...defaultKeymap,
            ...historyKeymap,
            ...searchKeymap,
          ]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) this.onDocChanged();
          }),
          EditorView.theme({ '&': { height: '100%' }, '.cm-scroller': { overflow: 'auto' } }),
        ],
      }),
    });

    (host.querySelector('#sc-compile') as HTMLButtonElement).addEventListener('click', () => this.compile());
    (host.querySelector('#sc-compile-all') as HTMLButtonElement).addEventListener('click', () => this.compileAll());
    (host.querySelector('#sc-reset-buffers') as HTMLButtonElement).addEventListener('click', () => reset_shader_buffers());
    (host.querySelector('#sc-mode') as HTMLSelectElement).addEventListener('change', (e) => {
      this.mode = (e.target as HTMLSelectElement).value as Mode;
      this.compile();
    });
    (host.querySelector('#sc-auto') as HTMLInputElement).addEventListener('change', (e) => {
      this.auto = (e.target as HTMLInputElement).checked;
    });
    exampleSel.addEventListener('change', (e) => {
      const ex = EXAMPLES[Number((e.target as HTMLSelectElement).value)];
      (host.querySelector('#sc-mode') as HTMLSelectElement).value = ex.mode;
      this.mode = ex.mode;
      // A single-pass example targets the Image pass.
      this.activePass = IMAGE;
      this.setSource(ex.source);
      this.cache.set(IMAGE, ex.source);
      this.compile();
    });
    multiSel.addEventListener('change', (e) => {
      const idx = Number((e.target as HTMLSelectElement).value);
      if (!Number.isNaN(idx) && (e.target as HTMLSelectElement).value !== '') this.applyMultipass(idx);
      (e.target as HTMLSelectElement).value = '';
    });
  }

  private project(): Project {
    try {
      return JSON.parse(project_json());
    } catch {
      return { passes: [], conflicts: [] };
    }
  }

  private setSource(src: string): void {
    this.view.dispatch({ changes: { from: 0, to: this.view.state.doc.length, insert: src } });
  }

  /** A shader layer was selected: reset the per-pass cache and load its project
   *  into the tabs (no compile — its last-known-good passes are already live). */
  loadLayer(_source: string, mode: number): void {
    this.mode = mode === 1 ? 'raw' : 'shadertoy';
    (this.hostEl.querySelector('#sc-mode') as HTMLSelectElement).value = this.mode;
    this.cache.clear();
    this.activePass = IMAGE;
    const proj = this.project();
    for (const p of proj.passes) this.cache.set(p.index, p.source);
    this.setSource(this.cache.get(IMAGE) ?? '');
    this.renderTabs(proj);
    this.renderChannels(proj);
  }

  private onDocChanged(): void {
    this.cache.set(this.activePass, this.view.state.doc.toString());
    if (!this.auto) return;
    window.clearTimeout(this.autoTimer);
    this.autoTimer = window.setTimeout(() => this.compile(), 600);
  }

  // --- Passes -------------------------------------------------------------

  private renderTabs(proj: Project): void {
    const tabs = this.hostEl.querySelector('#sc-tabs')!;
    tabs.innerHTML = '';
    const active = new Set(proj.passes.map((p) => p.index));
    // Buffer A–D + Image, in execution order.
    for (let i = 0; i <= IMAGE; i++) {
      if (!active.has(i)) continue;
      const info = proj.passes.find((p) => p.index === i)!;
      const btn = document.createElement('button');
      btn.className = 'sc-tab' + (i === this.activePass ? ' on' : '') + (info.compiled ? '' : ' err');
      btn.textContent = PASS_LABEL[i];
      btn.addEventListener('click', () => this.selectPass(i));
      tabs.appendChild(btn);
      if (i < IMAGE) {
        const rm = document.createElement('button');
        rm.className = 'sc-tab-rm';
        rm.textContent = '×';
        rm.title = `Remove ${PASS_LABEL[i]}`;
        rm.addEventListener('click', (e) => {
          e.stopPropagation();
          remove_buffer_pass(i);
          if (this.activePass === i) this.activePass = IMAGE;
          this.cache.delete(i);
          this.afterStructureChange();
        });
        tabs.appendChild(rm);
      }
    }
    // Add-buffer buttons for missing buffers.
    for (let i = 0; i < IMAGE; i++) {
      if (active.has(i)) continue;
      const add = document.createElement('button');
      add.className = 'sc-tab-add';
      add.textContent = `+ ${PASS_LABEL[i][7]}`;
      add.title = `Add ${PASS_LABEL[i]}`;
      add.addEventListener('click', () => {
        add_buffer_pass(i);
        this.activePass = i;
        this.afterStructureChange();
      });
      tabs.appendChild(add);
    }
    if (proj.conflicts.length) {
      const warn = document.createElement('span');
      warn.className = 'sc-conflict';
      warn.textContent = '⚠ ' + proj.conflicts.join('; ');
      tabs.appendChild(warn);
    }
  }

  private renderChannels(proj: Project): void {
    const host = this.hostEl.querySelector('#sc-channels')!;
    host.innerHTML = '';
    const info = proj.passes.find((p) => p.index === this.activePass);
    if (!info) return;
    for (let c = 0; c < 4; c++) {
      const wrap = document.createElement('label');
      wrap.className = 'sc-chan';
      const cur = info.channels[c] ?? 'none';
      wrap.innerHTML = `iChannel${c} <select>${CHANNEL_OPTS.map(([v, l]) => `<option value="${v}" ${v === cur ? 'selected' : ''}>${l}</option>`).join('')}</select>`;
      (wrap.querySelector('select') as HTMLSelectElement).addEventListener('change', (e) => {
        set_pass_channel(this.activePass, c, (e.target as HTMLSelectElement).value);
      });
      host.appendChild(wrap);
    }
  }

  private selectPass(index: number): void {
    this.cache.set(this.activePass, this.view.state.doc.toString());
    this.activePass = index;
    this.setSource(this.cache.get(index) ?? '');
    const proj = this.project();
    this.renderTabs(proj);
    this.renderChannels(proj);
    const info = proj.passes.find((p) => p.index === index);
    this.showDiagnostics(info?.diagnostics ?? []);
  }

  /** Refresh tabs/channels after adding/removing a pass; load the active pass. */
  private afterStructureChange(): void {
    const proj = this.project();
    for (const p of proj.passes) if (!this.cache.has(p.index)) this.cache.set(p.index, p.source);
    this.setSource(this.cache.get(this.activePass) ?? '');
    this.renderTabs(proj);
    this.renderChannels(proj);
  }

  // --- Compilation --------------------------------------------------------

  compile(): void {
    const src = this.view.state.doc.toString();
    this.cache.set(this.activePass, src);
    let report: CompileReport;
    try {
      report = JSON.parse(set_pass_source(this.activePass, this.mode === 'raw' ? 1 : 0, src));
    } catch (e) {
      this.setStatus(`compile call failed: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    this.showDiagnostics(report.diagnostics);
    if (report.ok) {
      this.setStatus(`✓ ${PASS_LABEL[this.activePass]} compiled in ${report.compileMs.toFixed(1)} ms`);
      this.onControls?.(report.controls ?? []);
    } else {
      this.setStatus(`✗ ${PASS_LABEL[this.activePass]}: ${report.diagnostics.length} error(s) — previous kept`);
    }
    this.renderTabs(this.project());
  }

  private compileAll(): void {
    // Persist the active editor first, then compile every cached pass.
    this.cache.set(this.activePass, this.view.state.doc.toString());
    let lastControls: ShaderControl[] = [];
    for (const [index, source] of this.cache) {
      try {
        const r: CompileReport = JSON.parse(set_pass_source(index, this.mode === 'raw' ? 1 : 0, source));
        if (r.ok && index === IMAGE) lastControls = r.controls;
        else if (r.ok) lastControls = r.controls;
      } catch {
        /* ignore */
      }
    }
    this.onControls?.(lastControls);
    this.setStatus('✓ compiled all passes');
    this.renderTabs(this.project());
  }

  private applyMultipass(exIndex: number): void {
    const ex = MULTIPASS_EXAMPLES[exIndex];
    // Reset to a clean project: drop any existing buffers.
    for (let i = 0; i < IMAGE; i++) remove_buffer_pass(i);
    this.cache.clear();
    this.mode = 'shadertoy';
    (this.hostEl.querySelector('#sc-mode') as HTMLSelectElement).value = 'shadertoy';
    // Add buffers, set sources + channels, compile.
    let controls: ShaderControl[] = [];
    for (const p of ex.passes) {
      const idx = ['buffera', 'bufferb', 'bufferc', 'bufferd', 'image'].indexOf(p.type);
      if (idx < 0) continue;
      if (idx < IMAGE) add_buffer_pass(idx);
      this.cache.set(idx, p.source);
      try {
        const r: CompileReport = JSON.parse(set_pass_source(idx, 0, p.source));
        if (r.ok) controls = r.controls;
      } catch {
        /* ignore */
      }
      for (let c = 0; c < 4; c++) set_pass_channel(idx, c, p.channels[c]);
    }
    reset_shader_buffers();
    this.activePass = IMAGE;
    this.setSource(this.cache.get(IMAGE) ?? '');
    this.onControls?.(controls);
    this.afterStructureChange();
    this.setStatus(`✓ loaded "${ex.name}"`);
  }

  private showDiagnostics(diags: RustDiagnostic[]): void {
    const cmDiags: CmDiagnostic[] = diags.map((d) => this.toCmDiagnostic(d));
    this.view.dispatch(setDiagnostics(this.view.state, cmDiags));
    this.errorPanel.textContent = diags.map((d) => `${PASS_LABEL[this.activePass]} — line ${d.line}:${d.column}  ${d.message}`).join('\n');
  }

  private toCmDiagnostic(d: RustDiagnostic): CmDiagnostic {
    const doc = this.view.state.doc;
    const lineNo = Math.min(Math.max(1, d.line || 1), doc.lines);
    const line = doc.line(lineNo);
    const from = Math.min(line.from + Math.max(0, (d.column || 1) - 1), line.to);
    return { from, to: line.to, severity: 'error', message: d.message };
  }

  private setStatus(msg: string): void {
    this.status.textContent = msg;
  }
}
