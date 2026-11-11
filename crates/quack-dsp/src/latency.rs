//! Latency and delay lines.
//!
//! The latency is what the pitch shifter needs for the lowest note of
//! the voice ([`VoiceRange`], chosen by the singer, like the "input
//! type" of classic pitch correctors): about 1.6 periods of that note
//! (see [`crate::shift::Psola::latency_for`]). It does not depend on the
//! mode or any other parameter, so switching modes live never makes the
//! host re-align; changing the range does (a setup choice, made once).
//! The engine pads the dry path with delay lines so the reported and the
//! real latency always match (checked by tests).

use crate::shift::Psola;

/// Lowest note the voice goes down to. The lower, the longer the
/// latency: a grain of the shifter holds a whole period of the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum VoiceRange {
    /// Down to F3 (175 Hz).
    High,
    /// Down to C3 (131 Hz).
    #[default]
    Tenor,
    /// Down to G2 (98 Hz).
    Baritone,
    /// Down to D2 (73 Hz).
    Bass,
}

impl VoiceRange {
    pub const COUNT: u32 = 4;
    pub const ALL: [Self; 4] = [Self::High, Self::Tenor, Self::Baritone, Self::Bass];

    /// From a parameter's plain value (index), clamped.
    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::High => "Soprano / Alto",
            Self::Tenor => "Tenor",
            Self::Baritone => "Baritone",
            Self::Bass => "Bass",
        }
    }

    /// Lowest fundamental, in Hz.
    #[must_use]
    pub const fn lowest_hz(self) -> f64 {
        match self {
            Self::High => 174.6,
            Self::Tenor => 130.8,
            Self::Baritone => 98.0,
            Self::Bass => 73.4,
        }
    }
}

/// Share of a period the grains fade in over in the Low Latency mode
/// (1 otherwise): 30 % less latency, formants a little less faithful
/// (about 1 dB more envelope error on average, see the `latency`
/// example).
pub const LOW_LATENCY_ATTACK: f64 = 0.5;

/// Grain attack for the Low Latency switch.
#[must_use]
pub fn attack(low_latency: bool) -> f64 {
    if low_latency { LOW_LATENCY_ATTACK } else { 1.0 }
}

/// Latency at `sample_rate` for a voice in `range`, in samples, with the
/// Low Latency switch on or off.
#[must_use]
pub fn latency_samples(sample_rate: f64, range: VoiceRange, low_latency: bool) -> usize {
    Psola::latency_for(
        sample_rate.max(8_000.0),
        range.lowest_hz(),
        attack(low_latency),
    )
}

/// Longest latency at `sample_rate` (the lowest range): what the delay
/// lines are allocated for.
#[must_use]
pub fn max_latency_samples(sample_rate: f64) -> usize {
    latency_samples(sample_rate, VoiceRange::Bass, false)
}

/// Delay line, allocated once in [`DelayLine::prepare`], whose delay can
/// then change up to that length.
#[derive(Debug, Clone, Default)]
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
    delay: usize,
}

impl DelayLine {
    /// Allocates for delays up to `delay` and sets it. Not for the
    /// audio thread.
    pub fn prepare(&mut self, delay: usize) {
        self.buf = vec![0.0; delay + 1];
        self.pos = 0;
        self.delay = delay;
    }

    /// Change the delay (clamped to the allocated length) and clear.
    /// Real-time safe.
    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay.min(self.buf.len().saturating_sub(1));
        self.reset();
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    #[must_use]
    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Push one sample, get the one from `delay` samples ago.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let len = self.buf.len();
        if self.delay == 0 || len == 0 {
            return x;
        }
        self.buf[self.pos] = x;
        let read = if self.pos >= self.delay {
            self.pos - self.delay
        } else {
            self.pos + len - self.delay
        };
        self.pos = if self.pos + 1 == len { 0 } else { self.pos + 1 };
        self.buf[read]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_follows_the_lowest_note() {
        // 1.6 periods of the lowest note, plus a few samples.
        assert_eq!(latency_samples(48_000.0, VoiceRange::Tenor, false), 591);
        assert_eq!(latency_samples(48_000.0, VoiceRange::High, false), 443);
        let ms = |r| latency_samples(48_000.0, r, false) as f64 / 48.0;
        assert!(ms(VoiceRange::High) < 10.0);
        // Low Latency: the Tenor range under 10 ms, and never longer.
        assert!((latency_samples(48_000.0, VoiceRange::Tenor, true) as f64 / 48.0) < 9.0);
        for r in VoiceRange::ALL {
            assert!(latency_samples(48_000.0, r, true) < latency_samples(48_000.0, r, false));
        }
        assert!(ms(VoiceRange::Bass) < 22.0);
        // Same time at any rate.
        for r in VoiceRange::ALL {
            let a = latency_samples(44_100.0, r, false) as f64 / 44.1;
            let b = latency_samples(96_000.0, r, false) as f64 / 96.0;
            assert!((a - ms(r)).abs() < 0.1 && (b - ms(r)).abs() < 0.1);
        }
        assert_eq!(
            max_latency_samples(48_000.0),
            latency_samples(48_000.0, VoiceRange::Bass, false)
        );
    }

    #[test]
    fn delay_line_delays_by_its_length() {
        let mut d = DelayLine::default();
        d.prepare(3);
        let out: Vec<f32> = [1.0, 2.0, 3.0, 4.0, 5.0].map(|x| d.tick(x)).to_vec();
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0, 2.0]);
        d.set_delay(1);
        let out: Vec<f32> = [1.0, 2.0, 3.0].map(|x| d.tick(x)).to_vec();
        assert_eq!(out, [0.0, 1.0, 2.0]);
        d.set_delay(9);
        assert_eq!(d.delay(), 3);
    }
}
