// Phase 10A.3 — minimal Library management panel.
//
// Intentionally small: validates the shader/scene save/load/rename/duplicate/
// delete/favorite flows without pre-empting the full Phase 10A.4 virtualized
// browser. Built from the same panel/button patterns as the rest of the UI so it
// can be superseded cleanly. Built-in entries are read-only (no destructive
// delete). All operations are local.

import type { ContentLibrary, ImportResult, LibraryItem } from './library';

export class LibraryPanel {
  private note: HTMLElement;
  private builtinsEl: HTMLElement;
  private userEl: HTMLElement;

  constructor(
    private readonly host: HTMLElement,
    private readonly content: ContentLibrary,
    private readonly onStatus: (msg: string) => void = () => {},
  ) {
    host.innerHTML = `
      <div class="lib-actions">
        <button id="lib-save-shader" title="Save the current shader project">Save Shader</button>
        <button id="lib-save-scene" title="Save the current scene">Save Scene</button>
        <button id="lib-refresh" title="Refresh">&#8635;</button>
      </div>
      <div class="lib-note" id="lib-note"></div>
      <div class="lib-h">Built-in shaders</div>
      <div id="lib-builtins" class="lib-list"></div>
      <div class="lib-h">My library</div>
      <div id="lib-user" class="lib-list"></div>`;
    this.note = host.querySelector('#lib-note')!;
    this.builtinsEl = host.querySelector('#lib-builtins')!;
    this.userEl = host.querySelector('#lib-user')!;

    host.querySelector('#lib-save-shader')!.addEventListener('click', () => this.saveShader());
    host.querySelector('#lib-save-scene')!.addEventListener('click', () => this.saveScene());
    host.querySelector('#lib-refresh')!.addEventListener('click', () => void this.refresh());
    void this.refresh();
  }

  private say(msg: string): void {
    this.note.textContent = msg;
    this.onStatus(msg);
  }

  private async saveShader(): Promise<void> {
    const name = prompt('Save shader as:');
    if (!name) return;
    const item = await this.content.saveCurrentShader(name);
    this.say(item ? `Saved shader "${name}"` : 'No shader layer to save — add a Shader layer first.');
    await this.refresh();
  }

  private async saveScene(): Promise<void> {
    const name = prompt('Save scene as:');
    if (!name) return;
    const item = await this.content.saveCurrentScene(name);
    this.say(item ? `Saved scene "${name}"` : 'Could not read the current scene.');
    await this.refresh();
  }

  private reportLoad(kind: string, res: ImportResult): void {
    this.say(res.ok ? `Loaded ${kind}` : `${kind} load failed (kept current): ${res.error ?? ''}`);
  }

  private row(item: LibraryItem, builtin: boolean): HTMLElement {
    const el = document.createElement('div');
    el.className = 'lib-row';
    const fav = item.favorite ? '★' : '☆';
    el.innerHTML = `<span class="lib-name" title="${item.type}${item.author ? ' · ' + item.author : ''}">${escapeHtml(item.name)}</span>
      <span class="lib-badge">${item.type}</span>
      <button class="lib-load">Load</button>
      <button class="lib-fav" title="Favorite">${fav}</button>` +
      (builtin
        ? '<button class="lib-dup" title="Duplicate to My library">Dup</button>'
        : '<button class="lib-ren" title="Rename">Ren</button><button class="lib-del" title="Delete">Del</button>');
    el.querySelector('.lib-load')!.addEventListener('click', async () => {
      const res = item.type === 'scene' ? await this.content.loadScene(item.id) : await this.content.loadShader(item.id);
      this.reportLoad(item.type, res);
    });
    el.querySelector('.lib-fav')!.addEventListener('click', async () => {
      await this.content.setFavorite(item.id, !item.favorite);
      await this.refresh();
    });
    el.querySelector('.lib-dup')?.addEventListener('click', async () => {
      await this.content.duplicate(item.id);
      this.say(`Duplicated "${item.name}"`);
      await this.refresh();
    });
    el.querySelector('.lib-ren')?.addEventListener('click', async () => {
      const n = prompt('Rename to:', item.name);
      if (n) {
        await this.content.rename(item.id, n);
        await this.refresh();
      }
    });
    el.querySelector('.lib-del')?.addEventListener('click', async () => {
      await this.content.delete(item.id);
      await this.refresh();
    });
    return el;
  }

  async refresh(): Promise<void> {
    this.builtinsEl.replaceChildren(...this.content.listBuiltinShaders().map((i) => this.row(i, true)));
    const [shaders, scenes] = await Promise.all([this.content.listByType('shader'), this.content.listByType('scene')]);
    const users = [...shaders, ...scenes].filter((i) => i.origin !== 'builtin');
    users.sort((a, b) => (b.lastUsed ?? b.dateAdded) - (a.lastUsed ?? a.dateAdded));
    this.userEl.replaceChildren(
      ...(users.length ? users.map((i) => this.row(i, false)) : [textDiv('Save a shader or scene to start your library.')]),
    );
  }
}

function textDiv(t: string): HTMLElement {
  const d = document.createElement('div');
  d.className = 'lib-empty';
  d.textContent = t;
  return d;
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
}
