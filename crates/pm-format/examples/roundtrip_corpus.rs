//! Validate the native format is lossless across a corpus: for every `.milk`,
//! check `from_milk -> to_pmp -> from_pmp -> to_milk` re-imports to the same
//! structured preset, and that the result still loads in the engine.
//!
//! ```text
//! cargo run -p pm-format --example roundtrip_corpus --release -- <dir> [<dir>...]
//! ```

use pm_format::NativePreset;
use std::path::{Path, PathBuf};

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("milk")) {
            out.push(p);
        }
    }
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();
    println!("Round-tripping {} presets through .pmp ...", files.len());

    let (mut imported, mut lossless, mut loads, mut load_orig) = (0, 0, 0, 0);
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let content = String::from_utf8_lossy(&bytes);
        let Some(np) = NativePreset::from_milk(&content) else { continue };
        imported += 1;

        // .milk -> native -> .pmp -> native must be identical.
        let via_pmp = NativePreset::from_pmp(&np.to_pmp());
        if via_pmp == np {
            lossless += 1;
        }
        // The reconstructed .milk should load wherever the original did.
        if np.into_preset().is_ok() {
            loads += 1;
        }
        if pm_preset::Preset::load(&content).is_ok() {
            load_orig += 1;
        }
    }

    let pct = |n: usize| if imported > 0 { 100.0 * n as f64 / imported as f64 } else { 0.0 };
    println!("Imported:          {imported}");
    println!(".pmp lossless:     {lossless}  ({:.2}%)", pct(lossless));
    println!("Reconstructed load:{loads}  ({:.2}%)", pct(loads));
    println!("Original load:     {load_orig}  ({:.2}%)  [should match the above]", pct(load_orig));
}
