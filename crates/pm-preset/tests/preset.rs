//! Integration tests: load `.milk` presets and verify equation evaluation.

use pm_audio::FrameAudioData;
use pm_preset::{FrameParams, Preset};

fn audio_with_bass(bass: f32) -> FrameAudioData {
    FrameAudioData { bass, mid: bass, treb: bass, ..FrameAudioData::default() }
}

#[test]
fn loads_defaults_and_overrides() {
    // Only `decay` is overridden; everything else keeps Milkdrop defaults.
    let preset = Preset::load("fDecay=0.95\nzoom=1.5").unwrap();
    let s = preset.state();
    assert_eq!(s.decay, 0.95);
    assert_eq!(s.zoom, 1.5);
    assert_eq!(s.gamma_adj, 2.0); // default
    assert_eq!(s.wave_alpha, 0.8); // default
    assert!(s.maximize_wave_color); // default true
}

#[test]
fn per_frame_init_seeds_q_variables() {
    let milk = "per_frame_init_1=`q1 = 0.5;\nper_frame_init_2=`q2 = q1 * 4;";
    let preset = Preset::load(milk).unwrap();
    assert_eq!(preset.state().frame_q_variables[0], 0.5);
    assert_eq!(preset.state().frame_q_variables[1], 2.0);
}

#[test]
fn per_frame_code_reads_audio_and_q() {
    let milk = "\
per_frame_init_1=`q1 = 0.5;
per_frame_1=`zoom = 1.0 + 0.1 * bass + q1;
";
    let mut preset = Preset::load(milk).unwrap();
    preset.update_frame(FrameParams::default(), audio_with_bass(2.0)).unwrap();
    // 1.0 + 0.1*2.0 + 0.5 = 1.7
    assert!((preset.state().zoom - 1.7).abs() < 1e-5, "zoom = {}", preset.state().zoom);
}

#[test]
fn q_variables_reset_each_frame_to_post_init() {
    // q1 starts at 1 after init; per-frame increments it. Each frame must start
    // from the post-init value, so q1 ends at 2 every frame (not accumulating).
    let milk = "\
per_frame_init_1=`q1 = 1.0;
per_frame_1=`q1 = q1 + 1.0; zoom = q1;
";
    let mut preset = Preset::load(milk).unwrap();
    for _ in 0..5 {
        preset.update_frame(FrameParams::default(), FrameAudioData::default()).unwrap();
        assert_eq!(preset.state().zoom, 2.0);
    }
}

#[test]
fn per_pixel_warps_per_vertex() {
    let milk = "\
per_frame_1=`zoom = 1.0;
per_pixel_1=`zoom = zoom + rad * 0.5;
";
    let mut preset = Preset::load(milk).unwrap();
    assert!(preset.has_per_pixel_code());
    preset.update_frame(FrameParams::default(), FrameAudioData::default()).unwrap();

    // Center vertex (rad 0) keeps zoom 1.0; an edge vertex (rad 1) gets +0.5.
    let center = preset.warp_vertex(0.5, 0.5, 0.0, 0.0).unwrap();
    let edge = preset.warp_vertex(1.0, 1.0, 1.0, 0.785).unwrap();
    assert!((center.zoom - 1.0).abs() < 1e-9);
    assert!((edge.zoom - 1.5).abs() < 1e-9);
}

#[test]
fn time_and_frame_inputs_flow_through() {
    let milk = "per_frame_1=`zoom = time * 2.0 + frame;";
    let mut preset = Preset::load(milk).unwrap();
    let frame = FrameParams { time: 3.0, frame: 10, ..FrameParams::default() };
    preset.update_frame(frame, FrameAudioData::default()).unwrap();
    // 3.0*2 + 10 = 16
    assert!((preset.state().zoom - 16.0).abs() < 1e-5);
}

#[test]
fn motion_vars_persist_into_state_for_rendering() {
    let milk = "per_frame_1=`rot = 0.25; warp = 2.0; cx = 0.3;";
    let mut preset = Preset::load(milk).unwrap();
    preset.update_frame(FrameParams::default(), FrameAudioData::default()).unwrap();
    let s = preset.state();
    assert_eq!(s.rot, 0.25);
    assert_eq!(s.warp_amount, 2.0);
    assert_eq!(s.rot_cx, 0.3);
}

#[test]
fn shader_sources_are_extracted() {
    let milk = "\
warp_1=`shader_body
warp_2=`{
warp_3=`ret = GetPixel(uv);
warp_4=`}
comp_1=`shader_body { ret = float3(1,0,0); }
";
    let preset = Preset::load(milk).unwrap();
    assert!(preset.warp_shader_source().contains("shader_body"));
    assert!(preset.warp_shader_source().contains("GetPixel"));
    assert!(preset.composite_shader_source().contains("float3(1,0,0)"));
}

#[test]
fn rejects_garbage() {
    assert!(Preset::load("\0\0\0").is_err());
}
