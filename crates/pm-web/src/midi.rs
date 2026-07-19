//! The stateful half of MIDI control: the live mapping store, MIDI-Learn state,
//! per-mapping runtime (soft-takeover / toggle-edge / smoothing memory), a
//! decoupled app-action queue, a value-update feed for UI reflection, and
//! diagnostics. The *pure* mapping math lives in `pm-midi`; the engine-facing
//! apply/read dispatch lives on `State` (it needs the compositor/clock/tempo).
//!
//! Mappings are stored globally (one versioned blob, persisted by JS to
//! localStorage) rather than embedded per-scene. Because layer/effect ids are
//! preserved across save/reload (see `compositor::import_scene`), an id-keyed
//! mapping survives reload and reorder. A mapping whose target no longer exists
//! is kept but reported `resolved:false` so the UI can flag/remove it.

use std::collections::HashMap;

use pm_midi::{Mapping, MappingMode, MappingSet, MessageType, MidiEvent, ANY_CHANNEL};

#[derive(Default)]
pub struct MidiDiag {
    pub last_type: String,
    pub last_device: String,
    pub last_channel: i32,
    pub last_selector: i32,
    pub last_value: i32,
    pub last_norm: f32,
    pub applied: u32,
    pub events: u64,
    pub ignored: u64,
}

pub struct MidiRouter {
    pub set: MappingSet,
    /// Target path currently awaiting a learn capture, if any.
    pub learn: Option<String>,
    pub diag: MidiDiag,
    // Per-mapping runtime (never serialized).
    pub pickup_engaged: HashMap<u32, bool>,
    pub prev_incoming: HashMap<u32, f32>,
    pub last_output: HashMap<u32, f32>,
    pub cc_on: HashMap<u32, bool>,
    /// App-side actions (paths starting with `app.`) fired by triggers, drained
    /// by JS each tick — keeps MIDI decoupled from button click handlers.
    pub actions: Vec<String>,
    /// Value changes from the last event, for the UI to reflect (drained by JS).
    pub updates: Vec<(String, f32)>,
    pub bool_updates: Vec<(String, bool)>,
}

impl MidiRouter {
    pub fn new() -> Self {
        MidiRouter {
            set: MappingSet::default(),
            learn: None,
            diag: MidiDiag::default(),
            pickup_engaged: HashMap::new(),
            prev_incoming: HashMap::new(),
            last_output: HashMap::new(),
            cc_on: HashMap::new(),
            actions: Vec::new(),
            updates: Vec::new(),
            bool_updates: Vec::new(),
        }
    }

    // --- Learn -------------------------------------------------------------

    pub fn learn_start(&mut self, path: String) {
        self.learn = Some(path);
    }
    pub fn learn_cancel(&mut self) {
        self.learn = None;
    }
    pub fn is_learning(&self) -> bool {
        self.learn.is_some()
    }

    /// Bind the learned control to the pending target, choosing the mapping mode
    /// from the target kind. Replaces any existing binding on the same physical
    /// control. Returns false (and stays in learn) if the event can't bind.
    pub fn add_learned(&mut self, path: String, device: &str, ev: &MidiEvent, kind: &str, min: f32, max: f32) -> bool {
        let (mt, selector) = match ev {
            MidiEvent::Cc { controller, .. } => (MessageType::Cc, *controller),
            MidiEvent::NoteOn { note, .. } => (MessageType::Note, *note),
            MidiEvent::PitchBend { .. } => (MessageType::PitchBend, 0),
            MidiEvent::NoteOff { .. } => return false,
        };
        let channel = ev.channel();
        self.set.remove_conflicts(device, channel, mt, selector);
        let mode = match kind {
            "toggle" => MappingMode::Toggle,
            "trigger" => MappingMode::Trigger,
            _ => MappingMode::Absolute,
        };
        let id = self.set.next_id();
        self.set.mappings.push(Mapping {
            id,
            target: path,
            device: device.to_string(),
            message_type: mt,
            channel,
            selector,
            in_min: 0,
            in_max: 127,
            out_min: min,
            out_max: max,
            invert: false,
            mode,
            pickup: kind == "continuous",
            curve: String::new(),
            smoothing: 0.0,
        });
        self.reset_runtime(id);
        self.learn = None;
        true
    }

    // --- Mapping edits -----------------------------------------------------

    pub fn clear(&mut self, id: u32) {
        self.set.remove(id);
        self.reset_runtime(id);
    }
    pub fn clear_all(&mut self) {
        self.set.mappings.clear();
        self.pickup_engaged.clear();
        self.prev_incoming.clear();
        self.last_output.clear();
        self.cc_on.clear();
    }
    pub fn reset_runtime(&mut self, id: u32) {
        self.pickup_engaged.remove(&id);
        self.prev_incoming.remove(&id);
        self.last_output.remove(&id);
        self.cc_on.remove(&id);
    }

    /// Edit one mapping field from the UI. Returns whether it applied.
    pub fn set_field(&mut self, id: u32, field: &str, value: &str) -> bool {
        // Runtime that depends on the changed field must reset (e.g. pickup).
        let mut reset = false;
        let ok = if let Some(m) = self.set.find_mut(id) {
            match field {
                "out_min" => value.parse().map(|v| m.out_min = v).is_ok(),
                "out_max" => value.parse().map(|v| m.out_max = v).is_ok(),
                "in_min" => value.parse::<u8>().map(|v| m.in_min = v).is_ok(),
                "in_max" => value.parse::<u8>().map(|v| m.in_max = v).is_ok(),
                "invert" => {
                    m.invert = value == "true" || value == "1";
                    true
                }
                "pickup" => {
                    m.pickup = value == "true" || value == "1";
                    reset = true;
                    true
                }
                "smoothing" => value.parse().map(|v: f32| m.smoothing = v.clamp(0.0, 0.999)).is_ok(),
                "curve" => {
                    m.curve = value.to_string();
                    true
                }
                "channel" => {
                    // "any" → omni, else 0..15
                    if value == "any" {
                        m.channel = ANY_CHANNEL;
                        true
                    } else {
                        value.parse::<u8>().map(|v| m.channel = v.min(15)).is_ok()
                    }
                }
                "mode" => {
                    if let Some(mode) = MappingMode::from_str(value) {
                        m.mode = mode;
                        reset = true;
                        true
                    } else {
                        false
                    }
                }
                "device" => {
                    m.device = value.to_string();
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if ok && reset {
            self.reset_runtime(id);
        }
        ok
    }

    // --- Persistence -------------------------------------------------------

    pub fn export(&self) -> String {
        self.set.to_json()
    }
    pub fn import(&mut self, json: &str) {
        self.set = MappingSet::from_json(json);
        self.pickup_engaged.clear();
        self.prev_incoming.clear();
        self.last_output.clear();
        self.cc_on.clear();
    }

    // --- Diagnostics feed --------------------------------------------------

    pub fn record_event(&mut self, device: &str, ev: &MidiEvent) {
        self.diag.events += 1;
        self.diag.last_type = ev.type_str().to_string();
        self.diag.last_device = device.to_string();
        self.diag.last_channel = ev.channel() as i32;
        self.diag.last_selector = ev.selector().map(|s| s as i32).unwrap_or(-1);
        self.diag.last_value = ev.value7() as i32;
        self.diag.last_norm = ev.value7() as f32 / 127.0;
    }

    pub fn take_actions(&mut self) -> String {
        if self.actions.is_empty() {
            return "[]".to_string();
        }
        let items: Vec<String> = self.actions.drain(..).map(|a| json_str(&a)).collect();
        format!("[{}]", items.join(","))
    }

    /// Drain the value-update feed (continuous + boolean) for UI reflection.
    pub fn take_updates(&mut self) -> String {
        let cont: Vec<String> = self
            .updates
            .drain(..)
            .map(|(p, v)| format!("{{\"path\":{},\"value\":{}}}", json_str(&p), v))
            .collect();
        let bools: Vec<String> = self
            .bool_updates
            .drain(..)
            .map(|(p, v)| format!("{{\"path\":{},\"bool\":{}}}", json_str(&p), v))
            .collect();
        format!("{{\"values\":[{}],\"bools\":[{}]}}", cont.join(","), bools.join(","))
    }

    pub fn diag_json(&self, enabled: bool, device_count: u32) -> String {
        format!(
            "{{\"enabled\":{},\"deviceCount\":{},\"learning\":{},\"learnTarget\":{},\
             \"events\":{},\"ignored\":{},\"applied\":{},\
             \"lastType\":{},\"lastDevice\":{},\"lastChannel\":{},\"lastSelector\":{},\"lastValue\":{},\"lastNorm\":{:.3}}}",
            enabled,
            device_count,
            self.learn.is_some(),
            self.learn.as_deref().map(json_str).unwrap_or_else(|| "null".into()),
            self.diag.events,
            self.diag.ignored,
            self.diag.applied,
            json_str(&self.diag.last_type),
            json_str(&self.diag.last_device),
            self.diag.last_channel,
            self.diag.last_selector,
            self.diag.last_value,
            self.diag.last_norm,
        )
    }

    /// All mappings as JSON. `is_valid` marks whether each target still resolves
    /// against the live registry (a deleted layer/effect → `resolved:false`).
    pub fn mappings_json(&self, is_valid: impl Fn(&str) -> bool) -> String {
        let items: Vec<String> = self
            .set
            .mappings
            .iter()
            .map(|m| {
                let ch = if m.channel == ANY_CHANNEL { "\"any\"".to_string() } else { m.channel.to_string() };
                let last = self.last_output.get(&m.id).copied();
                format!(
                    "{{\"id\":{},\"target\":{},\"resolved\":{},\"device\":{},\"channel\":{},\
                     \"messageType\":\"{}\",\"selector\":{},\"mode\":\"{}\",\"outMin\":{},\"outMax\":{},\
                     \"invert\":{},\"pickup\":{},\"curve\":{},\"smoothing\":{},\"engaged\":{},\"value\":{}}}",
                    m.id,
                    json_str(&m.target),
                    is_valid(&m.target),
                    json_str(&m.device),
                    ch,
                    match m.message_type {
                        MessageType::Cc => "cc",
                        MessageType::Note => "note",
                        MessageType::PitchBend => "pitchbend",
                    },
                    m.selector,
                    m.mode.as_str(),
                    m.out_min,
                    m.out_max,
                    m.invert,
                    m.pickup,
                    json_str(if m.curve.is_empty() { "linear" } else { &m.curve }),
                    m.smoothing,
                    self.pickup_engaged.get(&m.id).copied().unwrap_or(!m.pickup),
                    last.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }
}

/// Minimal JSON string escaper (mirrors the one in lib.rs; kept local so this
/// module stays self-contained).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
