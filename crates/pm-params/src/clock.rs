//! The controlled visual clock. Drives `iTime`/`iTimeDelta` for custom shaders
//! and (where wired) Milkdrop, and can be paused, speed-scaled, and reset —
//! structured so reverse time can be added later.

/// A visual time accumulator with pause and speed scaling.
#[derive(Debug, Clone)]
pub struct VisualClock {
    time: f32,
    scale: f32,
    paused: bool,
    last_delta: f32,
}

impl Default for VisualClock {
    fn default() -> Self {
        VisualClock { time: 0.0, scale: 1.0, paused: false, last_delta: 0.0 }
    }
}

impl VisualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by a real per-frame step. Returns the *visual* delta applied
    /// (0 while paused, `real_dt * scale` otherwise). `real_dt` should be the
    /// fixed frame step, so tab suspension can't produce a huge delta.
    pub fn tick(&mut self, real_dt: f32) -> f32 {
        let d = if self.paused { 0.0 } else { real_dt * self.scale };
        self.time += d;
        self.last_delta = d;
        d
    }

    pub fn time(&self) -> f32 {
        self.time
    }
    pub fn delta(&self) -> f32 {
        self.last_delta
    }
    pub fn scale(&self) -> f32 {
        self.scale
    }
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Set the speed multiplier (clamped ≥ 0; negative/reverse is deferred).
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(0.0);
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
    pub fn reset(&mut self) {
        self.time = 0.0;
        self.last_delta = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_affects_advance() {
        let mut c = VisualClock::new();
        c.tick(1.0);
        assert_eq!(c.time(), 1.0);
        c.set_scale(2.0);
        c.tick(1.0);
        assert_eq!(c.time(), 3.0);
        assert_eq!(c.delta(), 2.0);
    }

    #[test]
    fn pause_freezes_without_jump() {
        let mut c = VisualClock::new();
        c.tick(0.5);
        c.set_paused(true);
        for _ in 0..100 {
            c.tick(0.5); // even huge "real" deltas don't advance while paused
        }
        assert_eq!(c.time(), 0.5);
        assert_eq!(c.delta(), 0.0);
        c.set_paused(false);
        c.tick(0.5);
        assert_eq!(c.time(), 1.0); // resumes, no accumulated jump
    }

    #[test]
    fn reset_zeroes() {
        let mut c = VisualClock::new();
        c.tick(5.0);
        c.reset();
        assert_eq!(c.time(), 0.0);
    }

    #[test]
    fn negative_scale_clamped() {
        let mut c = VisualClock::new();
        c.set_scale(-3.0);
        assert_eq!(c.scale(), 0.0);
    }
}
