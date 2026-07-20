// Phase 10A.4 — unified, virtualized performance library browser.
//
// One predictable query pipeline over a lightweight in-memory aggregate of ALL
// content (built-in shaders + Milkdrop pack index + user/imported/saved items).
// Browse never loads a heavy payload (ShaderProject / SceneData / .milk text) —
// only metadata. The result window is virtualized so DOM node count stays
// bounded even with 10k+ Milkdrop entries. "Load" applies to the active visual
// via the existing safe/transactional paths (true non-destructive preview is a
// later phase and is deliberately NOT built here).

import type { Collection, ContentType, LibraryItem } from './library';

export interface ImportResult {
  ok: boolean;
  error?: string;
}

export interface BrowserDeps {
  collect(): Promise<LibraryItem[]>;
  load(item: LibraryItem): Promise<ImportResult>;
  setFavorite(id: string, fav: boolean): Promise<void>;
  rename(id: string, name: string): Promise<LibraryItem | null>;
  duplicate(id: string): Promise<LibraryItem | null>;
  remove(id: string): Promise<boolean>;
  collections(): Promise<Collection[]>;
  createCollection(name: string): Promise<Collection>;
  renameCollection(id: string, name: string): Promise<void>;
  deleteCollection(id: string): Promise<void>;
  addToCollection(id: string, colId: string): Promise<void>;
  removeFromCollection(id: string, colId: string): Promise<void>;
  importMilk(files: File[]): Promise<LibraryItem[]>;
  status(msg: string): void;
}

type View = 'all' | 'milkdrop' | 'shader' | 'scene' | 'favorites' | 'recent' | 'collections';

const ROW = 42; // px — fixed row height enables cheap virtualization
const BUFFER = 6; // rows rendered above/below the viewport
const RECENT_LIMIT = 100;

const TYPE_ICON: Record<ContentType, string> = { milkdrop: 'M', shader: 'Sh', scene: 'Sc' };
const TYPE_LABEL: Record<ContentType, string> = { milkdrop: 'Milkdrop', shader: 'Shader', scene: 'Scene' };

function esc(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
}

export class LibraryBrowser {
  private all: LibraryItem[] = [];
  private results: LibraryItem[] = [];
  private collectionsList: Collection[] = [];
  private view: View = 'all';
  private search = '';
  private activeCollection: string | null = null;
  private selected = -1;

  private scroll!: HTMLElement;
  private spacer!: HTMLElement;
  private windowEl!: HTMLElement;
  private tabsEl!: HTMLElement;
  private navEl!: HTMLElement;
  private collbar!: HTMLElement;
  private emptyEl!: HTMLElement;
  private detailsEl!: HTMLElement;
  private fileInput!: HTMLInputElement;

  constructor(
    private readonly host: HTMLElement,
    private readonly deps: BrowserDeps,
  ) {
    host.innerHTML = `
      <div class="br-top">
        <input id="br-search" class="br-search" type="search" placeholder="Search library…" aria-label="Search library" />
        <button id="br-import" title="Import local .milk files">Import .milk</button>
        <input id="br-file" type="file" accept=".milk" multiple hidden />
      </div>
      <div id="br-tabs" class="br-tabs" role="tablist"></div>
      <div id="br-collbar" class="br-collbar" hidden></div>
      <div id="br-nav" class="br-nav" hidden>
        <span class="br-navlabel">Milkdrop:</span>
        <button data-nav="prev" aria-label="Previous Milkdrop">◀ Prev</button>
        <button data-nav="random" aria-label="Random Milkdrop">Random</button>
        <button data-nav="next" aria-label="Next Milkdrop">Next ▶</button>
      </div>
      <div id="br-scroll" class="br-scroll" tabindex="0" role="listbox" aria-label="Library items">
        <div id="br-spacer" class="br-spacer"><div id="br-window" class="br-window"></div></div>
      </div>
      <div id="br-empty" class="br-empty" hidden></div>
      <div id="br-details" class="br-details" hidden></div>`;

    this.scroll = host.querySelector('#br-scroll')!;
    this.spacer = host.querySelector('#br-spacer')!;
    this.windowEl = host.querySelector('#br-window')!;
    this.tabsEl = host.querySelector('#br-tabs')!;
    this.navEl = host.querySelector('#br-nav')!;
    this.collbar = host.querySelector('#br-collbar')!;
    this.emptyEl = host.querySelector('#br-empty')!;
    this.detailsEl = host.querySelector('#br-details')!;
    this.fileInput = host.querySelector('#br-file')!;

    const VIEWS: [View, string][] = [
      ['all', 'All'], ['milkdrop', 'Milkdrop'], ['shader', 'Shaders'], ['scene', 'Scenes'],
      ['favorites', 'Favorites'], ['recent', 'Recent'], ['collections', 'Collections'],
    ];
    this.tabsEl.replaceChildren(...VIEWS.map(([v, label]) => {
      const b = document.createElement('button');
      b.className = 'br-tab';
      b.textContent = label;
      b.dataset.view = v;
      b.setAttribute('role', 'tab');
      b.addEventListener('click', () => this.setView(v));
      return b;
    }));

    const searchEl = host.querySelector('#br-search') as HTMLInputElement;
    searchEl.addEventListener('input', () => {
      this.search = searchEl.value.trim().toLowerCase();
      this.applyFilter();
    });
    host.querySelector('#br-import')!.addEventListener('click', () => this.fileInput.click());
    this.fileInput.addEventListener('change', () => void this.onImport());
    this.navEl.querySelectorAll('button').forEach((b) =>
      b.addEventListener('click', () => void this.navMilkdrop((b as HTMLElement).dataset.nav as 'prev' | 'next' | 'random')),
    );
    this.scroll.addEventListener('scroll', () => this.renderWindow());
    this.scroll.addEventListener('keydown', (e) => this.onKey(e));
    window.addEventListener('resize', () => this.renderWindow());
  }

  // --- data ---------------------------------------------------------------

  async refresh(): Promise<void> {
    [this.all, this.collectionsList] = await Promise.all([this.deps.collect(), this.deps.collections()]);
    this.applyFilter();
  }

  setView(v: View): void {
    this.view = v;
    this.activeCollection = v === 'collections' ? this.activeCollection ?? this.collectionsList[0]?.id ?? null : null;
    for (const t of this.tabsEl.querySelectorAll('.br-tab')) t.classList.toggle('on', (t as HTMLElement).dataset.view === v);
    this.navEl.hidden = !(v === 'all' || v === 'milkdrop' || v === 'favorites' || v === 'recent');
    this.collbar.hidden = v !== 'collections';
    if (v === 'collections') this.renderCollbar();
    this.applyFilter();
  }

  setSearch(s: string): void {
    this.search = s.trim().toLowerCase();
    (this.host.querySelector('#br-search') as HTMLInputElement).value = s;
    this.applyFilter();
  }

  private matches(it: LibraryItem): boolean {
    if (!this.search) return true;
    const hay = [it.name, it.author, (it.tags || []).join(' '), it.license, it.attribution?.attributionText, it.description]
      .filter(Boolean)
      .join(' ')
      .toLowerCase();
    return hay.includes(this.search);
  }

  private applyFilter(): void {
    let base = this.all;
    switch (this.view) {
      case 'milkdrop': case 'shader': case 'scene':
        base = base.filter((i) => i.type === this.view);
        break;
      case 'favorites':
        base = base.filter((i) => i.favorite);
        break;
      case 'recent':
        base = base.filter((i) => typeof i.lastUsed === 'number').sort((a, b) => (b.lastUsed! - a.lastUsed!)).slice(0, RECENT_LIMIT);
        break;
      case 'collections':
        base = this.activeCollection ? base.filter((i) => (i.collections || []).includes(this.activeCollection!)) : [];
        break;
    }
    let res = base.filter((i) => this.matches(i));
    if (this.view !== 'recent') res = res.sort((a, b) => a.name.localeCompare(b.name));
    this.results = res;
    this.selected = res.length ? Math.min(this.selected < 0 ? 0 : this.selected, res.length - 1) : -1;
    this.spacer.style.height = `${res.length * ROW}px`;
    this.scroll.scrollTop = 0;
    this.renderWindow();
    this.renderEmpty();
  }

  // --- virtualized window -------------------------------------------------

  private renderWindow(): void {
    const total = this.results.length;
    const viewport = this.scroll.clientHeight || 400;
    const first = Math.max(0, Math.floor(this.scroll.scrollTop / ROW) - BUFFER);
    const count = Math.ceil(viewport / ROW) + BUFFER * 2;
    const last = Math.min(total, first + count);
    this.windowEl.style.transform = `translateY(${first * ROW}px)`;
    const rows: HTMLElement[] = [];
    for (let i = first; i < last; i++) rows.push(this.row(this.results[i], i));
    this.windowEl.replaceChildren(...rows);
  }

  private row(it: LibraryItem, i: number): HTMLElement {
    const el = document.createElement('div');
    el.className = 'br-row' + (i === this.selected ? ' sel' : '');
    el.style.height = `${ROW}px`;
    el.setAttribute('role', 'option');
    el.setAttribute('aria-selected', String(i === this.selected));
    el.dataset.i = String(i);
    el.innerHTML = `
      <span class="br-thumb" data-type="${it.type}" aria-hidden="true">${TYPE_ICON[it.type]}</span>
      <span class="br-name">${esc(it.name)}</span>
      <span class="br-badge">${TYPE_LABEL[it.type]}</span>
      <button class="br-fav" aria-pressed="${it.favorite}" aria-label="${it.favorite ? 'Unfavorite' : 'Favorite'} ${esc(it.name)}">${it.favorite ? '★' : '☆'}</button>
      <button class="br-load" aria-label="Load ${esc(it.name)}">Load</button>`;
    el.addEventListener('click', (e) => {
      if ((e.target as HTMLElement).closest('button')) return;
      this.select(i);
    });
    el.querySelector('.br-fav')!.addEventListener('click', async (e) => {
      e.stopPropagation();
      await this.deps.setFavorite(it.id, !it.favorite);
      await this.refresh();
    });
    el.querySelector('.br-load')!.addEventListener('click', async (e) => {
      e.stopPropagation();
      await this.doLoad(it);
    });
    return el;
  }

  private renderEmpty(): void {
    if (this.results.length) {
      this.emptyEl.hidden = true;
      return;
    }
    this.emptyEl.hidden = false;
    const msgs: Record<string, string> = {
      milkdrop: 'No Milkdrop presets. Import .milk files to add presets.',
      shader: 'No shaders match.',
      scene: 'No saved scenes yet. Save the current scene to add one.',
      favorites: 'No favorites yet. Tap ☆ to favorite items.',
      recent: 'Nothing recent yet. Load an item to see it here.',
      collections: this.collectionsList.length ? 'This collection is empty.' : 'No collections yet. Create one below.',
      all: this.search ? `No results for “${this.search}”.` : 'Your library is empty.',
    };
    this.emptyEl.textContent = msgs[this.view] ?? 'Nothing here.';
  }

  // --- selection / details / keyboard ------------------------------------

  private select(i: number): void {
    this.selected = i;
    this.renderWindow();
    this.renderDetails();
  }

  selectIndex(i: number): void {
    if (i < 0 || i >= this.results.length) return;
    this.select(i);
    const top = i * ROW;
    if (top < this.scroll.scrollTop || top + ROW > this.scroll.scrollTop + this.scroll.clientHeight) this.scroll.scrollTop = top;
  }

  private onKey(e: KeyboardEvent): void {
    const t = e.target as HTMLElement;
    if (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable) return;
    if (e.key === 'ArrowDown') { e.preventDefault(); this.selectIndex(Math.min(this.selected + 1, this.results.length - 1)); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); this.selectIndex(Math.max(this.selected - 1, 0)); }
    else if (e.key === 'Enter') { if (this.selected >= 0) void this.doLoad(this.results[this.selected]); }
    else if (e.key === 'f' || e.key === 'F') {
      const it = this.results[this.selected];
      if (it) void this.deps.setFavorite(it.id, !it.favorite).then(() => this.refresh());
    }
  }

  private renderDetails(): void {
    const it = this.results[this.selected];
    if (!it) { this.detailsEl.hidden = true; return; }
    this.detailsEl.hidden = false;
    const rows: [string, string | undefined][] = [
      ['Name', it.name], ['Type', TYPE_LABEL[it.type]], ['Author', it.author], ['Description', it.description],
      ['Tags', (it.tags || []).join(', ') || undefined], ['License', it.license], ['Attribution', it.attribution?.attributionText],
      ['Source', it.attribution?.sourceUrl], ['Origin', it.origin],
      ['Textures', (it.tags || []).includes('requires-textures') ? 'may require external textures' : undefined],
    ];
    const meta = rows.filter(([, v]) => v).map(([k, v]) => `<div class="br-d-row"><b>${k}</b><span>${esc(String(v))}</span></div>`).join('');
    const builtin = it.origin === 'builtin';
    const user = it.origin === 'user' || it.origin === 'imported';
    const actions: string[] = ['<button data-a="load">Load</button>', `<button data-a="fav">${it.favorite ? 'Unfavorite' : 'Favorite'}</button>`, '<button data-a="addcol">Add to Collection</button>'];
    if (user) actions.push('<button data-a="rename">Rename</button>', '<button data-a="delete">Delete</button>');
    if (builtin || it.type !== 'scene') actions.push('<button data-a="duplicate">Duplicate</button>');
    this.detailsEl.innerHTML = `<div class="br-d-meta">${meta}</div><div class="br-d-actions">${actions.join('')}</div>`;
    this.detailsEl.querySelectorAll('button').forEach((b) =>
      b.addEventListener('click', () => void this.action((b as HTMLElement).dataset.a!, it)),
    );
  }

  private async action(a: string, it: LibraryItem): Promise<void> {
    if (a === 'load') return void this.doLoad(it);
    if (a === 'fav') { await this.deps.setFavorite(it.id, !it.favorite); return this.refresh(); }
    if (a === 'duplicate') { await this.deps.duplicate(it.id); this.deps.status(`Duplicated “${it.name}”`); return this.refresh(); }
    if (a === 'rename') { const n = prompt('Rename to:', it.name); if (n) { await this.deps.rename(it.id, n); return this.refresh(); } return; }
    if (a === 'delete') { if (confirm(`Delete “${it.name}”?`)) { await this.deps.remove(it.id); this.selected = -1; return this.refresh(); } return; }
    if (a === 'addcol') { await this.addToCollectionFlow(it); }
  }

  private async addToCollectionFlow(it: LibraryItem): Promise<void> {
    const names = this.collectionsList.map((c) => c.name);
    const pick = prompt(`Add to collection (existing: ${names.join(', ') || 'none'}) — type a name:`);
    if (!pick) return;
    let col = this.collectionsList.find((c) => c.name === pick);
    if (!col) col = await this.deps.createCollection(pick);
    await this.deps.addToCollection(it.id, col.id);
    this.deps.status(`Added to “${col.name}”`);
    await this.refresh();
  }

  // --- collections bar ----------------------------------------------------

  private renderCollbar(): void {
    const chips = this.collectionsList.map((c) => {
      const b = document.createElement('button');
      b.className = 'br-chip' + (c.id === this.activeCollection ? ' on' : '');
      b.textContent = c.name;
      b.addEventListener('click', () => { this.activeCollection = c.id; this.renderCollbar(); this.applyFilter(); });
      return b;
    });
    const add = document.createElement('button');
    add.className = 'br-chip br-chip-add';
    add.textContent = '+ New';
    add.addEventListener('click', async () => { const n = prompt('Collection name:'); if (n) { const c = await this.deps.createCollection(n); this.activeCollection = c.id; await this.refresh(); this.renderCollbar(); } });
    const tools: HTMLElement[] = [];
    if (this.activeCollection) {
      const ren = document.createElement('button'); ren.className = 'br-chip'; ren.textContent = 'Rename';
      ren.addEventListener('click', async () => { const n = prompt('Rename collection:'); if (n && this.activeCollection) { await this.deps.renameCollection(this.activeCollection, n); await this.refresh(); this.renderCollbar(); } });
      const del = document.createElement('button'); del.className = 'br-chip'; del.textContent = 'Delete';
      del.addEventListener('click', async () => { if (this.activeCollection && confirm('Delete collection (items are kept)?')) { await this.deps.deleteCollection(this.activeCollection); this.activeCollection = null; await this.refresh(); this.setView('collections'); } });
      tools.push(ren, del);
    }
    this.collbar.replaceChildren(...chips, add, ...tools);
  }

  // --- load / navigation / import ----------------------------------------

  private async doLoad(it: LibraryItem): Promise<void> {
    const res = await this.deps.load(it);
    this.deps.status(res.ok ? `Loaded ${TYPE_LABEL[it.type]}: ${it.name}` : `Load failed (kept current): ${res.error ?? ''}`);
    if (res.ok) await this.refresh(); // reflect recent/usage
  }

  private async navMilkdrop(kind: 'prev' | 'next' | 'random'): Promise<void> {
    const milk = this.results.filter((i) => i.type === 'milkdrop');
    if (!milk.length) { this.deps.status('No Milkdrop presets in the current results'); return; }
    const curId = this.selected >= 0 ? this.results[this.selected]?.id : undefined;
    let idx = milk.findIndex((i) => i.id === curId);
    let pick: LibraryItem;
    if (kind === 'random') pick = milk[Math.floor(Math.random() * milk.length)];
    else if (kind === 'next') pick = milk[idx < 0 ? 0 : (idx + 1) % milk.length];
    else pick = milk[idx < 0 ? milk.length - 1 : (idx - 1 + milk.length) % milk.length];
    const globalIdx = this.results.indexOf(pick);
    if (globalIdx >= 0) this.selectIndex(globalIdx);
    await this.doLoad(pick);
  }

  private async onImport(): Promise<void> {
    const files = [...(this.fileInput.files ?? [])];
    this.fileInput.value = '';
    if (!files.length) return;
    const added = await this.deps.importMilk(files);
    this.deps.status(`Imported ${added.length} preset(s)`);
    await this.refresh();
    this.setView('milkdrop');
  }

  // --- test/introspection helpers ----------------------------------------

  resultCount(): number {
    return this.results.length;
  }
  renderedRowCount(): number {
    return this.windowEl.querySelectorAll('.br-row').length;
  }
  scrollTo(px: number): void {
    this.scroll.scrollTop = px;
    this.renderWindow();
  }
  scrollHeight(): number {
    return this.results.length * ROW;
  }
  currentView(): View {
    return this.view;
  }
}
