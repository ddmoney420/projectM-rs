// Live GLSL shader console: a CodeMirror 6 editor (minimal extension set) plus
// the compile pipeline. Compilation is synchronous in Rust (naga validates,
// then swaps the pipeline), so a failed compile just returns diagnostics and the
// last-known-good shader keeps rendering — never a black flash, never a stall.

import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter } from '@codemirror/view';
import { EditorState } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, indentOnInput, syntaxHighlighting, defaultHighlightStyle } from '@codemirror/language';
import { search, searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { lintGutter, setDiagnostics, type Diagnostic as CmDiagnostic } from '@codemirror/lint';
import { cpp } from '@codemirror/lang-cpp';
import { oneDark } from '@codemirror/theme-one-dark';
import { set_shader_source, set_render_source } from './pm_web/pm_web.js';
import { EXAMPLES } from './examples';

interface RustDiagnostic {
  line: number;
  column: number;
  message: string;
}
interface CompileReport {
  ok: boolean;
  compileMs: number;
  diagnostics: RustDiagnostic[];
}

type Mode = 'shadertoy' | 'raw';

export class ShaderConsole {
  private view: EditorView;
  private mode: Mode = 'shadertoy';
  private auto = false;
  private autoTimer: number | undefined;
  private switchedOnce = false;

  private status: HTMLElement;
  private errorPanel: HTMLElement;

  /** Called after a successful compile switches the render source to the shader. */
  onRenderSource: ((s: 'preset' | 'shader') => void) | null = null;

  constructor(host: HTMLElement) {
    host.innerHTML = `
      <div class="sc-bar">
        <select id="sc-example" title="Load an example"></select>
        <select id="sc-mode" title="Shader dialect">
          <option value="shadertoy">Shadertoy</option>
          <option value="raw">Raw GLSL</option>
        </select>
        <button id="sc-compile" title="Compile (Ctrl/Cmd+Enter)">Compile ▸</button>
        <label class="sc-auto"><input id="sc-auto" type="checkbox" /> auto</label>
        <span id="sc-status">ready</span>
      </div>
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
    (host.querySelector('#sc-mode') as HTMLSelectElement).addEventListener('change', (e) => {
      this.mode = (e.target as HTMLSelectElement).value as Mode;
      this.compile();
    });
    (host.querySelector('#sc-auto') as HTMLInputElement).addEventListener('change', (e) => {
      this.auto = (e.target as HTMLInputElement).checked;
    });
    exampleSel.addEventListener('change', (e) => {
      const ex = EXAMPLES[Number((e.target as HTMLSelectElement).value)];
      const modeSel = host.querySelector('#sc-mode') as HTMLSelectElement;
      modeSel.value = ex.mode;
      this.mode = ex.mode;
      this.setSource(ex.source);
      this.compile();
    });
  }

  /** Replace the entire editor contents. */
  private setSource(src: string): void {
    this.view.dispatch({ changes: { from: 0, to: this.view.state.doc.length, insert: src } });
  }

  private onDocChanged(): void {
    if (!this.auto) return;
    // Debounce so we never compile mid-keystroke.
    window.clearTimeout(this.autoTimer);
    this.autoTimer = window.setTimeout(() => this.compile(), 600);
  }

  compile(): void {
    const src = this.view.state.doc.toString();
    let report: CompileReport;
    try {
      report = JSON.parse(set_shader_source(this.mode === 'raw' ? 1 : 0, src));
    } catch (e) {
      this.setStatus(`compile call failed: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }

    const cmDiags: CmDiagnostic[] = report.diagnostics.map((d) => this.toCmDiagnostic(d));
    this.view.dispatch(setDiagnostics(this.view.state, cmDiags));

    if (report.ok) {
      this.setStatus(`✓ compiled in ${report.compileMs.toFixed(1)} ms`);
      this.errorPanel.textContent = '';
      // First successful compile switches the canvas to the shader.
      if (!this.switchedOnce) {
        set_render_source(1);
        this.switchedOnce = true;
        this.onRenderSource?.('shader');
      }
    } else {
      this.setStatus(`✗ ${report.diagnostics.length} error(s) — previous shader kept`);
      this.errorPanel.textContent = report.diagnostics
        .map((d) => `line ${d.line}:${d.column}  ${d.message}`)
        .join('\n');
    }
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
