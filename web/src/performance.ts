// Phase 10C.3 — canonical performance command layer.
//
// One implementation of every performance behaviour, called by UI buttons,
// keyboard shortcuts, AND MIDI actions so the three paths never diverge. The
// crossfader always flows through `setCrossfader` (the single runtime setter);
// audition always flows through the transactional `audition` path. This owns the
// Preview-Bank cursor so "next/previous bank item" means the same thing
// everywhere.

import type { LibraryItem } from './library';

export interface ImportResult {
  ok: boolean;
  error?: string;
}

export interface PerformanceContext {
  /** Currently selected Library item (browser selection), or null. */
  selectedItem(): LibraryItem | null;
  /** Resolved Preview-Bank items in order (missing refs already skipped). */
  bankItems(): LibraryItem[];
  /** A random Milkdrop item from the current library, or null. */
  randomMilkdropItem(): LibraryItem | null;
  audition(item: LibraryItem): Promise<ImportResult>;
  clearAudition(): void;
  getCrossfader(): number;
  setCrossfader(t: number): void;
  favorite(item: LibraryItem, fav: boolean): Promise<void>;
  status(msg: string): void;
  /** Refresh any observing UI after state changes. */
  onChange(): void;
}

export const CROSSFADER_NUDGE = 0.05;

/** Centralized focus guard: performance shortcuts must never fire while the user
 *  is typing (search box, text inputs, textareas, CodeMirror, contenteditable,
 *  or a range slider being keyed). Reused by every keyboard entry point. */
export function shouldHandlePerformanceShortcut(e: KeyboardEvent): boolean {
  if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return false;
  const t = e.target as HTMLElement | null;
  if (!t) return true;
  const tag = t.tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return false;
  if (t.isContentEditable) return false;
  if (t.closest('.cm-editor')) return false; // CodeMirror
  return true;
}

export class PerformanceActions {
  private bankCursor = -1;

  constructor(private readonly ctx: PerformanceContext) {}

  // --- audition (transactional; never moves the crossfader) --------------

  async auditionSelected(): Promise<void> {
    const it = this.ctx.selectedItem();
    if (!it) {
      this.ctx.status('No item selected to audition.');
      return;
    }
    await this.doAudition(it);
  }

  private async doAudition(it: LibraryItem): Promise<void> {
    const res = await this.ctx.audition(it);
    this.ctx.status(res.ok ? `Auditioning: ${it.name}` : `Audition failed (live unaffected): ${res.error ?? ''}`);
    this.ctx.onChange();
  }

  clearAudition(): void {
    this.ctx.clearAudition();
    this.ctx.status('Cleared audition (Deck B)');
    this.ctx.onChange();
  }

  // --- preview bank navigation (wrap; empty = safe no-op) ----------------

  /** Move the bank cursor without auditioning; returns the selected item. */
  selectNextBankItem(): LibraryItem | null {
    return this.stepBank(1);
  }
  selectPreviousBankItem(): LibraryItem | null {
    return this.stepBank(-1);
  }

  private stepBank(dir: 1 | -1): LibraryItem | null {
    const items = this.ctx.bankItems();
    if (!items.length) {
      this.bankCursor = -1;
      return null;
    }
    this.bankCursor = this.bankCursor < 0
      ? (dir > 0 ? 0 : items.length - 1)
      : (this.bankCursor + dir + items.length) % items.length; // wrap end↔start
    const it = items[this.bankCursor];
    this.ctx.status(`Bank ${this.bankCursor + 1}/${items.length}: ${it.name}`);
    this.ctx.onChange();
    return it;
  }

  /** Audition the currently-selected bank item (or the first if none selected). */
  async auditionCurrentBankItem(): Promise<void> {
    const items = this.ctx.bankItems();
    if (!items.length) {
      this.ctx.status('Preview Bank is empty.');
      return;
    }
    if (this.bankCursor < 0 || this.bankCursor >= items.length) this.bankCursor = 0;
    await this.doAudition(items[this.bankCursor]);
  }

  /** Advance the bank cursor AND audition the new item (one performance move). */
  async auditionNextBankItem(): Promise<void> {
    const it = this.selectNextBankItem();
    if (it) await this.doAudition(it);
    else this.ctx.status('Preview Bank is empty.');
  }

  // --- crossfader (single setter is the source of truth) -----------------

  setCrossfader(t: number): void {
    this.ctx.setCrossfader(Math.max(0, Math.min(1, t)));
    this.ctx.onChange();
  }
  nudgeCrossfader(delta: number): void {
    this.setCrossfader(this.ctx.getCrossfader() + delta);
  }
  mixToA(): void {
    this.setCrossfader(0);
  }
  mixToB(): void {
    this.setCrossfader(1);
  }
  mixCenter(): void {
    this.setCrossfader(0.5);
  }

  // --- misc ---------------------------------------------------------------

  /** Prepare a random Milkdrop preset on Deck B (audition — never disrupts live). */
  async randomMilkdrop(): Promise<void> {
    const it = this.ctx.randomMilkdropItem();
    if (!it) {
      this.ctx.status('No Milkdrop presets available (import a pack or .milk files).');
      return;
    }
    await this.doAudition(it);
  }

  async favoriteSelected(): Promise<void> {
    const it = this.ctx.selectedItem();
    if (!it) return;
    await this.ctx.favorite(it, !it.favorite);
    this.ctx.onChange();
  }

  /** Route a MIDI `performance.*` action id to a command. Returns true if known. */
  dispatchAction(id: string): boolean {
    switch (id) {
      case 'performance.audition_selected': void this.auditionSelected(); return true;
      case 'performance.bank_next': this.selectNextBankItem(); return true;
      case 'performance.bank_previous': this.selectPreviousBankItem(); return true;
      case 'performance.bank_audition_next': void this.auditionNextBankItem(); return true;
      case 'performance.clear_audition': this.clearAudition(); return true;
      case 'performance.random_milkdrop': void this.randomMilkdrop(); return true;
      case 'performance.favorite_selected': void this.favoriteSelected(); return true;
      case 'performance.mix_to_a': this.mixToA(); return true;
      case 'performance.mix_to_b': this.mixToB(); return true;
      case 'performance.mix_center': this.mixCenter(); return true;
      default: return false;
    }
  }

  /** Handle a document keydown (already focus-guarded by the caller). Returns
   *  true if the key was consumed. */
  handleKey(e: KeyboardEvent): boolean {
    switch (e.key) {
      case '[': this.selectPreviousBankItem(); return true;
      case ']': this.selectNextBankItem(); return true;
      case 'p': case 'P': void this.auditionSelected(); return true;
      case 'r': case 'R': void this.randomMilkdrop(); return true;
      case '1': this.mixToA(); return true;
      case '2': this.mixCenter(); return true;
      case '3': this.mixToB(); return true;
      case 'ArrowLeft': if (e.shiftKey) { this.nudgeCrossfader(-CROSSFADER_NUDGE); return true; } return false;
      case 'ArrowRight': if (e.shiftKey) { this.nudgeCrossfader(CROSSFADER_NUDGE); return true; } return false;
      default: return false;
    }
  }
}
