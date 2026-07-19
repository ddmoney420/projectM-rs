//! Parsing of `// @control ...` metadata declarations in shader source.
//!
//! Grammar (one per line):
//! ```text
//! // @control <name> float  <min> <max> <default>
//! // @control <name> int    <min> <max> <default>
//! // @control <name> bool   <true|false>
//! // @control <name> color  #rrggbb
//! // @control <name> vec2   <min> <max> <defx> <defy>
//! // @control <name> enum    opt1,opt2,opt3 <defaultIndex>
//! // @control <name> trigger
//! ```
//! Malformed declarations are skipped silently — they never block compilation.
//! Each control binds to one `vec4` slot in `pm_user[16]`; the name is `#define`d
//! to the relevant lanes (see [`control_defines`]).

/// The maximum number of user controls (one `vec4` slot each).
pub const MAX_CONTROLS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Float,
    Int,
    Bool,
    Color,
    Enum,
    Vec2,
    Trigger,
}

impl ControlKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlKind::Float => "float",
            ControlKind::Int => "int",
            ControlKind::Bool => "bool",
            ControlKind::Color => "color",
            ControlKind::Enum => "enum",
            ControlKind::Vec2 => "vec2",
            ControlKind::Trigger => "trigger",
        }
    }
}

/// A parsed user control. `default` packs the initial value into the `vec4`
/// lanes actually used by `kind` (x for scalars, xy for vec2, rgb for color).
#[derive(Debug, Clone)]
pub struct Control {
    pub name: String,
    pub kind: ControlKind,
    pub min: f32,
    pub max: f32,
    pub default: [f32; 4],
    pub slot: u32,
    pub options: Vec<String>,
}

/// Parse every `// @control` line, in declaration order, up to [`MAX_CONTROLS`].
pub fn parse_controls(src: &str) -> Vec<Control> {
    let mut out: Vec<Control> = Vec::new();
    for line in src.lines() {
        let after_slashes = match line.trim_start().strip_prefix("//") {
            Some(r) => r.trim_start(),
            None => continue,
        };
        let body = match after_slashes.strip_prefix("@control") {
            Some(r) => r.trim(),
            None => continue,
        };
        if let Some(c) = parse_one(body, out.len() as u32) {
            out.push(c);
            if out.len() >= MAX_CONTROLS {
                break;
            }
        }
    }
    out
}

fn parse_one(s: &str, slot: u32) -> Option<Control> {
    let mut it = s.split_whitespace();
    let name = it.next()?.to_string();
    if !is_ident(&name) {
        return None;
    }
    let num = |o: Option<&str>| o.and_then(|v| v.parse::<f32>().ok());
    let kind_tok = it.next()?;
    match kind_tok {
        "float" | "int" => {
            let min = num(it.next())?;
            let max = num(it.next())?;
            let def = num(it.next()).unwrap_or(min);
            let kind = if kind_tok == "int" { ControlKind::Int } else { ControlKind::Float };
            Some(mk(name, kind, min, max, [def, 0.0, 0.0, 0.0], slot, vec![]))
        }
        "bool" => {
            let def = if matches!(it.next(), Some("true")) { 1.0 } else { 0.0 };
            Some(mk(name, ControlKind::Bool, 0.0, 1.0, [def, 0.0, 0.0, 0.0], slot, vec![]))
        }
        "color" => {
            let rgb = parse_hex(it.next()?)?;
            Some(mk(name, ControlKind::Color, 0.0, 1.0, [rgb[0], rgb[1], rgb[2], 1.0], slot, vec![]))
        }
        "vec2" => {
            let min = num(it.next())?;
            let max = num(it.next())?;
            let dx = num(it.next()).unwrap_or(min);
            let dy = num(it.next()).unwrap_or(min);
            Some(mk(name, ControlKind::Vec2, min, max, [dx, dy, 0.0, 0.0], slot, vec![]))
        }
        "enum" => {
            let options: Vec<String> = it.next()?.split(',').map(|s| s.to_string()).collect();
            let def = num(it.next()).unwrap_or(0.0);
            let max = options.len().saturating_sub(1) as f32;
            Some(mk(name, ControlKind::Enum, 0.0, max, [def, 0.0, 0.0, 0.0], slot, options))
        }
        "trigger" => Some(mk(name, ControlKind::Trigger, 0.0, 1.0, [0.0; 4], slot, vec![])),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn mk(
    name: String,
    kind: ControlKind,
    min: f32,
    max: f32,
    default: [f32; 4],
    slot: u32,
    options: Vec<String>,
) -> Control {
    Control { name, kind, min, max, default, slot, options }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

/// Generate the `#define name pm_user[slot].<lanes>` lines for the controls.
pub fn control_defines(controls: &[Control]) -> String {
    let mut s = String::new();
    for c in controls {
        let slot = c.slot;
        let line = match c.kind {
            ControlKind::Float | ControlKind::Int | ControlKind::Enum | ControlKind::Trigger => {
                format!("#define {} (pm_user[{slot}].x)\n", c.name)
            }
            ControlKind::Bool => format!("#define {} (pm_user[{slot}].x > 0.5)\n", c.name),
            ControlKind::Vec2 => format!("#define {} (pm_user[{slot}].xy)\n", c.name),
            ControlKind::Color => format!("#define {} (pm_user[{slot}].rgb)\n", c.name),
        };
        s.push_str(&line);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_kind() {
        let src = "// @control intensity float 0.0 2.0 0.75\n\
                   // @control steps int 1 8 4\n\
                   // @control mirror bool true\n\
                   // @control tint color #ff55cc\n\
                   // @control offset vec2 -1.0 1.0 0.0 0.5\n\
                   // @control mode enum a,b,c 1\n\
                   // @control kick trigger\n\
                   void mainImage(out vec4 c, in vec2 f) { c = vec4(0.0); }";
        let cs = parse_controls(src);
        assert_eq!(cs.len(), 7);
        assert_eq!(cs[0].name, "intensity");
        assert_eq!(cs[0].kind, ControlKind::Float);
        assert!((cs[0].default[0] - 0.75).abs() < 1e-6);
        assert_eq!(cs[2].kind, ControlKind::Bool);
        assert_eq!(cs[2].default[0], 1.0);
        assert_eq!(cs[3].kind, ControlKind::Color);
        assert!((cs[3].default[0] - 1.0).abs() < 1e-6);
        assert_eq!(cs[5].options, vec!["a", "b", "c"]);
        assert_eq!(cs[5].default[0], 1.0);
    }

    #[test]
    fn malformed_skipped_not_fatal() {
        let src = "// @control bad\n// @control ok float 0 1 0.5\n// @control 9bad float 0 1 0\n";
        let cs = parse_controls(src);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].name, "ok");
    }

    #[test]
    fn defines_map_lanes() {
        let cs = parse_controls("// @control tint color #ffffff\n// @control amt float 0 1 0.5\n");
        let d = control_defines(&cs);
        assert!(d.contains("#define tint (pm_user[0].rgb)"));
        assert!(d.contains("#define amt (pm_user[1].x)"));
    }

    #[test]
    fn respects_max_controls() {
        let mut src = String::new();
        for i in 0..40 {
            src.push_str(&format!("// @control c{i} float 0 1 0.5\n"));
        }
        assert_eq!(parse_controls(&src).len(), MAX_CONTROLS);
    }
}
