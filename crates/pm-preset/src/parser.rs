//! Port of `MilkdropPreset/PresetFileParser.{hpp,cpp}` — the `.milk` (INI-like)
//! file parser.
//!
//! Each line is `key<delim>value` where `<delim>` is the first space or `=`.
//! Keys are lowercased; the first occurrence of a key wins (Milkdrop behavior).
//! Multi-line code blocks are stored as numbered keys (`per_frame_1`,
//! `per_frame_2`, …) and reassembled by [`PresetFile::get_code`].

use std::collections::BTreeMap;

/// Reject absurdly large inputs, matching upstream's `maxFileSize` guard.
const MAX_FILE_SIZE: usize = 32 * 1024 * 1024;

#[derive(Debug, Default, Clone)]
pub struct PresetFile {
    values: BTreeMap<String, String>,
}

impl PresetFile {
    /// Parse `.milk` file contents. Returns `None` if it looks like binary
    /// (contains a NUL) or is too large or yields no key/value pairs.
    pub fn parse(content: &str) -> Option<PresetFile> {
        if content.len() > MAX_FILE_SIZE || content.contains('\0') {
            return None;
        }

        let mut values = BTreeMap::new();
        for line in content.lines() {
            parse_line(line, &mut values);
        }

        if values.is_empty() {
            None
        } else {
            Some(PresetFile { values })
        }
    }

    /// Reassemble a numbered code block: concatenates `prefix1`, `prefix2`, …
    /// (one per line) until the next index is missing, stripping the leading
    /// backtick Milkdrop puts on each code line.
    pub fn get_code(&self, prefix: &str) -> String {
        let prefix = prefix.to_ascii_lowercase();
        let mut code = String::new();
        for index in 1.. {
            let key = format!("{prefix}{index}");
            match self.values.get(&key) {
                Some(line) => {
                    let line = line.strip_prefix('`').unwrap_or(line);
                    code.push_str(line);
                    code.push('\n');
                }
                None => break,
            }
        }
        code
    }

    /// Like [`PresetFile::get_code`] but for equation blocks: appends a `;` to
    /// each line that forms a complete statement (i.e. doesn't end in a
    /// continuation operator). Milkdrop stores one statement per numbered line,
    /// often without a trailing `;`, relying on ns-eel's line-boundary
    /// statement separation; this reproduces that. Multi-line expressions
    /// (a line ending in `+`, `(`, `,`, …) are left joined.
    pub fn get_code_statements(&self, prefix: &str) -> String {
        let prefix = prefix.to_ascii_lowercase();

        let mut lines = Vec::new();
        for index in 1.. {
            let key = format!("{prefix}{index}");
            let Some(line) = self.values.get(&key) else { break };
            let line = line.strip_prefix('`').unwrap_or(line);
            lines.push(line.trim_end().to_string());
        }

        let mut code = String::new();
        for (i, line) in lines.iter().enumerate() {
            code.push_str(line);
            // Insert a separator only when this line is a complete statement AND
            // the next line doesn't continue it with a leading binary operator.
            let next_continues =
                lines.get(i + 1).is_some_and(|n| starts_with_continuation(n));
            if !ends_with_continuation(line) && !next_continues {
                code.push(';');
            }
            code.push('\n');
        }
        code
    }

    pub fn get_int(&self, key: &str, default: i32) -> i32 {
        self.values
            .get(&key.to_ascii_lowercase())
            .and_then(|v| parse_leading_int(v))
            .unwrap_or(default)
    }

    pub fn get_float(&self, key: &str, default: f32) -> f32 {
        self.values
            .get(&key.to_ascii_lowercase())
            .and_then(|v| parse_leading_float(v))
            .unwrap_or(default)
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get_int(key, default as i32) > 0
    }

    pub fn get_string(&self, key: &str, default: &str) -> String {
        self.values
            .get(&key.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// Raw access to the parsed key/value map.
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }
}

/// A trimmed line "continues" (no implicit `;`) if it's empty or ends in an
/// operator / open bracket / comma — i.e. the expression isn't complete yet.
/// A line already ending in `;` also needs no extra separator.
fn ends_with_continuation(line: &str) -> bool {
    match line.chars().last() {
        None => true,
        Some(c) => ";+-*/%^&|=<>!,([?~".contains(c),
    }
}

/// A line "continues" the previous one if it begins with a strictly-binary
/// operator (`*`, `/`, `)` …) that can't start a fresh expression. `+`/`-`/`!`
/// are excluded since they can be unary at the start of a new statement.
fn starts_with_continuation(line: &str) -> bool {
    match line.trim_start().chars().next() {
        None => false,
        Some(c) => "*/%^&|<>=,)]".contains(c),
    }
}

fn parse_line(line: &str, values: &mut BTreeMap<String, String>) {
    // First delimiter is a space or '='.
    let delim = line.find([' ', '=']);
    let Some(pos) = delim else { return };
    if pos == 0 {
        return;
    }
    let key = line[..pos].to_ascii_lowercase();
    let value = &line[pos + 1..];
    // First occurrence wins.
    values.entry(key).or_insert_with(|| value.to_string());
}

/// Parse a leading float the way C `strtof`/`stof` does: take the longest valid
/// numeric prefix and ignore any trailing characters.
fn parse_leading_float(s: &str) -> Option<f32> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if saw_digit && i < b.len() && (b[i] | 0x20) == b'e' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    if !saw_digit {
        return None;
    }
    s[..i].parse().ok()
}

fn parse_leading_int(s: &str) -> Option<i32> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let start_digits = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == start_digits {
        return None;
    }
    s[..i].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_value_pairs() {
        let f = PresetFile::parse("zoom=1.5\nrot 0.25\nfDecay=0.98").unwrap();
        assert_eq!(f.get_float("zoom", 0.0), 1.5);
        assert_eq!(f.get_float("rot", 0.0), 0.25); // space delimiter
        assert_eq!(f.get_float("fdecay", 0.0), 0.98); // case-insensitive key
    }

    #[test]
    fn first_occurrence_wins() {
        let f = PresetFile::parse("zoom=1.0\nzoom=2.0").unwrap();
        assert_eq!(f.get_float("zoom", 0.0), 1.0);
    }

    #[test]
    fn get_code_concatenates_numbered_lines() {
        let src = "per_frame_1=`zoom = 1.0 + 0.1*bass;\nper_frame_2=`rot = rot + 0.01;\nper_frame_3=`wave_r = 0.5;";
        let f = PresetFile::parse(src).unwrap();
        let code = f.get_code("per_frame_");
        assert_eq!(code, "zoom = 1.0 + 0.1*bass;\nrot = rot + 0.01;\nwave_r = 0.5;\n");
    }

    #[test]
    fn get_code_stops_at_gap() {
        // per_frame_3 missing -> only 1 and 2 are collected.
        let f = PresetFile::parse("per_frame_1=a;\nper_frame_2=b;\nper_frame_4=d;").unwrap();
        assert_eq!(f.get_code("per_frame_"), "a;\nb;\n");
    }

    #[test]
    fn get_code_statements_inserts_separators() {
        // Lines without trailing ';' get one; a line ending in '+' is joined.
        let src = "per_frame_1=`xspeed = 0.5\nper_frame_2=`yspeed = 0.3\nper_frame_3=`zoom = 1.0 +\nper_frame_4=`0.1 * bass";
        let f = PresetFile::parse(src).unwrap();
        let code = f.get_code_statements("per_frame_");
        assert_eq!(code, "xspeed = 0.5;\nyspeed = 0.3;\nzoom = 1.0 +\n0.1 * bass;\n");
    }

    #[test]
    fn get_code_statements_preserves_existing_semicolons() {
        let f = PresetFile::parse("per_frame_1=`a = 1; b = 2;").unwrap();
        assert_eq!(f.get_code_statements("per_frame_"), "a = 1; b = 2;\n");
    }

    #[test]
    fn leading_numeric_prefix() {
        let f = PresetFile::parse("x=1.5xyz\ny=42 comment").unwrap();
        assert_eq!(f.get_float("x", 0.0), 1.5);
        assert_eq!(f.get_int("y", 0), 42);
    }

    #[test]
    fn bool_and_defaults() {
        let f = PresetFile::parse("on=1\noff=0").unwrap();
        assert!(f.get_bool("on", false));
        assert!(!f.get_bool("off", true));
        assert!(f.get_bool("missing", true)); // default used
    }

    #[test]
    fn rejects_binary() {
        assert!(PresetFile::parse("zoom=1\0\x01binary").is_none());
    }
}
