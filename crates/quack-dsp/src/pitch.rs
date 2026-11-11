//! Pitch detection (roadmap step 2).
//!
//! [`Yin`] is a YIN detector (de Cheveigné and Kawahara, 2002) tuned for
//! the voice, from [`MIN_HZ`] to [`MAX_HZ`]:
//!
//! 1. the input is low-passed (anti-aliasing, and the upper harmonics
//!    that fool period detectors go too) and decimated to about 11 kHz,
//!    which divides the cost by the square of the factor; parabolic
//!    interpolation keeps the reading within a few cents;
//! 2. every [`HOP_MS`], the cumulative mean normalized difference of the
//!    last two longest periods gives the period (first dip under
//!    [`PICK_THRESHOLD`], which avoids the octave-down errors) and the
//!    aperiodicity of the frame;
//! 3. the voiced / unvoiced decision uses the aperiodicity with
//!    hysteresis, plus a level gate: consonants, breath and silence give
//!    no reading, so they will never be "corrected";
//! 4. voiced readings go through a median over [`MEDIAN_LEN`] hops, in
//!    semitones: an isolated octave glitch never reaches the output, a
//!    real note change or vibrato does, two hops later.
//!
//! The confidence (1 − aperiodicity) is what the shifter of step 3 and
//! the autotune of step 4 use to fade their correction in and out, key
//! to the Natural mode.
//!
//! Real-time: buffers are allocated in [`PitchDetector::prepare`] only;
//! [`Yin::push_sample`] does no allocation, no lock and a bounded amount
//! of work per hop.

use std::f64::consts::PI;

/// Lowest fundamental detected (low bass voice).
pub const MIN_HZ: f32 = 60.0;
/// Highest fundamental detected (high soprano, whistle excluded).
pub const MAX_HZ: f32 = 1500.0;
/// Analysis rate the input is decimated to, roughly.
const TARGET_RATE: f64 = 11_025.0;
/// Anti-aliasing low-pass cutoff (also drops upper harmonics).
const LOWPASS_HZ: f64 = 2_000.0;
/// DC and rumble removal.
const HIGHPASS_HZ: f64 = 40.0;
/// Time between two readings.
pub const HOP_MS: f64 = 2.5;
/// A dip of the normalized difference under this is taken as the period.
const PICK_THRESHOLD: f32 = 0.15;
/// Aperiodicity under which an unvoiced frame becomes voiced.
const VOICED_ON: f32 = 0.25;
/// Aperiodicity over which a voiced frame becomes unvoiced.
const VOICED_OFF: f32 = 0.40;
/// Frames quieter than this (RMS, dBFS) are unvoiced.
const GATE_DB: f32 = -55.0;
/// Shortest comparison window of the onset check, in seconds.
const ONSET_WINDOW_S: f64 = 0.005;
/// The onset check only takes clearly periodic readings...
const ONSET_MAX_APERIODICITY: f32 = 0.1;
/// ...that two hops in a row agree on, within this many semitones.
const ONSET_AGREEMENT_ST: f32 = 0.5;
/// Readings in the median smoother (odd).
pub const MEDIAN_LEN: usize = 5;

/// One pitch reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    /// Fundamental frequency in Hz.
    pub hz: f32,
    /// 0 (noise, unvoiced) to 1 (clearly periodic).
    pub confidence: f32,
}

impl PitchEstimate {
    /// The reading as a fractional MIDI note.
    #[must_use]
    pub fn note(&self) -> f32 {
        hz_to_midi(self.hz)
    }
}

/// Interface of the detectors. Implementations allocate in `prepare`
/// only.
pub trait PitchDetector {
    fn prepare(&mut self, sample_rate: f64, max_block: usize);
    fn reset(&mut self);
    /// Feed input samples.
    fn push(&mut self, input: &[f32]);
    /// Latest reading, `None` while unvoiced.
    fn estimate(&self) -> Option<PitchEstimate>;
    /// Samples between an input sample and the reading that includes it.
    fn latency_samples(&self) -> usize;
}

/// Frequency to MIDI note number (69 = A4 = 440 Hz), fractional.
#[must_use]
pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// MIDI note number (fractional) to frequency.
#[must_use]
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

/// RBJ biquad, transposed direct form II.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn lowpass(sample_rate: f64, hz: f64) -> Self {
        let w = 2.0 * PI * hz / sample_rate;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w.cos();
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - c) / 2.0 / a0) as f32,
            b1: ((1.0 - c) / a0) as f32,
            b2: ((1.0 - c) / 2.0 / a0) as f32,
            a1: (-2.0 * c / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    pub(crate) fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub(crate) fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Real-time YIN detector for the voice. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Yin {
    /// Decimation factor and position in it.
    factor: usize,
    phase: usize,
    /// Analysis rate (input rate / factor).
    rate: f32,
    lowpass: [Biquad; 2],
    hp_coef: f32,
    hp_x: f32,
    hp_y: f32,
    /// Integration window, shortest and longest lag, in analysis samples.
    window: usize,
    /// Shortest window of the onset check, in analysis samples.
    onset_window: usize,
    /// Onset check's lag at the previous hop, waiting for confirmation.
    onset_candidate: Option<f32>,
    tau_min: usize,
    tau_max: usize,
    /// Last `window + tau_max + 2` analysis samples, circular.
    ring: Vec<f32>,
    write: usize,
    filled: usize,
    /// Scratch: the ring unrolled, difference and normalized difference.
    frame: Vec<f32>,
    diff: Vec<f32>,
    cmnd: Vec<f32>,
    hop: usize,
    since_hop: usize,
    gate: f32,
    voiced: bool,
    /// Recent voiced readings in semitones, circular, `history_len` valid.
    history: [f32; MEDIAN_LEN],
    history_pos: usize,
    history_len: usize,
    current: Option<PitchEstimate>,
}

impl Yin {
    #[must_use]
    pub fn new(sample_rate: f64) -> Self {
        let mut y = Self::default();
        y.prepare(sample_rate, 0);
        y
    }

    /// Feed one input sample. Real-time safe.
    #[inline]
    pub fn push_sample(&mut self, x: f32) {
        let mut v = self.lowpass[0].tick(x);
        v = self.lowpass[1].tick(v);
        self.phase += 1;
        if self.phase < self.factor {
            return;
        }
        self.phase = 0;
        // DC blocker at the analysis rate.
        let hp = v - self.hp_x + self.hp_coef * self.hp_y;
        self.hp_x = v;
        self.hp_y = hp;
        self.ring[self.write] = hp;
        self.write = (self.write + 1) % self.ring.len();
        self.filled = (self.filled + 1).min(self.ring.len());
        self.since_hop += 1;
        if self.since_hop >= self.hop {
            self.since_hop = 0;
            self.analyse();
        }
    }

    /// Analysis rate in Hz.
    #[must_use]
    pub fn analysis_rate(&self) -> f32 {
        self.rate
    }

    /// One reading from the last frame.
    fn analyse(&mut self) {
        if self.filled < self.ring.len() {
            return;
        }
        let n = self.ring.len();
        let (old, new) = self.ring.split_at(self.write);
        self.frame[..new.len()].copy_from_slice(new);
        self.frame[new.len()..n].copy_from_slice(old);

        let w = self.window;
        let x = &self.frame;
        // Level of the most recent window.
        let recent = &x[n - w..];
        #[allow(clippy::cast_precision_loss)]
        let ms = recent.iter().map(|v| v * v).sum::<f32>() / w as f32;
        let o = self.onset_window;
        #[allow(clippy::cast_precision_loss)]
        let onset_ms = x[n - o..].iter().map(|v| v * v).sum::<f32>() / o as f32;
        let mut reading = if ms < self.gate { None } else { self.period() };
        let mut voiced = match reading {
            Some((_, ap)) if self.voiced => ap < VOICED_OFF,
            Some((_, ap)) => ap < VOICED_ON,
            None => false,
        };
        if !voiced {
            // The long frame still holds the silence before a note: look
            // at the last two periods only, so the note is read ~10 ms
            // after it starts instead of ~30 ms. A consonant burst can
            // look periodic over so short a window once: only a clearly
            // periodic reading confirmed by the next hop counts.
            let candidate = if onset_ms >= self.gate {
                self.recent_period()
                    .filter(|r| r.1 < ONSET_MAX_APERIODICITY)
            } else {
                None
            };
            if let (Some(r), Some(prev)) = (candidate, self.onset_candidate)
                && (12.0 * (r.0 / prev).log2()).abs() < ONSET_AGREEMENT_ST
            {
                reading = Some(r);
                voiced = true;
            }
            self.onset_candidate = candidate.map(|r| r.0);
        } else {
            self.onset_candidate = None;
        }
        self.voiced = voiced;
        match reading {
            Some((tau, ap)) if voiced => {
                let hz = self.rate / tau;
                self.history[self.history_pos] = hz_to_midi(hz);
                self.history_pos = (self.history_pos + 1) % MEDIAN_LEN;
                self.history_len = (self.history_len + 1).min(MEDIAN_LEN);
                let mut sorted = self.history;
                let valid = &mut sorted[..self.history_len];
                valid.sort_unstable_by(f32::total_cmp);
                let note = valid[self.history_len / 2];
                self.current = Some(PitchEstimate {
                    hz: midi_to_hz(note),
                    confidence: (1.0 - ap).clamp(0.0, 1.0),
                });
            }
            _ => {
                self.history_len = 0;
                self.current = None;
            }
        }
    }

    /// Onset check: period of the most recent samples, comparing the
    /// last `max(tau, onset_window)` samples with the ones `tau` earlier.
    /// Only a clear dip counts (no fallback to the global minimum).
    fn recent_period(&mut self) -> Option<(f32, f32)> {
        let n = self.frame.len();
        let last = self.tau_max + 1;
        let x = &self.frame;
        let mut cum = 0.0;
        self.cmnd[0] = 1.0;
        for tau in 1..=last {
            let w = tau.max(self.onset_window);
            let cur = &x[n - w..];
            let lagged = &x[n - w - tau..n - tau];
            #[allow(clippy::cast_precision_loss)]
            let d = cur
                .iter()
                .zip(lagged)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>()
                / w as f32;
            self.diff[tau] = d;
            cum += d;
            #[allow(clippy::cast_precision_loss)]
            let v = if cum > 0.0 { d * tau as f32 / cum } else { 1.0 };
            self.cmnd[tau] = v;
        }
        let d = &self.cmnd;
        let mut tau = (self.tau_min..=self.tau_max).find(|&t| d[t] < PICK_THRESHOLD)?;
        while tau < self.tau_max && d[tau + 1] < d[tau] {
            tau += 1;
        }
        let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
        let den = a - 2.0 * b + c;
        let shift = if den > 0.0 {
            (0.5 * (a - c) / den).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        let lag = tau as f32 + shift;
        Some((lag, d[tau]))
    }

    /// Period (fractional lag) and aperiodicity of `frame`.
    fn period(&mut self) -> Option<(f32, f32)> {
        let w = self.window;
        let last = self.tau_max + 1;
        let x = &self.frame;
        let head = &x[..w];
        for tau in 1..=last {
            let lagged = &x[tau..tau + w];
            self.diff[tau] = head
                .iter()
                .zip(lagged)
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
        }
        // Cumulative mean normalized difference.
        self.cmnd[0] = 1.0;
        let mut cum = 0.0;
        for tau in 1..=last {
            cum += self.diff[tau];
            #[allow(clippy::cast_precision_loss)]
            let v = if cum > 0.0 {
                self.diff[tau] * tau as f32 / cum
            } else {
                1.0
            };
            self.cmnd[tau] = v;
        }
        let d = &self.cmnd;
        // First dip under the threshold, down to its local minimum; else
        // the global minimum (an aperiodic frame, rejected by voicing).
        let mut tau = (self.tau_min..=self.tau_max).find(|&t| d[t] < PICK_THRESHOLD);
        if let Some(t) = tau.as_mut() {
            while *t < self.tau_max && d[*t + 1] < d[*t] {
                *t += 1;
            }
        }
        let tau = tau.unwrap_or_else(|| {
            (self.tau_min..=self.tau_max)
                .min_by(|a, b| d[*a].total_cmp(&d[*b]))
                .unwrap_or(self.tau_min)
        });
        // Parabolic interpolation on the raw difference.
        let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
        let den = a - 2.0 * b + c;
        let shift = if den > 0.0 {
            (0.5 * (a - c) / den).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        let lag = tau as f32 + shift;
        Some((lag, d[tau]))
    }
}

impl PitchDetector for Yin {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn prepare(&mut self, sample_rate: f64, _max_block: usize) {
        let sr = sample_rate.max(8_000.0);
        self.factor = ((sr / TARGET_RATE).floor() as usize).max(1);
        let rate = sr / self.factor as f64;
        self.rate = rate as f32;
        let cutoff = LOWPASS_HZ.min(0.4 * rate);
        self.lowpass = [Biquad::lowpass(sr, cutoff); 2];
        self.hp_coef = (-2.0 * PI * HIGHPASS_HZ / rate).exp() as f32;
        self.tau_max = (rate / f64::from(MIN_HZ)).ceil() as usize;
        self.tau_min = ((rate / f64::from(MAX_HZ)).floor() as usize).max(2);
        self.window = self.tau_max;
        self.onset_window = ((rate * ONSET_WINDOW_S).round() as usize).max(8);
        let len = self.window + self.tau_max + 2;
        self.ring = vec![0.0; len];
        self.frame = vec![0.0; len];
        self.diff = vec![0.0; self.tau_max + 2];
        self.cmnd = vec![0.0; self.tau_max + 2];
        self.hop = ((rate * HOP_MS / 1000.0).round() as usize).max(1);
        self.gate = 10.0_f32.powf(GATE_DB / 10.0);
        self.reset();
    }

    fn reset(&mut self) {
        for b in &mut self.lowpass {
            b.reset();
        }
        self.hp_x = 0.0;
        self.hp_y = 0.0;
        self.ring.fill(0.0);
        self.write = 0;
        self.filled = 0;
        self.phase = 0;
        self.since_hop = 0;
        self.voiced = false;
        self.onset_candidate = None;
        self.history_len = 0;
        self.history_pos = 0;
        self.current = None;
    }

    fn push(&mut self, input: &[f32]) {
        for &x in input {
            self.push_sample(x);
        }
    }

    fn estimate(&self) -> Option<PitchEstimate> {
        self.current
    }

    /// Centre of the analysis frame plus the median's delay, in input
    /// samples (about 25 ms): how late the reading follows the voice.
    /// This is a control delay for the shifter, not audio latency.
    fn latency_samples(&self) -> usize {
        (self.ring.len() / 2 + self.hop * (MEDIAN_LEN / 2)) * self.factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(sr: f64, secs: f64, mut f: impl FnMut(f64) -> f32) -> Vec<f32> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (sr * secs) as usize;
        #[allow(clippy::cast_precision_loss)]
        (0..n).map(|i| f(i as f64 / sr)).collect()
    }

    /// Voice-like: harmonics falling at 6 dB per octave.
    fn buzz(hz: f64, t: f64) -> f32 {
        #[allow(clippy::cast_possible_truncation)]
        let v = (1..=12)
            .map(f64::from)
            .filter(|k| k * hz < 5_000.0)
            .map(|k| (2.0 * PI * k * hz * t).sin() / k)
            .sum::<f64>() as f32;
        0.3 * v
    }

    /// Readings after each hop.
    fn track(sr: f64, x: &[f32]) -> Vec<Option<PitchEstimate>> {
        let mut y = Yin::new(sr);
        let mut out = Vec::new();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let hop = (sr * HOP_MS / 1000.0) as usize;
        for chunk in x.chunks(hop) {
            y.push(chunk);
            out.push(y.estimate());
        }
        out
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn midi_and_hz_round_trip() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((midi_to_hz(60.0) - 261.6256).abs() < 1e-2);
        for n in [40.0, 57.5, 81.25] {
            assert!((hz_to_midi(midi_to_hz(n)) - n).abs() < 1e-4);
        }
    }

    #[test]
    fn sines_and_voices_are_read_within_a_few_cents() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for hz in [65.0, 98.0, 147.0, 220.0, 330.0, 523.0, 880.0, 1250.0] {
                for (name, f) in [
                    (
                        "sine",
                        (|hz: f64, t: f64| 0.3 * (2.0 * PI * hz * t).sin() as f32)
                            as fn(f64, f64) -> f32,
                    ),
                    ("buzz", buzz),
                ] {
                    let x = tone(sr, 0.4, |t| f(hz, t));
                    let r = track(sr, &x);
                    let tail = &r[r.len() / 2..];
                    for e in tail {
                        let e = e.unwrap_or_else(|| panic!("{name} {hz} Hz @ {sr}: unvoiced"));
                        #[allow(clippy::cast_possible_truncation)]
                        let c = cents(e.hz, hz as f32);
                        assert!(c.abs() < 5.0, "{name} {hz} Hz @ {sr}: {c:.1} cents");
                        assert!(e.confidence > 0.9, "{name} {hz} Hz: {}", e.confidence);
                    }
                }
            }
        }
    }

    #[test]
    fn strong_second_harmonic_is_not_an_octave_up() {
        // Fundamental at a third of the second harmonic's level.
        let sr = 48_000.0;
        let x = tone(sr, 0.4, |t| {
            let w = 2.0 * PI * 110.0 * t;
            #[allow(clippy::cast_possible_truncation)]
            let v = (0.1 * w.sin() + 0.3 * (2.0 * w).sin() + 0.1 * (3.0 * w).sin()) as f32;
            v
        });
        for e in &track(sr, &x)[60..] {
            let e = e.expect("voiced");
            assert!(cents(e.hz, 110.0).abs() < 5.0, "{} Hz", e.hz);
        }
    }

    #[test]
    fn noise_and_silence_are_unvoiced() {
        let sr = 48_000.0;
        let mut s: u32 = 1;
        let noise = tone(sr, 1.0, |_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            #[allow(clippy::cast_precision_loss)]
            let v = (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            0.5 * v
        });
        let r = track(sr, &noise);
        let voiced = r.iter().filter(|e| e.is_some()).count();
        assert!(voiced * 50 < r.len(), "{voiced} of {} voiced", r.len());
        let quiet = tone(sr, 0.4, |t| 1e-4 * (2.0 * PI * 220.0 * t).sin() as f32);
        assert!(track(sr, &quiet).iter().all(Option::is_none));
        assert!(track(sr, &vec![0.0; 20_000]).iter().all(Option::is_none));
    }

    #[test]
    fn onset_is_quick_and_release_is_clean() {
        let sr = 48_000.0;
        let mut x = vec![0.0; 9_600];
        x.extend(tone(sr, 0.3, |t| buzz(196.0, t)));
        let start = x.len();
        x.extend(vec![0.0; 9_600]);
        let r = track(sr, &x);
        let hop = sr * HOP_MS / 1000.0;
        let first = r.iter().position(Option::is_some).expect("voiced");
        let onset_ms = (first as f64 * hop - 9_600.0) / sr * 1000.0;
        assert!(onset_ms < 15.0, "onset {onset_ms:.1} ms");
        let last = r.iter().rposition(Option::is_some).expect("voiced");
        let release_ms = (last as f64 * hop - start as f64) / sr * 1000.0;
        assert!(release_ms < 40.0, "release {release_ms:.1} ms");
    }

    #[test]
    fn vibrato_and_note_changes_are_followed() {
        let sr = 48_000.0;
        // 220 Hz, 5.5 Hz vibrato of ±50 cents.
        let mut phase = 0.0;
        let x = tone(sr, 1.0, |t| {
            let f = 220.0 * (0.5 / 12.0 * (2.0 * PI * 5.5 * t).sin()).exp2();
            phase += 2.0 * PI * f / sr;
            #[allow(clippy::cast_possible_truncation)]
            let v = 0.3 * phase.sin() as f32;
            v
        });
        let r = track(sr, &x);
        let lag = Yin::new(sr).latency_samples() as f64 / sr;
        let hop = sr * HOP_MS / 1000.0;
        for (i, e) in r.iter().enumerate().skip(40) {
            let t = (i + 1) as f64 * hop / sr - lag;
            let want = 220.0 * (0.5 / 12.0 * (2.0 * PI * 5.5 * t).sin()).exp2();
            #[allow(clippy::cast_possible_truncation)]
            let c = cents(e.expect("voiced").hz, want as f32);
            assert!(c.abs() < 15.0, "hop {i}: {c:.1} cents");
        }
        // A fifth up, read within 30 ms.
        let mut x = tone(sr, 0.3, |t| buzz(220.0, t));
        x.extend(tone(sr, 0.3, |t| buzz(330.0, t)));
        let r = track(sr, &x);
        let change = (0.3 / (HOP_MS / 1000.0)) as usize;
        let settled = change + (0.03 / (HOP_MS / 1000.0)) as usize;
        for e in &r[settled..] {
            assert!(cents(e.expect("voiced").hz, 330.0).abs() < 5.0);
        }
    }

    #[test]
    fn reset_forgets_the_voice() {
        let sr = 44_100.0;
        let mut y = Yin::new(sr);
        y.push(&tone(sr, 0.2, |t| buzz(150.0, t)));
        assert!(y.estimate().is_some());
        y.reset();
        assert!(y.estimate().is_none());
        assert!(y.latency_samples() > 0);
    }
}
