//! Debug helper: translate one preset's composite (or warp) shader and print
//! the generated WGSL plus the naga error, to diagnose failures.
//!
//! ```text
//! cargo run -p pm-preset --example dump_shader -- <file.milk> [warp|comp]
//! ```

use pm_preset::{shader_to_wgsl, Preset, ShaderKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: dump_shader <file.milk> [warp|comp]");
    let kind = match args.next().as_deref() {
        Some("warp") => ShaderKind::Warp,
        _ => ShaderKind::Composite,
    };

    let content = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
    let preset = Preset::load(&content).expect("load");
    let src = match kind {
        ShaderKind::Warp => preset.warp_shader_source(),
        ShaderKind::Composite => preset.composite_shader_source(),
    };
    println!("=== HLSL source ===\n{src}\n");

    match shader_to_wgsl(src, kind) {
        Err(e) => println!("translate error: {e}"),
        Ok(wgsl) => {
            match naga::front::wgsl::parse_str(&wgsl) {
                Ok(_) => println!("naga: parsed OK"),
                Err(e) => {
                    println!("=== naga error ===\n{}", e.emit_to_string(&wgsl));
                    // Print the offending region of the generated WGSL.
                    println!("=== generated WGSL (PS function) ===");
                    if let Some(pos) = wgsl.find("fn PS") {
                        println!("{}", &wgsl[pos..(pos + 1200).min(wgsl.len())]);
                    }
                }
            }
        }
    }
}
