//! `pm-format` — projectm-rs's native preset format (`.pmp`) plus a `.milk`
//! importer/exporter.
//!
//! The `.milk` format is a flat INI-like key/value soup: scalar parameters
//! (`zoom=1.01`), numbered equation lines (`per_frame_1=` … `per_frame_2=` …),
//! and numbered shader lines (`warp_1=` …), with code lines distinguished only
//! by a leading backtick. That's faithful to Milkdrop but awkward to read, diff,
//! or author by hand.
//!
//! [`NativePreset`] is a structured, **lossless** intermediate: scalar
//! parameters in one map, each numbered code block reassembled into a
//! multi-line string keyed by its prefix. It round-trips to/from `.milk`
//! (behaviourally — keys are normalised to lowercase, comments dropped) and
//! serialises to the readable `.pmp` text format. Because [`NativePreset::to_milk`]
//! reconstructs a valid `.milk`, an imported preset loads through the existing
//! [`pm_preset`] engine unchanged — no separate runtime path.
//!
//! ```no_run
//! let milk = std::fs::read_to_string("preset.milk").unwrap();
//! let np = pm_format::NativePreset::from_milk(&milk).unwrap();
//! std::fs::write("preset.pmp", np.to_pmp()).unwrap();           // export native
//! let back = pm_format::NativePreset::from_pmp(&np.to_pmp());   // re-import
//! let preset = pm_preset::Preset::load(&back.to_milk()).unwrap(); // run it
//! ```

use std::collections::BTreeMap;

mod milk;
mod pmp;

/// A structured, lossless representation of a Milkdrop preset.
///
/// `scalars` holds every non-code `key=value` (init parameters, flags, ratings).
/// `code` holds each numbered code block — keyed by its exact `.milk` prefix
/// (e.g. `per_frame_`, `per_pixel_`, `warp_`, `comp_`, `wave_0_per_frame`,
/// `shape_0_per_frame`) — as an ordered list of lines with the Milkdrop backtick
/// stripped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativePreset {
    pub scalars: BTreeMap<String, String>,
    pub code: BTreeMap<String, Vec<String>>,
}

impl NativePreset {
    /// The preset's display name (`name=`/`MilkdropName=` if present).
    pub fn name(&self) -> Option<&str> {
        self.scalars.get("name").or_else(|| self.scalars.get("milkdropname")).map(String::as_str)
    }

    /// Load this preset into the runtime engine via its reconstructed `.milk`.
    pub fn into_preset(&self) -> Result<pm_preset::Preset, pm_preset::PresetError> {
        pm_preset::Preset::load(&self.to_milk())
    }
}

/// Split a `.milk` key into its de-numbered prefix and trailing index, e.g.
/// `per_frame_12` → (`per_frame_`, 12) and `wave_0_per_point3` →
/// (`wave_0_per_point`, 3). Returns `None` if the key has no trailing digits.
pub(crate) fn split_index(key: &str) -> Option<(&str, u32)> {
    let digits_start = key.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits_start == key.len() {
        return None; // no trailing digits
    }
    let idx = key[digits_start..].parse().ok()?;
    Some((&key[..digits_start], idx))
}
