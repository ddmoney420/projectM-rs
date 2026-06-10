# pm-app — macOS quick start

A 5-minute guide to building and running the **`pm-app`** Milkdrop player on macOS
(Apple Silicon or Intel). It renders via **Metal** through `wgpu`.

> ✅ **Status: verified on macOS** (2026-06-09, iMac · Apple M4 · arm64).
> First-try clean build (29s release), Metal backend, window + presets rendered
> correctly under human observation against the cream-of-the-crop corpus
> (9,795 presets found; black-preset skip and help overlay confirmed working).
> Audio captured the default input device (built-in mic); music *reactivity*
> not yet exercised — see [Audio](#audio).

## Prerequisites

```sh
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"      # or open a new terminal

# Xcode Command Line Tools (the linker; wgpu/cpal won't build without them)
xcode-select --install
```

## Get the code and a preset corpus

No presets are bundled with the repo (by design — see `NOTICE`), so grab a pack.

```sh
# Clone the project (private repo: you'll need credentials, e.g. `gh auth login`
# or a personal access token; install gh with `brew install gh` if needed)
git clone https://github.com/ddmoney420/projectM-rs.git
cd projectM-rs

# A free Milkdrop preset pack, for example:
git clone https://github.com/projectM-visualizer/presets-cream-of-the-crop.git ~/milk-presets
```

Clone fresh rather than copying a Windows working directory — a clean clone is
~1.4 MB and skips the large platform-specific `target/` build artifacts.

## Build and run

```sh
# First build is slow (wgpu/naga are large); later runs are fast.
cargo run -p pm-app --release -- ~/milk-presets
```

You should see, in the terminal: `Found N presets …`, the controls banner,
`Rendering on: <your GPU>`, and `Audio source: …` — and a window opens.

Always pass a preset folder. With no argument the app prints a hint and falls
back to a tiny built-in preset.

## Audio

`pm-app` captures an **audio input** device. Two cases:

- **No input / permission denied** → it logs `Audio source: synthetic` and reacts
  to a built-in synthetic signal. Good for a first "does it render?" check; it
  never crashes on missing audio.
- **React to music you're playing** → route your system *output* to an *input*
  with a loopback device — **[BlackHole](https://github.com/ExistentialAudio/BlackHole)**
  (free) is the standard choice — then select it as the input.

On first launch macOS may show a **microphone privacy prompt** attached to your
terminal app (Terminal/iTerm). Allow it, or grant it later under
**System Settings → Privacy & Security → Microphone**.

## Controls (in-window)

Press **`/`** (or `?`) any time for the on-screen help overlay. Quick reference:

| Action | Key(s) |
|---|---|
| Next / Previous / Random | `→` `Space` `N` / `←` `P` / `R` |
| Reload current | `F5` / `L` |
| Transitions / Perf overlay / HUD | `T` / `F` / `H` |
| Freeze, then step one frame | `Pause` `K`, then `.` |
| Auto-advance, adjust interval | `A`, then `[` / `]` |
| Shuffle | `S` |
| Screenshot | `C` |
| Quit | `Esc` / `Q` |

## Where things go

- **Screenshots:** `./screenshots/` (in the working directory), timestamped (UTC).
- **Preferences:** `$XDG_CONFIG_HOME/pm-app/config.txt`, or `~/.config/pm-app/config.txt`.
- **Last preset:** `last_preset.txt` next to the config (resumes on next launch).

## Optional

- `PM_SCAN=1 cargo run -p pm-app --release -- ~/milk-presets` — print a one-off
  corpus compatibility summary at startup.
- `PM_PERF=1 …` — start with the per-second performance overlay on.

## Gatekeeper

Not an issue here: you're building locally, and locally-compiled binaries run
without signing prompts. (Gatekeeper only blocks *downloaded* unsigned binaries.)

## What to look for / report

A handy first-run checklist:

1. Does it build and launch a window?
2. Does it find your presets (`Found N presets …`)?
3. Do presets render and look right? Any black/garbled frames?
4. Audio: synthetic by default — and does it react once you wire up BlackHole?
5. Do the controls work (next/prev, pause/step, screenshot, `/` help)?
6. Smooth framerate, or stutter? (`PM_PERF=1` prints fps.)
7. Any crash, panic, or error in the terminal?

Note your **macOS version**, **Mac model / chip (Apple Silicon vs Intel)**, and
**GPU**, and include any terminal error output.
