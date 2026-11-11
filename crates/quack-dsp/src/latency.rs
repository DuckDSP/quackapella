//! Latency budget and delay lines.
//!
//! Quackapella reports one fixed latency to the host from the first
//! version on, whatever the mode: hosts compensate it on playback, and a
//! value that changes while playing (or between modes) makes them
//! re-align or click. The budget is what the natural pitch shifter of
//! step 3 needs to analyse at least one period of a low voice; until
//! then the engine pads the signal with plain delay lines so the
//! reported and the real latency always match (checked by tests).

/// Total latency budget, in milliseconds. Live use target: about 10 ms,
/// below what a singer notices in their monitoring. To be confirmed by
/// the step 3 measurements before the first release, then frozen.
pub const LATENCY_MS: f64 = 10.0;

/// The budget in samples at `sample_rate`.
#[must_use]
pub fn latency_samples(sample_rate: f64) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (sample_rate.max(1.0) * LATENCY_MS / 1000.0).round() as usize;
    n
}

/// Fixed delay, allocated once in [`DelayLine::prepare`].
#[derive(Debug, Clone, Default)]
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    /// Allocates. Not for the audio thread.
    pub fn prepare(&mut self, delay: usize) {
        self.buf = vec![0.0; delay];
        self.pos = 0;
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    #[must_use]
    pub fn delay(&self) -> usize {
        self.buf.len()
    }

    /// Push one sample, get the one from `delay` samples ago.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        if self.buf.is_empty() {
            return x;
        }
        let y = std::mem::replace(&mut self.buf[self.pos], x);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_scales_with_rate() {
        assert_eq!(latency_samples(48_000.0), 480);
        assert_eq!(latency_samples(44_100.0), 441);
        assert_eq!(latency_samples(96_000.0), 960);
    }

    #[test]
    fn delay_line_delays_by_its_length() {
        let mut d = DelayLine::default();
        d.prepare(3);
        let out: Vec<f32> = [1.0, 2.0, 3.0, 4.0, 5.0].map(|x| d.tick(x)).to_vec();
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0, 2.0]);
    }
}
