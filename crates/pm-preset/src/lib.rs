//! `pm-preset` — the Milkdrop `.milk` preset engine.
//!
//! Parses a preset file, holds its [`PresetState`], and evaluates the
//! `per_frame_init` / `per_frame` / `per_pixel` equations each frame by driving
//! [`pm_eval`] with Milkdrop's named-variable model. This is the CPU side of a
//! preset; the GPU warp/composite passes are wired up by the renderer
//! (`pm-render` + `pm-shader`) in the orchestrator.
//!
//! # Example
//!
//! ```
//! use pm_preset::{Preset, FrameParams};
//! use pm_audio::FrameAudioData;
//!
//! let milk = "\
//! per_frame_init_1=`q1 = 0.5;
//! per_frame_1=`zoom = 1.0 + 0.1 * bass + q1;
//! ";
//! let mut preset = Preset::load(milk).unwrap();
//!
//! let mut audio = FrameAudioData::default();
//! audio.bass = 2.0;
//! preset.update_frame(FrameParams::default(), audio).unwrap();
//!
//! // zoom = 1.0 + 0.1*2.0 + 0.5 = 1.7
//! assert!((preset.state().zoom - 1.7).abs() < 1e-5);
//! ```

mod error;
mod parser;
mod per_frame;
mod per_pixel;
mod state;

pub use error::PresetError;
pub use parser::PresetFile;
pub use per_frame::PerFrameContext;
pub use per_pixel::{PerPixelContext, PerPixelOutput};
pub use state::{
    FrameParams, PresetState, CUSTOM_SHAPE_COUNT, CUSTOM_WAVEFORM_COUNT, Q_VAR_COUNT,
};

use pm_audio::FrameAudioData;

/// A loaded Milkdrop preset: state plus its compiled equation contexts.
pub struct Preset {
    state: PresetState,
    per_frame: PerFrameContext,
    per_pixel: PerPixelContext,
}

impl Preset {
    /// Parse and compile a preset from `.milk` file contents.
    pub fn load(content: &str) -> Result<Preset, PresetError> {
        let file = PresetFile::parse(content).ok_or(PresetError::InvalidFile)?;
        let mut state = PresetState::initialize(&file);

        let mut per_frame = PerFrameContext::new(&state)?;
        per_frame.evaluate_init(&mut state)?;
        let per_pixel = PerPixelContext::new(&state)?;

        Ok(Preset { state, per_frame, per_pixel })
    }

    /// Advance one frame: apply the inputs, run the per-frame code, and prepare
    /// the per-pixel context for the upcoming mesh evaluation.
    pub fn update_frame(
        &mut self,
        frame: FrameParams,
        audio: FrameAudioData,
    ) -> Result<(), PresetError> {
        self.state.frame = frame;
        self.state.audio = audio;

        self.per_frame.execute(&mut self.state)?;

        let q = self.per_frame.q_variables();
        self.per_pixel.begin_frame(&self.state, &q);
        Ok(())
    }

    /// Evaluate the per-pixel warp for one mesh vertex. Call after
    /// [`Preset::update_frame`]. `x`/`y` are mesh UVs in `[0,1]`; `rad`/`ang` are
    /// the vertex's polar coordinates.
    pub fn warp_vertex(
        &mut self,
        x: f64,
        y: f64,
        rad: f64,
        ang: f64,
    ) -> Result<PerPixelOutput, PresetError> {
        self.per_pixel.execute_vertex(&self.state, x, y, rad, ang)
    }

    /// True if the preset has per-pixel warp code.
    pub fn has_per_pixel_code(&self) -> bool {
        self.per_pixel.has_code()
    }

    /// The current preset state (updated after each [`Preset::update_frame`]).
    pub fn state(&self) -> &PresetState {
        &self.state
    }

    /// Raw warp shader source (Milkdrop HLSL), if the preset defines one.
    pub fn warp_shader_source(&self) -> &str {
        &self.state.warp_shader
    }

    /// Raw composite shader source (Milkdrop HLSL), if the preset defines one.
    pub fn composite_shader_source(&self) -> &str {
        &self.state.composite_shader
    }
}
