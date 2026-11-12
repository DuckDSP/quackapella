//! Throat: the size of the singer's vocal tract (age and gender).
//!
//! A child, a woman, a man and a giant differ by more than where their
//! formants sit. Moving the formants alone (the *Formant* knob) makes a
//! man with a small head, not a child: a shorter throat also has less
//! chest (the low resonance of a big body) and a brighter top, a longer
//! one more body and a darker top. *Throat* moves all of it together:
//!
//! - the formants, through the shifter's grain resampling, up to
//!   [`THROAT_FORMANT_ST`] either way (a child's tract is about a third
//!   shorter than a man's, +5 to +6 semitones);
//! - the body: a chest resonance around 200 Hz, lifted for a longer
//!   throat, a low shelf taking it away for a shorter one;
//! - the top: a high shelf, darker for a longer throat, a little
//!   presence for a shorter one;
//! - and, with *Throat Pitch*, the pitch, as a real speaker of that size
//!   would sing it (a child is about an octave above a man):
//!   [`THROAT_PITCH_ST`] at full throat and full link.
//!
//! The filters sit on the shifter's input, before the grains: the
//! formant shift then moves them with the rest of the tract, as in a
//! real throat. They are minimum phase (biquads): no latency, and at
//! *Throat* 0 they are bypassed (the signal comes out bit for bit).
//!
//! Real-time: no allocation; the coefficients are only recomputed while
//! the knob moves, every [`UPDATE_EVERY`] samples.

/// Formant shift at full throat, semitones (shorter throat: up).
pub const THROAT_FORMANT_ST: f64 = 6.0;
/// Pitch shift at full throat and full *Throat Pitch*, semitones.
pub const THROAT_PITCH_ST: f64 = 12.0;

/// Samples between two coefficient updates while the knob moves.
const UPDATE_EVERY: u32 = 32;
/// Longer throat at full: chest resonance and darker top (dB).
const CHEST_HZ: f64 = 200.0;
const CHEST_Q: f64 = 0.9;
const CHEST_DB: f64 = 4.0;
const DARK_HZ: f64 = 5_000.0;
const DARK_DB: f64 = -4.0;
/// Shorter throat at full: less chest, a little presence (dB).
const THIN_HZ: f64 = 300.0;
const THIN_DB: f64 = -6.0;
const BRIGHT_HZ: f64 = 3_500.0;
const BRIGHT_DB: f64 = 2.5;

/// Formant shift (semitones) for a throat setting, -1 (shortest) to
/// +1 (longest).
#[must_use]
pub fn formant_st(throat: f64) -> f64 {
    -throat.clamp(-1.0, 1.0) * THROAT_FORMANT_ST
}

/// Pitch shift (semitones) for a throat setting and its pitch link
/// (0 to 1).
#[must_use]
pub fn pitch_st(throat: f64, link: f64) -> f64 {
    -throat.clamp(-1.0, 1.0) * link.clamp(0.0, 1.0) * THROAT_PITCH_ST
}

/// Biquad coefficients, normalized (`a0` = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Coefs {
    b: [f32; 3],
    a: [f32; 2],
}

impl Coefs {
    const UNITY: Self = Self {
        b: [1.0, 0.0, 0.0],
        a: [0.0, 0.0],
    };

    fn from_raw(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        Self {
            b: [(b0 / a0) as f32, (b1 / a0) as f32, (b2 / a0) as f32],
            a: [(a1 / a0) as f32, (a2 / a0) as f32],
        }
    }

    /// Peaking EQ (RBJ cookbook).
    fn peak(sr: f64, hz: f64, q: f64, db: f64) -> Self {
        let a = 10f64.powf(db / 40.0);
        let w = std::f64::consts::TAU * hz / sr;
        let alpha = w.sin() / (2.0 * q);
        let c = w.cos();
        Self::from_raw(
            1.0 + alpha * a,
            -2.0 * c,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * c,
            1.0 - alpha / a,
        )
    }

    /// Low or high shelf, slope 1 (RBJ cookbook).
    fn shelf(sr: f64, hz: f64, db: f64, high: bool) -> Self {
        let a = 10f64.powf(db / 40.0);
        let w = std::f64::consts::TAU * hz / sr;
        let c = w.cos();
        let alpha = w.sin() / 2.0 * std::f64::consts::SQRT_2;
        let k = 2.0 * a.sqrt() * alpha;
        if high {
            Self::from_raw(
                a * ((a + 1.0) + (a - 1.0) * c + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - k),
                (a + 1.0) - (a - 1.0) * c + k,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - k,
            )
        } else {
            Self::from_raw(
                a * ((a + 1.0) - (a - 1.0) * c + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - k),
                (a + 1.0) + (a - 1.0) * c + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - k,
            )
        }
    }
}

/// Transposed direct form II state, one per channel and stage.
#[derive(Debug, Clone, Copy, Default)]
struct State([f32; 2]);

impl State {
    #[inline]
    fn tick(&mut self, c: &Coefs, x: f32) -> f32 {
        let y = c.b[0] * x + self.0[0];
        self.0[0] = c.b[1] * x - c.a[0] * y + self.0[1];
        self.0[1] = c.b[2] * x - c.a[1] * y;
        y
    }
}

/// The throat's body and top filters, stereo. See the module docs.
#[derive(Debug, Clone)]
pub struct Throat {
    sample_rate: f64,
    /// Setting now, -1 to 1 (smoothed by the caller).
    applied: f32,
    coefs: [Coefs; 2],
    state: [[State; 2]; 2],
    countdown: u32,
}

impl Default for Throat {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            applied: 0.0,
            coefs: [Coefs::UNITY; 2],
            state: Default::default(),
            countdown: 0,
        }
    }
}

impl Throat {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        let t = self.applied;
        self.applied = f32::NAN;
        self.update(t);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.state = Default::default();
        self.countdown = 0;
    }

    fn update(&mut self, throat: f32) {
        if throat == self.applied {
            return;
        }
        self.applied = throat;
        let t = f64::from(throat.clamp(-1.0, 1.0));
        let sr = self.sample_rate;
        self.coefs = if t >= 0.0 {
            [
                Coefs::peak(sr, CHEST_HZ, CHEST_Q, CHEST_DB * t),
                Coefs::shelf(sr, DARK_HZ, DARK_DB * t, true),
            ]
        } else {
            let s = -t;
            [
                Coefs::shelf(sr, THIN_HZ, THIN_DB * s, false),
                Coefs::shelf(sr, BRIGHT_HZ, BRIGHT_DB * s, true),
            ]
        };
    }

    /// One stereo sample, with the throat setting now (-1 shortest to
    /// +1 longest). At 0 the input comes out as is.
    #[inline]
    pub fn tick(&mut self, throat: f32, l: f32, r: f32) -> (f32, f32) {
        if throat == 0.0 && self.applied == 0.0 {
            // At rest: no filtering, and no state left from before
            // (it would ring once the knob moves again).
            if self.countdown != u32::MAX {
                self.state = Default::default();
                self.countdown = u32::MAX;
            }
            return (l, r);
        }
        if self.countdown == u32::MAX {
            self.countdown = 0;
        }
        if self.countdown == 0 {
            self.update(throat);
            self.countdown = UPDATE_EVERY;
        }
        self.countdown -= 1;
        let mut out = [l, r];
        for (ch, x) in out.iter_mut().enumerate() {
            for (k, c) in self.coefs.iter().enumerate() {
                *x = self.state[ch][k].tick(c, *x);
            }
        }
        (out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_gain(t: &mut Throat, throat: f32, hz: f64) -> f64 {
        t.reset();
        let sr = 48_000.0;
        let n = 9_600;
        let mut e_in = 0.0;
        let mut e_out = 0.0;
        for i in 0..n {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let x = (std::f64::consts::TAU * hz * i as f64 / sr).sin() as f32;
            let (y, _) = t.tick(throat, x, x);
            if i > n / 2 {
                e_in += f64::from(x * x);
                e_out += f64::from(y * y);
            }
        }
        10.0 * (e_out / e_in).log10()
    }

    #[test]
    fn rest_is_bit_exact() {
        let mut t = Throat::default();
        t.prepare(48_000.0);
        for i in 0..100 {
            #[allow(clippy::cast_precision_loss)]
            let x = (i as f32 * 0.37).sin();
            assert_eq!(t.tick(0.0, x, -x), (x, -x));
        }
    }

    #[test]
    fn longer_throat_has_more_body_and_a_darker_top() {
        let mut t = Throat::default();
        t.prepare(48_000.0);
        let body = tone_gain(&mut t, 1.0, 200.0);
        let top = tone_gain(&mut t, 1.0, 9_000.0);
        assert!(body > 3.0, "body {body:.1} dB");
        assert!(top < -3.0, "top {top:.1} dB");
        // The voice's middle (vowels) stays where it was.
        let mid = tone_gain(&mut t, 1.0, 1_200.0);
        assert!(mid.abs() < 1.5, "mid {mid:.1} dB");
    }

    #[test]
    fn shorter_throat_loses_chest_and_gains_presence() {
        let mut t = Throat::default();
        t.prepare(48_000.0);
        let chest = tone_gain(&mut t, -1.0, 120.0);
        let top = tone_gain(&mut t, -1.0, 8_000.0);
        assert!(chest < -4.0, "chest {chest:.1} dB");
        assert!(top > 1.5, "top {top:.1} dB");
    }

    #[test]
    fn mapping_goes_the_natural_way() {
        // Shorter throat (child): formants and pitch up.
        assert!(formant_st(-1.0) > 5.0);
        assert!((pitch_st(-1.0, 1.0) - 12.0).abs() < 1e-9);
        assert_eq!(pitch_st(-1.0, 0.0), 0.0);
        assert!(formant_st(0.5) < 0.0);
    }
}
