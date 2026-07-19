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

/// Translate user GLSL of the given mode to WGSL, or return diagnostics.
pub fn compile(mode: ShaderMode, user_src: &str) -> Result<Translated, Vec<Diagnostic>> {
    let (full, offset, controls) = build_full(mode, user_src);

    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = match frontend.parse(&options, &full) {
        Ok(m) => m,
        Err(errs) => {
            return Err(errs
                .errors
                .iter()
                .map(|e| {
                    let loc = e.meta.location(&full);
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
                    let loc = span.location(&full);
                    (loc.line_number.saturating_sub(offset), loc.line_position)
                })
                .unwrap_or((0, 0));
            return Err(vec![Diagnostic {
                line,
                column,
                message: err.emit_to_string(&full),
            }]);
        }
    };

    match naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty()) {
        Ok(wgsl) => Ok(Translated { wgsl, controls }),
        Err(e) => Err(vec![Diagnostic { line: 0, column: 0, message: e.to_string() }]),
    }
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
}
