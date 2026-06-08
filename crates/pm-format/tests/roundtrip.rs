//! Round-trip and import tests for the native preset format.

use pm_format::NativePreset;

const SAMPLE: &str = "\
MILKDROP_PRESET_VERSION=201
name=Test Preset
zoom=1.010
rot=0.040
fDecay=0.980
nWaveMode=0
per_frame_1=`wave_r = 0.5 + 0.5*sin(time);
per_frame_2=`wave_g = 0.5 + 0.5*sin(time*1.3);
per_pixel_1=`zoom = zoom + 0.01*sin(rad*10);
warp_1=`shader_body
warp_2=`{
warp_3=`ret = tex2D(sampler_main, uv).xyz;
warp_4=`}
wavecode_0_enabled=1
wave_0_per_frame1=`wave_r = 1.0;
wave_0_per_point1=`x = 0.5 + 0.4*sin(t);
";

#[test]
fn splits_scalars_and_code() {
    let np = NativePreset::from_milk(SAMPLE).unwrap();

    // Scalars (lowercased), code excluded.
    assert_eq!(np.scalars.get("zoom").map(String::as_str), Some("1.010"));
    assert_eq!(np.scalars.get("name").map(String::as_str), Some("Test Preset"));
    assert_eq!(np.scalars.get("wavecode_0_enabled").map(String::as_str), Some("1"));
    assert!(!np.scalars.contains_key("per_frame_1"), "code is not a scalar");

    // Code blocks grouped by exact prefix, backtick stripped, in index order.
    assert_eq!(
        np.code.get("per_frame_").unwrap(),
        &["wave_r = 0.5 + 0.5*sin(time);", "wave_g = 0.5 + 0.5*sin(time*1.3);"]
    );
    assert_eq!(np.code.get("warp_").unwrap().len(), 4);
    assert_eq!(np.code.get("warp_").unwrap()[0], "shader_body");
    assert_eq!(np.code.get("wave_0_per_frame").unwrap(), &["wave_r = 1.0;"]);
    assert_eq!(np.code.get("wave_0_per_point").unwrap(), &["x = 0.5 + 0.4*sin(t);"]);
}

#[test]
fn name_accessor() {
    let np = NativePreset::from_milk(SAMPLE).unwrap();
    assert_eq!(np.name(), Some("Test Preset"));
}

#[test]
fn milk_import_is_idempotent() {
    // from_milk -> to_milk -> from_milk yields the same structured preset.
    let a = NativePreset::from_milk(SAMPLE).unwrap();
    let b = NativePreset::from_milk(&a.to_milk()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn pmp_roundtrips() {
    let np = NativePreset::from_milk(SAMPLE).unwrap();
    let text = np.to_pmp();
    let back = NativePreset::from_pmp(&text);
    assert_eq!(np, back, "\n--- .pmp ---\n{text}");
}

#[test]
fn pmp_is_human_readable() {
    let np = NativePreset::from_milk(SAMPLE).unwrap();
    let text = np.to_pmp();
    assert!(text.contains("[scalars]"));
    assert!(text.contains("[code per_frame_]"));
    assert!(text.contains("    wave_r = 0.5 + 0.5*sin(time);"));
    assert!(text.starts_with("# Test Preset"));
}

#[test]
fn imported_preset_loads_into_engine() {
    // The reconstructed .milk drives the real engine unchanged.
    let np = NativePreset::from_milk(SAMPLE).unwrap();
    let preset = np.into_preset().expect("reconstructed .milk should load");
    // And it behaves like loading the original directly.
    let direct = pm_preset::Preset::load(SAMPLE).expect("original loads");
    assert_eq!(preset.state().zoom, direct.state().zoom);
    assert_eq!(preset.state().decay, direct.state().decay);
}

#[test]
fn empty_code_lines_survive_pmp_roundtrip() {
    // A genuinely empty code line (vs. a separator) must be preserved.
    let mut np = NativePreset::default();
    np.scalars.insert("zoom".into(), "1.0".into());
    np.code.insert("per_frame_".into(), vec!["a = 1;".into(), String::new(), "b = 2;".into()]);
    let back = NativePreset::from_pmp(&np.to_pmp());
    assert_eq!(np, back);
}
