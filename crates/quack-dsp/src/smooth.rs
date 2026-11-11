//! Parameter smoothing.

/// Linear ramp to a target over a fixed time. Real-time safe.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    left: u32,
    len: u32,
}

impl Ramp {
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            left: 0,
            len: 1,
        }
    }

    /// Ramp length from a time in milliseconds.
    pub fn prepare(&mut self, sample_rate: f64, ms: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let len = (sample_rate * ms / 1000.0).round().max(1.0) as u32;
        self.len = len;
        self.snap(self.target);
    }

    pub fn set(&mut self, target: f32) {
        if target != self.target {
            self.target = target;
            self.left = self.len;
            #[allow(clippy::cast_precision_loss)]
            let n = self.len as f32;
            self.step = (target - self.value) / n;
        }
    }

    pub fn snap(&mut self, target: f32) {
        self.target = target;
        self.value = target;
        self.left = 0;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            self.value = if self.left == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        self.value
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.left == 0
    }

    #[must_use]
    pub fn target(&self) -> f32 {
        self.target
    }
}
