// Phase 10A.2 — local `.milk` import helpers (browser-safe, fully local).
//
// Imported content NEVER leaves the device: it becomes an `origin:'imported'`
// LibraryItem with an inline payload in IndexedDB. We do not upload it.

/** Parse a `.milk` filename. The widely-used Milkdrop convention is
 *  "Author - Title.milk"; we surface the prefix as a *heuristic* author (it is
 *  frequently but not always the author). We do NOT fabricate an author when the
 *  convention does not clearly match — the field is simply left undefined. */
export function parseMilkFilename(filename: string): { name: string; author?: string } {
  const base = filename.replace(/\.milk$/i, '').trim();
  const m = base.match(/^(.{2,40}?)\s+-\s+(.{2,}?)$/); // "Author - Title"
  if (m) return { name: m[2].trim(), author: m[1].trim() };
  return { name: base || filename };
}

/** Best-effort detection of external texture references in a `.milk` preset
 *  (e.g. `sampler_foo`). Used only to *report* likely missing-texture needs —
 *  it never downloads anything and is not authoritative. */
export function detectTextureRefs(milkText: string): string[] {
  const refs = new Set<string>();
  for (const m of milkText.matchAll(/\bsampler_(\w+)/gi)) {
    const name = m[1].toLowerCase();
    if (name !== 'main' && name !== 'fw_main' && name !== 'pw_main' && name !== 'fc_main') refs.add(name);
  }
  return [...refs].slice(0, 32);
}

/** Read selected File objects into `{name, text}` pairs (UTF-8), skipping any
 *  that fail to read. Caller supplies the files from an explicit user gesture. */
export async function readMilkFiles(files: File[]): Promise<{ name: string; text: string }[]> {
  const out: { name: string; text: string }[] = [];
  for (const f of files) {
    try {
      const text = await f.text();
      if (text && text.length) out.push({ name: f.name, text });
    } catch {
      /* skip unreadable file */
    }
  }
  return out;
}
