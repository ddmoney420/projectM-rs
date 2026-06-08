//! Convert a `.milk` preset to the native `.pmp` format (or back).
//!
//! ```text
//! cargo run -p pm-format --example convert -- preset.milk          # -> preset.pmp
//! cargo run -p pm-format --example convert -- preset.pmp           # -> preset.milk
//! ```

use pm_format::NativePreset;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("usage: convert <file.milk|file.pmp>");
    let text = std::fs::read_to_string(&path).expect("read input");
    let p = Path::new(&path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");

    let (np, out_path, out_text) = if ext.eq_ignore_ascii_case("pmp") {
        let np = NativePreset::from_pmp(&text);
        let out = p.with_extension("milk");
        let body = np.to_milk();
        (np, out, body)
    } else {
        let np = NativePreset::from_milk(&text).expect("parse .milk");
        let out = p.with_extension("pmp");
        let body = np.to_pmp();
        (np, out, body)
    };

    std::fs::write(&out_path, &out_text).expect("write output");
    println!(
        "{} -> {}  ({} scalars, {} code blocks{})",
        p.display(),
        out_path.display(),
        np.scalars.len(),
        np.code.len(),
        np.name().map(|n| format!(", name: {n}")).unwrap_or_default(),
    );

    // Confirm the converted preset still loads in the engine.
    match np.into_preset() {
        Ok(_) => println!("loads into engine: OK"),
        Err(e) => println!("loads into engine: FAILED ({e:?})"),
    }
}
