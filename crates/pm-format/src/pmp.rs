//! The readable `.pmp` text format ⇄ [`NativePreset`].
//!
//! ```text
//! # <preset name>
//! [scalars]
//! zoom = 1.01
//! rot = 0.04
//!
//! [code per_frame_]
//!     wave_r = 0.5 + 0.5*sin(time);
//!     wave_g = 0.5 + 0.5*sin(time*1.3);
//!
//! [code warp_]
//!     shader_body
//!     { ret = tex2D(sampler_main, uv).xyz; }
//! ```
//!
//! Section headers sit at column 0. Code lines are indented four spaces so they
//! can't be mistaken for a header (and an *empty* code line is written as four
//! spaces, while a truly blank separator line is column-0 empty and ignored).

use crate::NativePreset;

const INDENT: &str = "    ";

impl NativePreset {
    /// Serialise to the `.pmp` text format.
    pub fn to_pmp(&self) -> String {
        let mut out = String::new();
        if let Some(name) = self.name() {
            out.push_str("# ");
            out.push_str(name);
            out.push('\n');
        }

        out.push_str("[scalars]\n");
        for (key, value) in &self.scalars {
            out.push_str(key);
            out.push_str(" = ");
            out.push_str(value);
            out.push('\n');
        }

        for (prefix, lines) in &self.code {
            out.push('\n');
            out.push_str("[code ");
            out.push_str(prefix);
            out.push_str("]\n");
            for line in lines {
                out.push_str(INDENT);
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// Parse the `.pmp` text format. Unknown/malformed lines are skipped, so a
    /// partially hand-edited file still loads what it can.
    pub fn from_pmp(text: &str) -> Self {
        let mut np = NativePreset::default();
        // None = preamble, Some(None) = scalars, Some(Some(prefix)) = code block.
        let mut section: Option<Option<String>> = None;

        for line in text.lines() {
            // A column-0 `[...]` is a section header.
            if line.starts_with('[') && line.ends_with(']') {
                let inner = &line[1..line.len() - 1];
                section = Some(match inner.strip_prefix("code ") {
                    Some(prefix) => {
                        np.code.entry(prefix.to_string()).or_default();
                        Some(prefix.to_string())
                    }
                    None => None, // "[scalars]" (or anything else) -> scalars
                });
                continue;
            }

            match &section {
                Some(Some(prefix)) => {
                    // Code line: strip the four-space indent; column-0 blank
                    // lines are separators, not content.
                    if let Some(code) = line.strip_prefix(INDENT) {
                        np.code.get_mut(prefix).unwrap().push(code.to_string());
                    } else if !line.is_empty() {
                        np.code.get_mut(prefix).unwrap().push(line.to_string());
                    }
                }
                Some(None) => {
                    // Scalar `key = value`. Split on the exact `" = "` separator
                    // we emit so the value's own leading/trailing whitespace
                    // survives (`.milk` keeps values verbatim); fall back to a
                    // trimmed `=` split for hand-edited files.
                    if line.trim_start().starts_with('#') || line.trim().is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once(" = ") {
                        np.scalars.insert(k.to_string(), v.to_string());
                    } else if let Some((k, v)) = line.split_once('=') {
                        np.scalars.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
                None => {} // preamble comments before the first section
            }
        }
        np
    }
}
