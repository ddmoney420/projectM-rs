//! Compatibility report: run the preset engine over a directory tree of `.milk`
//! files and summarize how many load and evaluate.
//!
//! ```text
//! cargo run -p pm-preset --example compat_report --release -- <dir> [<dir>...]
//! ```
//!
//! For each preset it runs the full CPU pipeline — parse, compile the equation
//! blocks, run `per_frame_init`, one `per_frame`, and one `per_pixel` vertex —
//! and buckets the outcome. The histogram of unknown functions points directly
//! at the remaining `pm-eval` gaps.

use pm_audio::FrameAudioData;
use pm_preset::{FrameParams, Preset, PresetError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Report {
    total: usize,
    ok: usize,
    by_bucket: HashMap<String, usize>,
    unknown_functions: HashMap<String, usize>,
    samples: HashMap<String, (String, String)>, // bucket -> (file, message)
}

impl Report {
    fn record_ok(&mut self) {
        self.total += 1;
        self.ok += 1;
    }

    fn record_err(&mut self, path: &Path, err: &PresetError) {
        self.total += 1;
        let bucket = bucket_of(err);
        *self.by_bucket.entry(bucket.clone()).or_default() += 1;
        self.samples
            .entry(bucket)
            .or_insert_with(|| (path.display().to_string(), err.to_string()));

        if let PresetError::Eval { source: pm_eval::EvalError::UnknownFunction(name), .. } = err {
            *self.unknown_functions.entry(name.clone()).or_default() += 1;
        }
    }
}

fn bucket_of(err: &PresetError) -> String {
    match err {
        PresetError::InvalidFile => "invalid_file".into(),
        PresetError::Compile { block, .. } => format!("compile:{block}"),
        PresetError::Eval { block, source } => match source {
            pm_eval::EvalError::UnknownFunction(_) => format!("eval:{block}:unknown_fn"),
            _ => format!("eval:{block}:other"),
        },
    }
}

/// Load + exercise one preset through the full CPU pipeline.
fn check(content: &str) -> Result<(), PresetError> {
    let mut preset = Preset::load(content)?; // parse + compile + per_frame_init
    let frame = FrameParams { viewport_width: 1280, viewport_height: 720, ..FrameParams::default() };
    preset.update_frame(frame, FrameAudioData::default())?; // per_frame
    if preset.has_per_pixel_code() {
        preset.warp_vertex(0.5, 0.5, 0.5, 0.0)?; // per_pixel
    }
    Ok(())
}

fn collect_milk_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_milk_files(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk")) {
            out.push(path);
        }
    }
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if dirs.is_empty() {
        eprintln!("usage: compat_report <dir> [<dir>...]");
        std::process::exit(2);
    }

    let mut files = Vec::new();
    for dir in &dirs {
        collect_milk_files(dir, &mut files);
    }
    files.sort();
    println!("Found {} .milk files", files.len());

    let mut report = Report::default();
    for path in &files {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let content = String::from_utf8_lossy(&bytes);
        match check(&content) {
            Ok(()) => report.record_ok(),
            Err(e) => report.record_err(path, &e),
        }
    }

    let pct = |n: usize| if report.total > 0 { 100.0 * n as f64 / report.total as f64 } else { 0.0 };

    println!("\n===== Compatibility report =====");
    println!("Total presets:  {}", report.total);
    println!("Fully OK:       {}  ({:.1}%)", report.ok, pct(report.ok));
    println!("Failed:         {}  ({:.1}%)", report.total - report.ok, pct(report.total - report.ok));

    println!("\n--- Failures by bucket ---");
    let mut buckets: Vec<_> = report.by_bucket.iter().collect();
    buckets.sort_by(|a, b| b.1.cmp(a.1));
    for (bucket, count) in buckets {
        println!("  {count:>6}  {bucket}");
        if let Some((file, msg)) = report.samples.get(bucket) {
            let short = Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            println!("            e.g. {short}: {}", msg.lines().next().unwrap_or("").trim());
        }
    }

    if !report.unknown_functions.is_empty() {
        println!("\n--- Most-wanted missing functions (pm-eval gaps) ---");
        let mut fns: Vec<_> = report.unknown_functions.iter().collect();
        fns.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in fns.iter().take(20) {
            println!("  {count:>6}  {name}()");
        }
    }
}
