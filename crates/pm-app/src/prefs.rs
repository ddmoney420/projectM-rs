//! Small, dependency-free persistence for pm-app's runtime preferences.
//!
//! Stored as human-readable `key=value` lines in the per-OS app config dir
//! (`%APPDATA%\pm-app\config.txt` on Windows, `$XDG_CONFIG_HOME` or
//! `~/.config/pm-app/config.txt` elsewhere), falling back to `pm-app.conf` in
//! the working directory if no home is resolvable. A missing file means
//! defaults; a malformed file logs a warning and uses defaults for the bad keys
//! rather than failing.

use std::path::{Path, PathBuf};

/// The persisted preferences (a subset of the app's runtime state).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub hud: bool,
    pub transitions: bool,
    pub perf: bool,
    pub auto: bool,
    pub auto_interval: f32,
    pub shuffle: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { hud: true, transitions: true, perf: false, auto: false, auto_interval: 30.0, shuffle: false }
    }
}

/// The config file path (see module docs for the resolution order).
pub fn config_path() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    match base {
        Some(b) => b.join("pm-app").join("config.txt"),
        None => PathBuf::from("pm-app.conf"),
    }
}

/// Path of the "last shown preset" state file (sibling of the config). Kept
/// separate so navigation state never touches the human-edited preferences.
pub fn last_preset_path() -> PathBuf {
    let mut p = config_path();
    p.set_file_name("last_preset.txt");
    p
}

/// Read the saved last-preset string (a corpus-root-relative path), if any.
pub fn load_last_preset(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let line = s.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Write the last-preset string. Errors are logged, never fatal.
pub fn save_last_preset(path: &Path, rel: &str) {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("pm-app: could not create state dir {}: {e}", dir.display());
                return;
            }
        }
    }
    if let Err(e) = std::fs::write(path, format!("{rel}\n")) {
        eprintln!("pm-app: could not write {}: {e}", path.display());
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Some(true),
        "false" | "0" | "off" | "no" => Some(false),
        _ => None,
    }
}

impl Prefs {
    /// Load preferences from `path`. Missing file → defaults (silent). Unreadable
    /// or malformed lines → a warning + defaults for the affected keys.
    pub fn load(path: &Path) -> Prefs {
        let mut p = Prefs::default();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return p,
            Err(e) => {
                eprintln!("pm-app: could not read config {}: {e}; using defaults", path.display());
                return p;
            }
        };
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                eprintln!("pm-app: config line {} malformed (no '='), ignored", i + 1);
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let ok = match k {
                "hud" => parse_bool(v).map(|b| p.hud = b).is_some(),
                "transitions" => parse_bool(v).map(|b| p.transitions = b).is_some(),
                "perf" => parse_bool(v).map(|b| p.perf = b).is_some(),
                "auto" => parse_bool(v).map(|b| p.auto = b).is_some(),
                "shuffle" => parse_bool(v).map(|b| p.shuffle = b).is_some(),
                "auto_interval" => v.parse::<f32>().ok().map(|f| p.auto_interval = f).is_some(),
                _ => continue, // unknown key: ignore (forward-compatible)
            };
            if !ok {
                eprintln!("pm-app: config '{k}' has invalid value '{v}'; using default");
            }
        }
        p
    }

    /// Write preferences to `path`, creating the parent dir. Errors are logged,
    /// never fatal.
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    eprintln!("pm-app: could not create config dir {}: {e}", dir.display());
                    return;
                }
            }
        }
        let body = format!(
            "# pm-app preferences (auto-saved)\nhud={}\ntransitions={}\nperf={}\nauto={}\nauto_interval={}\nshuffle={}\n",
            self.hud, self.transitions, self.perf, self.auto, self.auto_interval, self.shuffle
        );
        if let Err(e) = std::fs::write(path, body) {
            eprintln!("pm-app: could not write config {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing() {
        let p = Prefs::load(Path::new("/no/such/pm-app-config-xyz.txt"));
        assert_eq!(p, Prefs::default());
    }

    #[test]
    fn parse_bool_forms() {
        assert_eq!(parse_bool("on"), Some(true));
        assert_eq!(parse_bool("FALSE"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn save_load_round_trip() {
        let path = std::env::temp_dir().join("pm-app-roundtrip-test.txt");
        let _ = std::fs::remove_file(&path);
        let p = Prefs { hud: false, transitions: false, perf: true, auto: true, auto_interval: 45.0, shuffle: true };
        p.save(&path);
        let loaded = Prefs::load(&path);
        assert_eq!(loaded, p);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_falls_back_per_key() {
        let path = std::env::temp_dir().join("pm-app-malformed-test.txt");
        // Valid `hud=off`, a junk line, a bad value, and an unknown key.
        std::fs::write(&path, "hud=off\ngarbage line\nauto_interval=banana\nunknown=1\n").unwrap();
        let p = Prefs::load(&path);
        assert!(!p.hud, "valid key applied");
        assert_eq!(p.auto_interval, Prefs::default().auto_interval, "bad value kept default");
        assert_eq!(p.transitions, Prefs::default().transitions, "untouched key default");
        let _ = std::fs::remove_file(&path);
    }
}
