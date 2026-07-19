# pm-web Security Review (Phase 9)

Scope: the browser app's handling of untrusted input (imported scenes, share
URLs, shader source, MIDI, filenames) and resource bounds. No backend exists;
all data is local.

## Findings

### Scene import / share URLs
- Scenes are parsed with `serde_json` in `pm-scene::parse_scene`, which **never
  executes** anything — it deserializes to plain data. Unknown schema versions
  are **rejected**; import is **transactional** (parse + validate, then swap), so
  a bad import leaves the current scene intact.
- Share URLs are `deflate-raw` + base64url in the URL **fragment** (never sent to
  a server). Decoding is wrapped in try/catch; a bad payload is ignored and the
  current scene is kept. Oversized scenes are rejected with a message rather than
  producing an unusable URL.

### Resource bounds (prevent unbounded GPU/CPU allocation from imported data)
Enforced in `pm-scene::validate`:
- `MAX_LAYERS = 16`, `MAX_SHADER_LAYERS = 8`.
- `MAX_EFFECTS_PER_LAYER = 8`, `MAX_GLOBAL_EFFECTS = 8`, `MAX_TOTAL_EFFECTS = 64`.
- `MAX_SHADER_SOURCE = 64 KB` per pass; `MAX_PROJECT_SOURCE = 256 KB` per shader
  project; `MAX_SCENE_BYTES = 1 MB` for the whole scene JSON.
- Multipass: `MAX_PASSES = 5` (Buffer A–D + Image), truncated on import.
- Opacity/transform/speed/bpm are clamped. Imported buffer/feedback **history is
  never deserialized** — it always starts cleared, so no scene can inject GPU
  memory contents.

### Untrusted metadata rendering (XSS)
- All user/imported strings shown in the UI (layer names, attribution, mapping
  targets, diagnostics, About capability names) are HTML-escaped before
  insertion, or set via `textContent`. Grep points: `escapeHtml`/`esc` helpers in
  `layers.ts`, `midi-ui.ts`, `help.ts`; JSON string escaping in the Rust
  `json_str`/`json_string` helpers.
- **Imported scene data is never executed as JavaScript.** There is no `eval`,
  `new Function`, or `innerHTML` of untrusted content.

### MIDI
- Mappings are plain serializable data (target path + filter + range). Incoming
  MIDI is parsed to a small typed event; system real-time / SysEx are dropped
  before routing or learning. No SysEx permission is ever requested. MIDI data is
  never uploaded.

### Downloads / blob URLs
- Recording and scene export create local `Blob` URLs and revoke them after use.
  Download filenames are app-generated (timestamp / `scene.json`), not derived
  from untrusted input.

### Projection postMessage
- The control and output windows exchange a tiny versioned protocol; every
  incoming message is validated (`parseMessage`) with an origin check
  (`e.origin === location.origin`) and a version check before use. The shared
  MediaStream is same-origin by reference.

## Conclusion
No path was found where imported/untrusted data can inject script or allocate
unbounded GPU/CPU resources. Bounds are centralized in `pm-scene::validate` and
covered by unit tests; transactional import + last-known-good compilation keep a
malformed input from destabilizing the running app.
