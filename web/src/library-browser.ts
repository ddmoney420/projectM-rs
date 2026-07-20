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

export interface DeckSlot {
  name: string;
  type: string;
}

export interface BrowserDeps {
  collect(): Promise<LibraryItem[]>;
  load(item: LibraryItem): Promise<ImportResult>;
  // Phase 10B — audition into the inactive deck + preview bank + monitor.
  audition(item: LibraryItem): Promise<ImportResult>;
  clearAudition(): void;
  deckStatus(): { live: DeckSlot | null; audition: DeckSlot | null };
  getBank(): Promise<string[]>;
  setBank(ids: string[]): Promise<void>;
  attachPreview(canvas: HTMLCanvasElement): boolean;
  detachPreview(): void;
  getCrossfader(): number;
  setCrossfader(t: number): void;
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

type View = 'all' | 'milkdrop' | 'shader' | 'scene' | 'favorites' | 'recent' | 'collections' | 'bank';

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
  private bankIds: string[] = [];
  private bankCursor = -1;
  private view: View = 'all';
  private search = '';
  private activeCollection: string | null = null;
  private selected = -1;
  private previewAttached = false;

  private status!: HTMLElement;
  private previewCanvas!: HTMLCanvasElement;
  private xfader!: HTMLInputElement;
  private bankbar!: HTMLElement;
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
      <div class="br-status">
        <div class="br-status-text">
          <div><span class="br-tag br-tag-live">LIVE</span> <span id="br-live" class="br-slot">—</span></div>
          <div><span class="br-tag br-tag-aud">AUDITION</span> <span id="br-aud" class="br-slot">—</span>
            <button id="br-clear-aud" class="br-x" aria-label="Clear audition" title="Clear audition">✕</button></div>
        </div>
        <canvas id="br-preview" class="br-preview" width="128" height="72" aria-label="Audition monitor (Deck B, not shown to the audience)"></canvas>
      </div>
      <div class="br-xf">
        <span class="br-xf-a">A</span>
        <input id="br-xf" class="br-xf-slider" type="range" min="0" max="1" step="0.01" value="0"
          aria-label="Master crossfader: 0 is Deck A, 1 is Deck B" />
        <span class="br-xf-b">B</span>
        <span id="br-xf-val" class="br-xf-val" aria-hidden="true">A</span>
      </div>
      <div class="br-top">
        <input id="br-search" class="br-search" type="search" placeholder="Search library…" aria-label="Search library" />
        <button id="br-import" title="Import local .milk files">Import .milk</button>
        <input id="br-file" type="file" accept=".milk" multiple hidden />
      </div>
      <div id="br-tabs" class="br-tabs" role="tablist"></div>
      <div id="br-bankbar" class="br-bankbar" hidden></div>
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
    this.status = host.querySelector('#br-status')!;
    this.previewCanvas = host.querySelector('#br-preview')!;
    this.bankbar = host.querySelector('#br-bankbar')!;
    host.querySelector('#br-clear-aud')!.addEventListener('click', () => {
      this.deps.clearAudition();
      this.syncCrossfader();
      this.renderStatus();
    });
    // Master crossfader (mouse/touch via range input; keyboard arrows are native
    // to <input type=range>). 0 = Deck A (live default), 1 = Deck B.
    this.xfader = host.querySelector('#br-xf') as HTMLInputElement;
    this.xfader.addEventListener('input', () => {
      const t = Number(this.xfader.value);
      this.deps.setCrossfader(t);
      this.renderXfaderLabel(t);
    });
    // Attach the audition monitor (GPU blit of Deck B's texture). Non-fatal if
    // the device can't create a second surface.
    try {
      this.previewAttached = this.deps.attachPreview(this.previewCanvas);
    } catch {
      this.previewAttached = false;
    }

    const VIEWS: [View, string][] = [
      ['all', 'All'], ['milkdrop', 'Milkdrop'], ['shader', 'Shaders'], ['scene', 'Scenes'],
      ['favorites', 'Favorites'], ['recent', 'Recent'], ['collections', 'Collections'], ['bank', 'Bank'],
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
    [this.all, this.collectionsList, this.bankIds] = await Promise.all([
      this.deps.collect(),
      this.deps.collections(),
      this.deps.getBank(),
    ]);
    this.renderStatus();
    this.applyFilter();
  }

  private renderStatus(): void {
    const s = this.deps.deckStatus();
    (this.host.querySelector('#br-live') as HTMLElement).textContent = s.live ? `${s.live.name} (${s.live.type})` : '—';
    (this.host.querySelector('#br-aud') as HTMLElement).textContent = s.audition ? `${s.audition.name} (${s.audition.type})` : '—';
    this.previewCanvas.style.opacity = s.audition && this.previewAttached ? '1' : '.3';
    this.syncCrossfader();
  }

  /** Reflect the engine's crossfader value into the slider (e.g. after clear
   *  resets it to 0). */
  private syncCrossfader(): void {
    if (!this.xfader) return;
    const t = this.deps.getCrossfader();
    this.xfader.value = String(t);
    this.renderXfaderLabel(t);
  }

  private renderXfaderLabel(t: number): void {
    const label = t <= 0.001 ? 'A' : t >= 0.999 ? 'B' : `${Math.round(t * 100)}%`;
    (this.host.querySelector('#br-xf-val') as HTMLElement).textContent = label;
  }

  /** Audition an item into the inactive deck (never disturbs Deck A/master). */
  private async doAudition(it: LibraryItem): Promise<void> {
    const res = await this.deps.audition(it);
    this.deps.status(res.ok ? `Auditioning: ${it.name}` : `Audition failed (live unaffected): ${res.error ?? ''}`);
    this.renderStatus();
  }

  setView(v: View): void {
    this.view = v;
    this.activeCollection = v === 'collections' ? this.activeCollection ?? this.collectionsList[0]?.id ?? null : null;
    for (const t of this.tabsEl.querySelectorAll('.br-tab')) t.classList.toggle('on', (t as HTMLElement).dataset.view === v);
    this.navEl.hidden = !(v === 'all' || v === 'milkdrop' || v === 'favorites' || v === 'recent');
    this.collbar.hidden = v !== 'collections';
    this.bankbar.hidden = v !== 'bank';
    if (v === 'collections') this.renderCollbar();
    if (v === 'bank') this.renderBankbar();
    this.applyFilter();
  }

  private renderBankbar(): void {
    const clear = document.createElement('button');
    clear.className = 'br-chip';
    clear.textContent = 'Clear bank';
    clear.addEventListener('click', () => void this.bankClear());
    const prev = document.createElement('button');
    prev.className = 'br-chip';
    prev.textContent = '◀ Prev';
    prev.addEventListener('click', () => void this.bankNav('prev'));
    const next = document.createElement('button');
    next.className = 'br-chip';
    next.textContent = 'Next ▶';
    next.addEventListener('click', () => void this.bankNav('next'));
    const label = document.createElement('span');
    label.className = 'br-navlabel';
    label.textContent = `Audition queue (${this.bankIds.length}):`;
    this.bankbar.replaceChildren(label, prev, next, clear);
  }

  // --- preview bank ops (by id, so reorder/remove survive missing refs) ----

  private async bankAdd(id: string): Promise<void> {
    if (!this.bankIds.includes(id)) {
      this.bankIds = [...this.bankIds, id];
      await this.deps.setBank(this.bankIds);
    }
    this.deps.status('Added to Preview Bank');
    await this.refresh();
  }
  private async bankRemove(id: string): Promise<void> {
    this.bankIds = this.bankIds.filter((x) => x !== id);
    await this.deps.setBank(this.bankIds);
    await this.refresh();
    if (this.view === 'bank') this.renderBankbar();
  }
  private async bankMove(id: string, dir: 'up' | 'down'): Promise<void> {
    const i = this.bankIds.indexOf(id);
    const j = dir === 'up' ? i - 1 : i + 1;
    if (i < 0 || j < 0 || j >= this.bankIds.length) return;
    const next = [...this.bankIds];
    [next[i], next[j]] = [next[j], next[i]];
    this.bankIds = next;
    await this.deps.setBank(this.bankIds);
    await this.refresh();
    this.setView('bank');
  }
  private async bankClear(): Promise<void> {
    this.bankIds = [];
    await this.deps.setBank([]);
    this.bankCursor = -1;
    await this.refresh();
    this.setView('bank');
  }
  private async bankNav(dir: 'prev' | 'next'): Promise<void> {
    const avail = this.bankIds.filter((id) => this.all.some((i) => i.id === id));
    if (!avail.length) return;
    this.bankCursor = dir === 'next' ? (this.bankCursor + 1) % avail.length : (this.bankCursor - 1 + avail.length) % avail.length;
    const item = this.all.find((i) => i.id === avail[this.bankCursor]);
    if (item) await this.doAudition(item);
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
      case 'bank': {
        // Preview Bank order preserved; missing referenced items are skipped.
        const byId = new Map(base.map((i) => [i.id, i]));
        base = this.bankIds.map((id) => byId.get(id)).filter((x): x is LibraryItem => !!x);
        break;
      }
    }
    let res = base.filter((i) => this.matches(i));
    if (this.view !== 'recent' && this.view !== 'bank') res = res.sort((a, b) => a.name.localeCompare(b.name));
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
    const bankCtl =
      this.view === 'bank'
        ? `<button class="br-mv" data-mv="up" aria-label="Move up">▲</button>
           <button class="br-mv" data-mv="down" aria-label="Move down">▼</button>
           <button class="br-rm" aria-label="Remove from bank">✕</button>`
        : '';
    el.innerHTML = `
      <span class="br-thumb" data-type="${it.type}" aria-hidden="true">${TYPE_ICON[it.type]}</span>
      <span class="br-name">${esc(it.name)}</span>
      <span class="br-badge">${TYPE_LABEL[it.type]}</span>
      <button class="br-fav" aria-pressed="${it.favorite}" aria-label="${it.favorite ? 'Unfavorite' : 'Favorite'} ${esc(it.name)}">${it.favorite ? '★' : '☆'}</button>
      <button class="br-aud" aria-label="Audition ${esc(it.name)} (Deck B)" title="Audition into Deck B (not live)">Aud</button>
      <button class="br-load" aria-label="Load ${esc(it.name)} live">Load</button>${bankCtl}`;
    el.addEventListener('click', (e) => {
      if ((e.target as HTMLElement).closest('button')) return;
      this.select(i);
    });
    el.querySelector('.br-fav')!.addEventListener('click', async (e) => {
      e.stopPropagation();
      await this.deps.setFavorite(it.id, !it.favorite);
      await this.refresh();
    });
    el.querySelector('.br-aud')!.addEventListener('click', (e) => {
      e.stopPropagation();
      void this.doAudition(it);
    });
    el.querySelector('.br-load')!.addEventListener('click', (e) => {
      e.stopPropagation();
      void this.doLoad(it);
    });
    el.querySelectorAll('.br-mv').forEach((b) =>
      b.addEventListener('click', (e) => {
        e.stopPropagation();
        void this.bankMove(it.id, (b as HTMLElement).dataset.mv as 'up' | 'down');
      }),
    );
    el.querySelector('.br-rm')?.addEventListener('click', (e) => {
      e.stopPropagation();
      void this.bankRemove(it.id);
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
      bank: 'Preview Bank is empty. Add items from the Library (Add to Bank / B).',
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
    } else if (e.key === 'p' || e.key === 'P') {
      const it = this.results[this.selected];
      if (it) void this.doAudition(it);
    } else if (e.key === 'b' || e.key === 'B') {
      const it = this.results[this.selected];
      if (it) void this.bankAdd(it.id);
    } else if (e.key === '[') {
      e.preventDefault();
      void this.bankNav('prev');
    } else if (e.key === ']') {
      e.preventDefault();
      void this.bankNav('next');
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
    const actions: string[] = [
      '<button data-a="load" title="Load live (Deck A)">Load</button>',
      '<button data-a="audition" title="Audition (Deck B, not live)">Audition</button>',
      '<button data-a="bank">Add to Bank</button>',
      `<button data-a="fav">${it.favorite ? 'Unfavorite' : 'Favorite'}</button>`,
      '<button data-a="addcol">Add to Collection</button>',
    ];
    if (user) actions.push('<button data-a="rename">Rename</button>', '<button data-a="delete">Delete</button>');
    if (builtin || it.type !== 'scene') actions.push('<button data-a="duplicate">Duplicate</button>');
    this.detailsEl.innerHTML = `<div class="br-d-meta">${meta}</div><div class="br-d-actions">${actions.join('')}</div>`;
    this.detailsEl.querySelectorAll('button').forEach((b) =>
      b.addEventListener('click', () => void this.action((b as HTMLElement).dataset.a!, it)),
    );
  }

  private async action(a: string, it: LibraryItem): Promise<void> {
    if (a === 'load') return void this.doLoad(it);
    if (a === 'audition') return void this.doAudition(it);
    if (a === 'bank') return this.bankAdd(it.id);
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
