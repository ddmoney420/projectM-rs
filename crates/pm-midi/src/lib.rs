//! Platform-neutral Web-MIDI mapping model.
//!
//! This crate owns the *pure* half of MIDI performance control: parsing raw
//! MIDI bytes into channel-voice events, the serializable [`Mapping`] model
//! (target path + device/channel/selector filter + range/invert/curve + one of
//! four [`MappingMode`]s + soft-takeover), and the value math (normalize →
//! invert → curve → scale, pickup engagement, event-blend smoothing).
//!
//! It deliberately knows nothing about the engine, the browser, or `wasm`. The
//! `pm-web` router owns the *stateful* half: the live device list, per-mapping
//! runtime (pickup/toggle/edge state), applying resolved values to the engine,
//! and reading current values back for soft-takeover. Keeping the math here
//! makes it native-unit-testable without a browser or real hardware.
//!
//! ## Value model (matches the rest of the app)
//! MIDI drives a parameter's **base** value; audio/LFO/beat modulation is
//! applied *after* (in the engine), so a MIDI-mapped control behaves exactly
//! like the same control moved by mouse. Nothing here bypasses the engine's
//! clamping.

use serde::{Deserialize, Serialize};

/// Channel sentinel meaning "any channel" (omni). Real MIDI channels are 0..15.
pub const ANY_CHANNEL: u8 = 0xFF;
/// Current mapping-store schema version. Bump on incompatible changes.
pub const MAPPING_SCHEMA_VERSION: u32 = 1;
/// A CC/pitch value at or above this (of 127) counts as "on" for toggle/
/// momentary/trigger modes driven by a continuous controller.
pub const ON_THRESHOLD: u8 = 64;

// ---------------------------------------------------------------------------
// Raw message parsing
// ---------------------------------------------------------------------------

/// A parsed MIDI channel-voice event we act on. System real-time (clock,
/// active-sensing), SysEx, program-change and aftertouch are intentionally not
/// represented — [`MidiEvent::from_raw`] returns `None` for them so they can
/// never drive a mapping or (importantly) hijack MIDI-Learn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    Cc { channel: u8, controller: u8, value: u8 },
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8, velocity: u8 },
    /// 14-bit bend value 0..16383, center 8192.
    PitchBend { channel: u8, value: u16 },
}

impl MidiEvent {
    /// Parse a 3-byte channel-voice message. Returns `None` for status bytes we
    /// don't handle (including all `>= 0xF0` system messages, so MIDI clock
    /// spam can't reach mappings or learn). A Note-On with velocity 0 is
    /// normalized to a Note-Off (running-status convention).
    pub fn from_raw(status: u8, d1: u8, d2: u8) -> Option<MidiEvent> {
        if status < 0x80 || status >= 0xF0 {
            return None; // data byte as status, or a system message
        }
        let channel = status & 0x0F;
        let d1 = d1 & 0x7F;
        let d2 = d2 & 0x7F;
        Some(match status & 0xF0 {
            0xB0 => MidiEvent::Cc { channel, controller: d1, value: d2 },
            0x90 if d2 == 0 => MidiEvent::NoteOff { channel, note: d1, velocity: 0 },
            0x90 => MidiEvent::NoteOn { channel, note: d1, velocity: d2 },
            0x80 => MidiEvent::NoteOff { channel, note: d1, velocity: d2 },
            0xE0 => MidiEvent::PitchBend { channel, value: ((d2 as u16) << 7) | d1 as u16 },
            _ => return None, // program change (0xC0), aftertouch (0xA0/0xD0)
        })
    }

    pub fn channel(&self) -> u8 {
        match self {
            MidiEvent::Cc { channel, .. }
            | MidiEvent::NoteOn { channel, .. }
            | MidiEvent::NoteOff { channel, .. }
            | MidiEvent::PitchBend { channel, .. } => *channel,
        }
    }

    /// A 7-bit "primary value" for normalization/diagnostics: CC value, note
    /// velocity, or the high 7 bits of a pitch bend. Note-Off reads as 0.
    pub fn value7(&self) -> u8 {
        match self {
            MidiEvent::Cc { value, .. } => *value,
            MidiEvent::NoteOn { velocity, .. } => *velocity,
            MidiEvent::NoteOff { .. } => 0,
            MidiEvent::PitchBend { value, .. } => (*value >> 7) as u8,
        }
    }

    /// Short type name for diagnostics.
    pub fn type_str(&self) -> &'static str {
        match self {
            MidiEvent::Cc { .. } => "cc",
            MidiEvent::NoteOn { .. } => "noteon",
            MidiEvent::NoteOff { .. } => "noteoff",
            MidiEvent::PitchBend { .. } => "pitchbend",
        }
    }

    /// The selector (CC number / note number) this event carries, if any.
    /// Pitch bend has no selector (there is one bend per channel).
    pub fn selector(&self) -> Option<u8> {
        match self {
            MidiEvent::Cc { controller, .. } => Some(*controller),
            MidiEvent::NoteOn { note, .. } | MidiEvent::NoteOff { note, .. } => Some(*note),
            MidiEvent::PitchBend { .. } => None,
        }
    }

    /// Whether this event is a meaningful target for MIDI-Learn (a knob turn or
    /// a pad press) — Note-Off and bare pitch-bend jitter are not.
    pub fn is_learnable(&self) -> bool {
        matches!(
            self,
            MidiEvent::Cc { .. } | MidiEvent::NoteOn { velocity: 1.., .. } | MidiEvent::PitchBend { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Mapping model
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    Cc,
    Note,
    PitchBend,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MappingMode {
    /// Continuous knob/fader → parameter across `out_min..out_max`.
    Absolute,
    /// A press flips a boolean target.
    Toggle,
    /// Press = on, release = off.
    Momentary,
    /// A qualifying press fires a one-shot action.
    Trigger,
}

impl MappingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MappingMode::Absolute => "absolute",
            MappingMode::Toggle => "toggle",
            MappingMode::Momentary => "momentary",
            MappingMode::Trigger => "trigger",
        }
    }
    pub fn from_str(s: &str) -> Option<MappingMode> {
        Some(match s {
            "absolute" => MappingMode::Absolute,
            "toggle" => MappingMode::Toggle,
            "momentary" => MappingMode::Momentary,
            "trigger" => MappingMode::Trigger,
            _ => return None,
        })
    }
}

fn default_channel() -> u8 {
    ANY_CHANNEL
}
fn default_in_max() -> u8 {
    127
}
fn default_pickup() -> bool {
    true
}
fn default_mode() -> MappingMode {
    MappingMode::Absolute
}

/// One serializable MIDI→target binding.
///
/// `device` empty means "any device". `channel` == [`ANY_CHANNEL`] means omni.
/// `selector` is the CC number or note number (ignored for pitch bend). The
/// input window `in_min..in_max` is in raw 7-bit units; `out_min..out_max` is
/// in the target parameter's own units. `curve` reuses the app's response
/// shapes ("linear"/"exp"/"log"/"scurve").
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Mapping {
    pub id: u32,
    pub target: String,
    #[serde(default)]
    pub device: String,
    pub message_type: MessageType,
    #[serde(default = "default_channel")]
    pub channel: u8,
    #[serde(default)]
    pub selector: u8,
    #[serde(default)]
    pub in_min: u8,
    #[serde(default = "default_in_max")]
    pub in_max: u8,
    pub out_min: f32,
    pub out_max: f32,
    #[serde(default)]
    pub invert: bool,
    #[serde(default = "default_mode")]
    pub mode: MappingMode,
    #[serde(default = "default_pickup")]
    pub pickup: bool,
    #[serde(default)]
    pub curve: String,
    #[serde(default)]
    pub smoothing: f32,
}

impl Mapping {
    /// Does an incoming event on `device` match this mapping's filter?
    pub fn matches(&self, ev: &MidiEvent, device: &str) -> bool {
        if !self.device.is_empty() && self.device != device {
            return false;
        }
        if self.channel != ANY_CHANNEL && self.channel != ev.channel() {
            return false;
        }
        match (self.message_type, ev) {
            (MessageType::Cc, MidiEvent::Cc { controller, .. }) => *controller == self.selector,
            (MessageType::Note, MidiEvent::NoteOn { note, .. })
            | (MessageType::Note, MidiEvent::NoteOff { note, .. }) => *note == self.selector,
            (MessageType::PitchBend, MidiEvent::PitchBend { .. }) => true,
            _ => false,
        }
    }

    /// Normalized 0..1 signal for this event (after the input window, invert,
    /// and response curve). Used both for the output value and pickup.
    pub fn normalized(&self, ev: &MidiEvent) -> f32 {
        let mut n = normalize(ev.value7(), self.in_min, self.in_max);
        if self.invert {
            n = 1.0 - n;
        }
        curve_apply(&self.curve, n)
    }

    /// The absolute output value for this event, scaled into `out_min..out_max`.
    pub fn output(&self, ev: &MidiEvent) -> f32 {
        self.out_min + self.normalized(ev) * (self.out_max - self.out_min)
    }

    /// Absolute pickup threshold in output units (a fraction of the range).
    pub fn pickup_threshold(&self) -> f32 {
        (self.out_max - self.out_min).abs().max(1e-6) * PICKUP_FRACTION
    }
}

/// Soft-takeover engages when the hardware value comes within this fraction of
/// the parameter's range of the current value (or crosses it).
pub const PICKUP_FRACTION: f32 = 0.03;

/// Map a raw 7-bit value into 0..1 over the input window (order-independent).
pub fn normalize(value: u8, in_min: u8, in_max: u8) -> f32 {
    let lo = in_min.min(in_max) as f32;
    let hi = in_min.max(in_max) as f32;
    if (hi - lo).abs() < f32::EPSILON {
        return 0.0;
    }
    ((value as f32 - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Apply a named response curve (reusing [`pm_params::Curve`]).
pub fn curve_apply(curve: &str, x: f32) -> f32 {
    let c = match curve {
        "exp" => pm_params::Curve::Exp,
        "log" => pm_params::Curve::Log,
        "scurve" => pm_params::Curve::SCurve,
        _ => pm_params::Curve::Linear,
    };
    c.apply(x)
}

/// Soft-takeover decision. Once `engaged`, stays engaged. Otherwise engages
/// when `incoming` is within `threshold` of `current`, or when it crossed
/// `current` since `prev` (so a fast sweep past the value also catches).
/// All arguments are in the same (output) units.
pub fn pickup_engage(engaged: bool, prev: Option<f32>, incoming: f32, current: f32, threshold: f32) -> bool {
    if engaged {
        return true;
    }
    if (incoming - current).abs() <= threshold {
        return true;
    }
    if let Some(p) = prev {
        // Crossed if `current` lies between the previous and current incoming.
        if (p - current) == 0.0 || (incoming - current) == 0.0 {
            return true;
        }
        if (p - current).is_sign_positive() != (incoming - current).is_sign_positive() {
            return true;
        }
    }
    false
}

/// One-pole event-blend smoothing toward `target` (0 = instant, →1 = slower).
/// Applied per event; since faders emit dense CC streams this tames jitter
/// without a separate timer. Non-continuous modes ignore smoothing.
pub fn smooth_step(prev: f32, target: f32, smoothing: f32) -> f32 {
    let s = smoothing.clamp(0.0, 0.999);
    prev + (target - prev) * (1.0 - s)
}

/// Whether a continuous value counts as an "on"/press.
pub fn is_on(value: u8) -> bool {
    value >= ON_THRESHOLD
}

// ---------------------------------------------------------------------------
// Persisted mapping set
// ---------------------------------------------------------------------------

/// The full serializable mapping store. Versioned so old blobs can migrate.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MappingSet {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
}

impl Default for MappingSet {
    fn default() -> Self {
        MappingSet { version: MAPPING_SCHEMA_VERSION, mappings: Vec::new() }
    }
}

impl MappingSet {
    /// Parse from JSON, tolerating a missing/old version by migrating. A totally
    /// invalid blob yields an empty set (never panics on startup).
    pub fn from_json(s: &str) -> MappingSet {
        match serde_json::from_str::<MappingSet>(s) {
            Ok(mut m) => {
                m.migrate();
                m
            }
            Err(_) => MappingSet::default(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"version\":1,\"mappings\":[]}".into())
    }

    /// Bring an older blob up to the current schema. Version 0 (pre-versioning)
    /// is treated as v1 with no field changes.
    pub fn migrate(&mut self) {
        if self.version == 0 {
            self.version = MAPPING_SCHEMA_VERSION;
        }
        // Future: per-version field migrations go here.
        self.version = MAPPING_SCHEMA_VERSION;
    }

    pub fn next_id(&self) -> u32 {
        self.mappings.iter().map(|m| m.id).max().unwrap_or(0) + 1
    }

    pub fn find(&self, id: u32) -> Option<&Mapping> {
        self.mappings.iter().find(|m| m.id == id)
    }
    pub fn find_mut(&mut self, id: u32) -> Option<&mut Mapping> {
        self.mappings.iter_mut().find(|m| m.id == id)
    }
    pub fn remove(&mut self, id: u32) {
        self.mappings.retain(|m| m.id != id);
    }
    /// Remove any existing mapping bound to the same physical control (device +
    /// channel + type + selector) so a re-learn replaces rather than stacks.
    pub fn remove_conflicts(&mut self, device: &str, channel: u8, mt: MessageType, selector: u8) {
        self.mappings
            .retain(|m| !(m.device == device && m.channel == channel && m.message_type == mt && m.selector == selector));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(ch: u8, ctrl: u8, val: u8) -> MidiEvent {
        MidiEvent::Cc { channel: ch, controller: ctrl, value: val }
    }

    #[test]
    fn parse_cc_note_bend_and_ignores_realtime() {
        assert_eq!(MidiEvent::from_raw(0xB0, 7, 100), Some(cc(0, 7, 100)));
        assert_eq!(MidiEvent::from_raw(0x92, 60, 120), Some(MidiEvent::NoteOn { channel: 2, note: 60, velocity: 120 }));
        // Note-On velocity 0 → Note-Off.
        assert_eq!(MidiEvent::from_raw(0x90, 60, 0), Some(MidiEvent::NoteOff { channel: 0, note: 60, velocity: 0 }));
        assert_eq!(MidiEvent::from_raw(0x80, 60, 40), Some(MidiEvent::NoteOff { channel: 0, note: 60, velocity: 40 }));
        assert_eq!(MidiEvent::from_raw(0xE0, 0x00, 0x40), Some(MidiEvent::PitchBend { channel: 0, value: 8192 }));
        // System real-time (clock/sensing) and program-change ignored.
        assert_eq!(MidiEvent::from_raw(0xF8, 0, 0), None);
        assert_eq!(MidiEvent::from_raw(0xFE, 0, 0), None);
        assert_eq!(MidiEvent::from_raw(0xC0, 5, 0), None);
        assert_eq!(MidiEvent::from_raw(0x40, 0, 0), None); // data byte as status
    }

    #[test]
    fn normalize_and_output() {
        assert!((normalize(127, 0, 127) - 1.0).abs() < 1e-6);
        assert!((normalize(0, 0, 127) - 0.0).abs() < 1e-6);
        assert!((normalize(64, 0, 127) - 0.5039).abs() < 1e-3);
        // degenerate window
        assert_eq!(normalize(50, 10, 10), 0.0);
    }

    fn base_map() -> Mapping {
        Mapping {
            id: 1,
            target: "layer.2.opacity".into(),
            device: String::new(),
            message_type: MessageType::Cc,
            channel: ANY_CHANNEL,
            selector: 7,
            in_min: 0,
            in_max: 127,
            out_min: 0.0,
            out_max: 1.0,
            invert: false,
            mode: MappingMode::Absolute,
            pickup: true,
            curve: String::new(),
            smoothing: 0.0,
        }
    }

    #[test]
    fn output_range_and_invert() {
        let mut m = base_map();
        m.out_min = 0.0;
        m.out_max = 1.5;
        assert!((m.output(&cc(0, 7, 127)) - 1.5).abs() < 1e-6);
        assert!((m.output(&cc(0, 7, 0)) - 0.0).abs() < 1e-6);
        m.invert = true;
        assert!((m.output(&cc(0, 7, 0)) - 1.5).abs() < 1e-6);
        assert!((m.output(&cc(0, 7, 127)) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn curve_shapes_output() {
        let mut m = base_map();
        m.curve = "exp".into();
        // exp(0.5)=0.25 → mid input gives quarter output.
        let v = m.output(&cc(0, 7, 64));
        assert!(v < 0.3, "exp curve should pull midpoint down: {v}");
    }

    #[test]
    fn channel_and_selector_and_device_filter() {
        let mut m = base_map();
        m.channel = 3;
        assert!(m.matches(&cc(3, 7, 10), "dev"));
        assert!(!m.matches(&cc(4, 7, 10), "dev")); // wrong channel
        assert!(!m.matches(&cc(3, 8, 10), "dev")); // wrong selector
        m.channel = ANY_CHANNEL;
        assert!(m.matches(&cc(9, 7, 10), "dev")); // omni
        m.device = "launchctl".into();
        assert!(m.matches(&cc(0, 7, 10), "launchctl"));
        assert!(!m.matches(&cc(0, 7, 10), "other")); // wrong device
        // wrong message type
        assert!(!m.matches(&MidiEvent::NoteOn { channel: 0, note: 7, velocity: 100 }, "launchctl"));
    }

    #[test]
    fn pickup_waits_then_engages() {
        let m = base_map();
        let thr = m.pickup_threshold(); // 3% of 1.0
        // current 0.8, incoming 0.2 → far, not engaged
        assert!(!pickup_engage(false, None, 0.2, 0.8, thr));
        // sweeping up to within threshold engages
        assert!(pickup_engage(false, Some(0.5), 0.79, 0.8, thr));
        // crossing the current value engages even if it overshoots
        assert!(pickup_engage(false, Some(0.7), 0.9, 0.8, thr));
        // once engaged, stays engaged
        assert!(pickup_engage(true, None, 0.0, 1.0, thr));
    }

    #[test]
    fn smoothing_moves_partway() {
        let a = smooth_step(0.0, 1.0, 0.0);
        assert!((a - 1.0).abs() < 1e-6); // instant
        let b = smooth_step(0.0, 1.0, 0.5);
        assert!((b - 0.5).abs() < 1e-6);
        let c = smooth_step(0.0, 1.0, 0.9);
        assert!(c < 0.2);
    }

    #[test]
    fn on_threshold() {
        assert!(!is_on(63));
        assert!(is_on(64));
        assert!(is_on(127));
    }

    #[test]
    fn learnable_filters_noise() {
        assert!(cc(0, 1, 5).is_learnable());
        assert!(MidiEvent::NoteOn { channel: 0, note: 1, velocity: 1 }.is_learnable());
        assert!(!MidiEvent::NoteOff { channel: 0, note: 1, velocity: 0 }.is_learnable());
    }

    #[test]
    fn mapping_set_round_trip_and_defaults() {
        let mut set = MappingSet::default();
        set.mappings.push(base_map());
        let json = set.to_json();
        let back = MappingSet::from_json(&json);
        assert_eq!(back.mappings.len(), 1);
        assert_eq!(back.mappings[0].target, "layer.2.opacity");
        assert_eq!(back.version, MAPPING_SCHEMA_VERSION);
    }

    #[test]
    fn from_json_tolerates_missing_fields_and_migrates() {
        // Minimal blob: no version, sparse mapping relying on defaults.
        let json = r#"{"mappings":[{"id":5,"target":"global.speed","message_type":"cc","selector":20,"out_min":0.0,"out_max":2.0}]}"#;
        let set = MappingSet::from_json(json);
        assert_eq!(set.version, MAPPING_SCHEMA_VERSION);
        let m = &set.mappings[0];
        assert_eq!(m.channel, ANY_CHANNEL); // defaulted to omni
        assert_eq!(m.in_max, 127);
        assert!(m.pickup); // defaulted on
        assert_eq!(m.mode, MappingMode::Absolute);
    }

    #[test]
    fn from_json_garbage_is_empty() {
        assert_eq!(MappingSet::from_json("not json").mappings.len(), 0);
        assert_eq!(MappingSet::from_json("").mappings.len(), 0);
    }

    #[test]
    fn next_id_and_remove_conflicts() {
        let mut set = MappingSet::default();
        set.mappings.push(base_map());
        assert_eq!(set.next_id(), 2);
        set.remove_conflicts("", ANY_CHANNEL, MessageType::Cc, 7);
        assert_eq!(set.mappings.len(), 0);
    }
}
