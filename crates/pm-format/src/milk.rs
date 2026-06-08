//! `.milk` ⇄ [`NativePreset`] conversion.

use crate::{split_index, NativePreset};
use std::collections::BTreeMap;
use pm_preset::PresetFile;

impl NativePreset {
    /// Import a Milkdrop `.milk` preset. Returns `None` if the text isn't a
    /// parseable preset (binary, empty, or oversized).
    ///
    /// Code lines (those Milkdrop prefixes with a leading backtick) are grouped
    /// by their de-numbered prefix and ordered by index; everything else is a
    /// scalar parameter. Keys are lowercased (Milkdrop is case-insensitive).
    pub fn from_milk(content: &str) -> Option<Self> {
        let file = PresetFile::parse(content)?;

        let mut scalars = BTreeMap::new();
        // prefix -> (index -> line), so out-of-order / sparse indices still sort.
        let mut blocks: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();

        for (key, value) in file.values() {
            if let Some(code) = value.strip_prefix('`') {
                if let Some((prefix, idx)) = split_index(key) {
                    blocks.entry(prefix.to_string()).or_default().insert(idx, code.to_string());
                    continue;
                }
            }
            scalars.insert(key.clone(), value.clone());
        }

        let code =
            blocks.into_iter().map(|(prefix, lines)| (prefix, lines.into_values().collect())).collect();

        Some(NativePreset { scalars, code })
    }

    /// Reconstruct a `.milk` file. Scalars are emitted as `key=value`; each code
    /// block is renumbered from 1 with the backtick re-added, exactly as the
    /// `.milk` parser and the engine expect.
    pub fn to_milk(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.scalars {
            out.push_str(key);
            out.push('=');
            out.push_str(value);
            out.push('\n');
        }
        for (prefix, lines) in &self.code {
            for (i, line) in lines.iter().enumerate() {
                out.push_str(prefix);
                out.push_str(&(i + 1).to_string());
                out.push_str("=`");
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}
