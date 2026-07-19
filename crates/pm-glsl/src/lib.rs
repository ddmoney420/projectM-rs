//! GLSL → WGSL translation for the pm-web live shader console.
//!
//! Two authoring modes, one compilation path:
//!   * [`ShaderMode::Shadertoy`] — the user writes `mainImage(out vec4, in vec2)`
//!     and we generate the wrapper + entry point.
//!   * [`ShaderMode::Raw`] — the user writes a full fragment shader (their own
//!     `main` + `out`) against the documented binding contract below.
//!
//! Both are prefixed with the same PRELUDE (uniform block + audio/`iChannel`
//! samplers), translated to WGSL with `naga` (so we get line/column
//! diagnostics), and handed to wgpu as a WGSL string. This crate is
//! platform-neutral and unit-tested on native — no wgpu device required.

mod controls;
mod prelude;

pub use controls::{control_defines, parse_controls, Control, ControlKind, MAX_CONTROLS};
pub use prelude::{ShaderUniforms, AUDIO_TEX_HEIGHT, AUDIO_TEX_WIDTH, PRELUDE};

/// Which authoring dialect the user source is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderMode {
    /// Shadertoy `void mainImage(out vec4 fragColor, in vec2 fragCoord)`.
    Shadertoy,
    /// A full GLSL fragment shader with its own `main` and `out`.
    Raw,
}

/// A compiler diagnostic mapped back to the *user's* source coordinates
/// (1-based line/column; the wrapper prelude is subtracted out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1-based line in the user's source (0 if it falls in generated code).
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    pub message: String,
}

const SHADERTOY_TAIL: &str = "
void main() {
\tvec4 pm_c = vec4(0.0);
\t// Shadertoy fragCoord: bottom-left origin (flip Vulkan's top-left).
\tvec2 pm_fc = vec2(gl_FragCoord.x, iResolution.y - gl_FragCoord.y);
\tmainImage(pm_c, pm_fc);
\tpm_fragColor = pm_c;
}
";

/// Assemble the full GLSL (prelude + control `#define`s + user code + wrapper),
/// returning it with the line offset to subtract from diagnostics and the
/// parsed user controls.
fn build_full(mode: ShaderMode, user_src: &str) -> (String, u32, Vec<Control>) {
    let controls = parse_controls(user_src);
    let defines = control_defines(&controls);
    let prefix = match mode {
        ShaderMode::Shadertoy => {
            format!("{PRELUDE}\n{defines}layout(location = 0) out vec4 pm_fragColor;\n")
        }
        ShaderMode::Raw => format!("{PRELUDE}\n{defines}"),
    };
    let offset = prefix.matches('\n').count() as u32;
    let full = match mode {
        ShaderMode::Shadertoy => format!("{prefix}{user_src}{SHADERTOY_TAIL}"),
        ShaderMode::Raw => format!("{prefix}{user_src}\n"),
    };
    (full, offset, controls)
}

/// Successful translation: the WGSL to hand to wgpu, plus parsed user controls.
#[derive(Debug, Clone)]
pub struct Translated {
    pub wgsl: String,
    pub controls: Vec<Control>,
}

/// Translate user GLSL of the given mode to WGSL, or return diagnostics. The
/// user's own `@control` declarations are parsed and used for the `#define`s.
pub fn compile(mode: ShaderMode, user_src: &str) -> Result<Translated, Vec<Diagnostic>> {
    let (full, offset, controls) = build_full(mode, user_src);
    let wgsl = translate(&full, offset)?;
    Ok(Translated { wgsl, controls })
}

/// Translate a pass with an EXTERNALLY-supplied control set — used by the
/// multipass project so every pass shares one project-level control registry
/// (each control's `slot` is the project slot, so the same `@control feedback`
/// drives Buffer A and Image via the same `pm_user` slot). The pass's own
/// `@control` comment lines are ignored here; the caller aggregates them.
pub fn compile_with(mode: ShaderMode, user_src: &str, controls: &[Control]) -> Result<Translated, Vec<Diagnostic>> {
    let defines = control_defines(controls);
    let prefix = match mode {
        ShaderMode::Shadertoy => format!("{PRELUDE}\n{defines}layout(location = 0) out vec4 pm_fragColor;\n"),
        ShaderMode::Raw => format!("{PRELUDE}\n{defines}"),
    };
    let offset = prefix.matches('\n').count() as u32;
    let full = match mode {
        ShaderMode::Shadertoy => format!("{prefix}{user_src}{SHADERTOY_TAIL}"),
        ShaderMode::Raw => format!("{prefix}{user_src}\n"),
    };
    let wgsl = translate(&full, offset)?;
    Ok(Translated { wgsl, controls: controls.to_vec() })
}

/// The shared naga parse → validate → WGSL step. `offset` is the number of
/// prelude lines to subtract so diagnostics map to the user's own source.
fn translate(full: &str, offset: u32) -> Result<String, Vec<Diagnostic>> {
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = match frontend.parse(&options, full) {
        Ok(m) => m,
        Err(errs) => {
            return Err(errs
                .errors
                .iter()
                .map(|e| {
                    let loc = e.meta.location(full);
                    Diagnostic {
                        line: loc.line_number.saturating_sub(offset),
                        column: loc.line_position,
                        message: e.kind.to_string(),
                    }
                })
                .collect());
        }
    };

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    let info = match validator.validate(&module) {
        Ok(info) => info,
        Err(err) => {
            let (line, column) = err
                .spans()
                .next()
                .map(|(span, _)| {
                    let loc = span.location(full);
                    (loc.line_number.saturating_sub(offset), loc.line_position)
                })
                .unwrap_or((0, 0));
            return Err(vec![Diagnostic { line, column, message: err.emit_to_string(full) }]);
        }
    };

    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| vec![Diagnostic { line: 0, column: 0, message: e.to_string() }])
}

/// Aggregate `@control` declarations from several passes into one project-level
/// registry. Controls are matched by name (first-seen order wins) and assigned
/// project slots `0..MAX_CONTROLS`. A later declaration of the same name with a
/// different kind/range/options is reported as a conflict (but the first
/// definition is kept so the project still runs). Returns `(merged, conflicts)`.
pub fn merge_controls(pass_controls: &[Vec<Control>]) -> (Vec<Control>, Vec<String>) {
    let mut merged: Vec<Control> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();
    for list in pass_controls {
        for c in list {
            if let Some(existing) = merged.iter().find(|m| m.name == c.name) {
                if existing.kind != c.kind || existing.min != c.min || existing.max != c.max || existing.options != c.options {
                    let msg = format!("control '{}' redeclared with a conflicting definition", c.name);
                    if !conflicts.contains(&msg) {
                        conflicts.push(msg);
                    }
                }
            } else if merged.len() < MAX_CONTROLS {
                let mut nc = c.clone();
                nc.slot = merged.len() as u32; // reassign to a project slot
                merged.push(nc);
            }
        }
    }
    (merged, conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLASMA: &str = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float v = sin(uv.x * 10.0 + iTime) + cos(uv.y * 10.0 + iTime);
    fragColor = vec4(0.5 + 0.5 * sin(v), 0.5 + 0.5 * cos(v), 0.5, 1.0);
}
"#;

    #[test]
    fn shadertoy_plasma_translates() {
        let out = compile(ShaderMode::Shadertoy, PLASMA).expect("should translate");
        assert!(out.wgsl.contains("fn main"), "expected an entry point");
        // Dump the WGSL so we can see naga's exact binding layout.
        println!("---- WGSL ----\n{}\n--------------", out.wgsl);
    }

    #[test]
    fn audio_sampler_translates() {
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float fft = texture(iChannel0, vec2(uv.x, 0.25)).x;
    fragColor = vec4(fft, iBass, iTreb, 1.0);
}
"#;
        let out = compile(ShaderMode::Shadertoy, src).expect("audio sampler should translate");
        println!("---- WGSL (audio) ----\n{}\n--------------", out.wgsl);
    }

    #[test]
    fn invalid_glsl_reports_user_line() {
        // `iTime` misspelled; error should map near user line 3.
        let bad = "void mainImage(out vec4 c, in vec2 fc) {\n    vec2 uv = fc;\n    c = vec4(uv, iTiiime, 1.0);\n}\n";
        let diags = compile(ShaderMode::Shadertoy, bad).expect_err("should fail");
        assert!(!diags.is_empty());
        println!("diagnostics: {diags:?}");
    }

    #[test]
    fn compile_with_project_controls() {
        // A pass that uses `feedback` which is declared at the project level
        // (project slot 3), not in this pass's own source.
        let src = "void mainImage(out vec4 c, in vec2 f){ c = vec4(feedback, 0.0, 0.0, 1.0); }\n";
        let ctrl = parse_controls("// @control feedback float 0.0 1.0 0.5\n");
        let mut project = ctrl.clone();
        project[0].slot = 3; // project slot
        let out = compile_with(ShaderMode::Shadertoy, src, &project).expect("uses project control");
        assert!(out.wgsl.contains("pm_user[3]"), "define should use project slot 3");
    }

    #[test]
    fn merge_controls_shares_by_name_and_reslots() {
        let a = parse_controls("// @control feedback float 0.0 1.0 0.5\n// @control speed float 0.0 4.0 1.0\n");
        let b = parse_controls("// @control feedback float 0.0 1.0 0.5\n// @control hue float 0.0 1.0 0.0\n");
        let (merged, conflicts) = merge_controls(&[a, b]);
        assert!(conflicts.is_empty());
        // feedback (0), speed (1), hue (2) — shared feedback appears once.
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "feedback");
        assert_eq!(merged[0].slot, 0);
        assert_eq!(merged[2].name, "hue");
        assert_eq!(merged[2].slot, 2);
    }

    #[test]
    fn merge_controls_detects_conflict() {
        let a = parse_controls("// @control k float 0.0 1.0 0.5\n");
        let b = parse_controls("// @control k float 0.0 8.0 1.0\n"); // different range
        let (merged, conflicts) = merge_controls(&[a, b]);
        assert_eq!(merged.len(), 1); // first definition kept
        assert_eq!(conflicts.len(), 1);
    }
}
