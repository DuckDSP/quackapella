//! Pitch shifting with formant preservation (roadmap step 3).
//!
//! The heart of the *Natural* mode: [`Psola`], a real-time TD-PSOLA
//! (pitch-synchronous overlap-add) driven by the pitch detector.
//!
//! 1. **Analysis marks.** While the voice is voiced, one mark per
//!    period, on the peak of the low-passed input found around the
//!    predicted position (last mark + detected period): the marks follow
//!    the waveform itself, so the detector's 25 ms reading delay and
//!    vibrato do not blur them. Unvoiced input (s, f, ch, breath,
//!    silence) gets marks at a fixed short spacing instead.
//! 2. **Grains.** Each mark gives a grain windowed by an asymmetric Hann
//!    reaching the previous and the next mark (two periods). One grain
//!    holds one glottal pulse with its vocal-tract response: its shape
//!    *is* the spectral envelope. Neighbouring grains sum to exactly one
//!    at ratio 1, so the shifter is transparent when it does nothing.
//! 3. **Synthesis.** Grains are overlap-added at synthesis marks spaced
//!    by the period divided by the pitch ratio; each synthesis mark takes
//!    the analysis grain nearest in time (repeated going up, skipped
//!    going down). Pitch moves, the grain shape does not: formants stay
//!    in place, without the chipmunk or monster colour of resampling,
//!    and with none of the phasiness of a phase vocoder.
//! 4. **Formants on their own.** Reading a grain faster or slower
//!    (`formant_ratio`) moves its envelope without touching the pitch.
//! 5. **Unvoiced** grains are always put back at ratio 1: noise is never
//!    pitched (it would buzz), consonants and breath stay intact.
//!
//! Latency: the output is the input timeline delayed by exactly
//! [`crate::latency::latency_samples`], so dry, wet and bypass line up.
//! A grain needs its full two periods of input before it is placed;
//! below about 120 Hz that is more than the budget, and the shifter then
//! uses the newest complete grain, which delays the content of low
//! voices by a few milliseconds more (no click, no change of reported
//! latency).
//!
//! Real-time: every buffer is allocated in [`Psola::prepare`]; `tick`
//! does no allocation, no lock and a bounded amount of work.

use std::f64::consts::PI;

use crate::pitch::{Biquad, MAX_HZ, MIN_HZ};

/// Spacing of the unvoiced marks, as a fraction of the latency budget
/// (2.5 ms at the 10 ms budget). Grains of unvoiced input are put back
/// unshifted; the spacing only has to leave the grain complete in time.
const UNVOICED_FRACTION: usize = 4;
/// Peak search around the predicted mark, in periods.
const SEARCH: f64 = 0.3;
/// Low-pass of the mark search signal: keeps the fundamental's peak.
const MARK_LOWPASS_HZ: f64 = 1_000.0;
/// Time the voicing glides over when the voice starts or stops: the
/// first grains of a note move from the sung pitch to the target (a
/// short scoop) instead of jumping, which would click.
const GLIDE_S: f64 = 0.012;
/// A detector dropout shorter than this keeps the last period, so a
/// rough onset does not flip between shifted and unshifted grains.
const HOLD_S: f64 = 0.010;
/// Analysis marks remembered (several periods of the lowest voice).
const MARKS: usize = 256;
/// Widest formant shift supported by the buffers (ratio 0.5 to 2).
pub const MAX_FORMANT_RATIO: f32 = 2.0;
/// Widest pitch shift supported (ratio 0.25 to 4, two octaves).
pub const MAX_PITCH_RATIO: f32 = 4.0;

pub const CHANNELS: usize = 2;

/// Semitones to pitch ratio.
#[must_use]
pub fn semitones_to_ratio(st: f32) -> f32 {
    (st / 12.0).exp2()
}

#[derive(Debug, Clone, Copy, Default)]
struct Mark {
    /// Input sample index.
    pos: i64,
    voiced: bool,
}

/// Real-time pitch-synchronous shifter. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Psola {
    latency: i64,
    unvoiced: i64,
    min_period: f64,
    max_period: f64,
    /// Next input sample index.
    n: i64,
    input: [Vec<f32>; CHANNELS],
    /// Low-passed mono input, for the mark search.
    search: Vec<f32>,
    in_mask: usize,
    lowpass: [Biquad; 2],
    marks: Vec<Mark>,
    /// Marks placed since reset; the newest is `marks[(count - 1) % MARKS]`.
    count: usize,
    /// Output accumulators, indexed by output-timeline sample.
    acc: [Vec<f32>; CHANNELS],
    out_mask: usize,
    /// Next synthesis mark, on the input timeline.
    next_s: f64,
    /// How far ahead of the output the next grain must be placed.
    lead: f64,
    /// 0 (unvoiced, unshifted) to 1 (voiced, fully shifted).
    voicing: f64,
    glide: f64,
    /// Last detected period and how many samples it is still held for.
    held_period: f32,
    hold: usize,
    hold_len: usize,
}

impl Psola {
    #[must_use]
    pub fn new(sample_rate: f64, latency: usize) -> Self {
        let mut s = Self::default();
        s.prepare(sample_rate, latency);
        s
    }

    /// Allocates for `sample_rate` and an output delay of `latency`
    /// samples. Not for the audio thread.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn prepare(&mut self, sample_rate: f64, latency: usize) {
        let sr = sample_rate.max(8_000.0);
        self.latency = latency as i64;
        self.unvoiced = ((latency / UNVOICED_FRACTION).max(16)) as i64;
        self.min_period = sr / f64::from(MAX_HZ);
        self.max_period = sr / f64::from(MIN_HZ);
        let pmax = self.max_period.ceil() as usize;
        // Oldest read: an output delay, the extra delay of low voices
        // and a grain's left half, all at most a few long periods.
        let in_len = (2 * (latency + 6 * pmax)).next_power_of_two();
        // Furthest write: lead plus a grain's right half, both widened by
        // the formant ratio.
        let out_len = (8 * pmax * MAX_FORMANT_RATIO as usize + latency).next_power_of_two();
        self.input = [vec![0.0; in_len], vec![0.0; in_len]];
        self.search = vec![0.0; in_len];
        self.in_mask = in_len - 1;
        self.acc = [vec![0.0; out_len], vec![0.0; out_len]];
        self.marks = vec![Mark::default(); MARKS];
        self.out_mask = out_len - 1;
        self.lowpass = [Biquad::lowpass(sr, MARK_LOWPASS_HZ); 2];
        self.glide = sr * GLIDE_S;
        self.hold_len = (sr * HOLD_S) as usize;
        self.reset();
    }

    /// Clears audio state. Real-time safe.
    pub fn reset(&mut self) {
        for b in self.input.iter_mut().chain(self.acc.iter_mut()) {
            b.fill(0.0);
        }
        self.search.fill(0.0);
        for b in &mut self.lowpass {
            b.reset();
        }
        self.n = 0;
        self.count = 0;
        self.next_s = 0.0;
        self.voicing = 0.0;
        self.hold = 0;
        #[allow(clippy::cast_precision_loss)]
        let u = self.unvoiced as f64;
        self.lead = u;
    }

    /// Output delay in samples.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let l = self.latency as usize;
        l
    }

    /// One stereo sample in, one out (`latency` samples later).
    ///
    /// `period` is the detected period in samples (`None` while
    /// unvoiced), `pitch_ratio` the transposition (2 = octave up) and
    /// `formant_ratio` the envelope shift on top of preservation
    /// (1 = formants kept in place). Real-time safe.
    #[inline]
    pub fn tick(
        &mut self,
        left: f32,
        right: f32,
        period: Option<f32>,
        pitch_ratio: f32,
        formant_ratio: f32,
    ) -> (f32, f32) {
        let n = self.n;
        let i = self.in_index(n);
        self.input[0][i] = left;
        self.input[1][i] = right;
        let mono = 0.5 * (left + right);
        let lp = self.lowpass[0].tick(mono);
        self.search[i] = self.lowpass[1].tick(lp);
        let period = match period {
            Some(p) => {
                self.held_period = p;
                self.hold = self.hold_len;
                Some(p)
            }
            None if self.hold > 0 => {
                self.hold -= 1;
                Some(self.held_period)
            }
            None => None,
        };
        self.place_mark(period);

        let pr = pitch_ratio.clamp(1.0 / MAX_PITCH_RATIO, MAX_PITCH_RATIO);
        let fr = formant_ratio.clamp(1.0 / MAX_FORMANT_RATIO, MAX_FORMANT_RATIO);
        #[allow(clippy::cast_precision_loss)]
        let c = (n - self.latency) as f64;
        while self.next_s <= c + self.lead {
            self.place_grain(c, f64::from(pr), f64::from(fr));
        }
        #[allow(clippy::cast_possible_truncation)]
        let o = self.out_index(n - self.latency);
        let out = (self.acc[0][o], self.acc[1][o]);
        self.acc[0][o] = 0.0;
        self.acc[1][o] = 0.0;
        self.n += 1;
        out
    }

    #[inline]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn in_index(&self, i: i64) -> usize {
        (i as usize) & self.in_mask
    }

    #[inline]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn out_index(&self, i: i64) -> usize {
        (i as usize) & self.out_mask
    }

    fn mark(&self, k: usize) -> Mark {
        self.marks[k % MARKS]
    }

    fn push_mark(&mut self, pos: i64, voiced: bool) {
        self.marks[self.count % MARKS] = Mark { pos, voiced };
        self.count += 1;
    }

    /// Add an analysis mark when one is due.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn place_mark(&mut self, period: Option<f32>) {
        let n = self.n;
        let last = (self.count > 0).then(|| self.mark(self.count - 1).pos);
        let Some(p) = period.map(|p| f64::from(p).clamp(self.min_period, self.max_period)) else {
            match last {
                Some(m) if n < m + self.unvoiced => {}
                _ => self.push_mark(n, false),
            }
            return;
        };
        let radius = ((SEARCH * p) as i64).max(1);
        let pred = match last {
            Some(m) if (n - m) as f64 <= 2.0 * p => m + p.round() as i64,
            _ => n - radius,
        };
        if n < pred + radius {
            return;
        }
        let mut from = pred - radius;
        if let Some(m) = last {
            from = from.max(m + (0.5 * p) as i64);
        }
        let mut best = n;
        let mut best_v = f32::MIN;
        for k in from..=n.min(pred + radius) {
            let v = self.search[self.in_index(k)];
            if v > best_v {
                best_v = v;
                best = k;
            }
        }
        self.push_mark(best, true);
    }

    /// Overlap-add the grain for the next synthesis mark, then move the
    /// mark on by one output period.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn place_grain(&mut self, c: f64, pitch_ratio: f64, formant_ratio: f64) {
        let s = self.next_s;
        let Some(k) = self.nearest_complete(s) else {
            // Nothing complete yet (just after a reset): silence.
            self.next_s += self.unvoiced as f64;
            return;
        };
        let m = self.mark(k);
        let right = self.interval(k + 1);
        let left = if k + MARKS > self.count && k > 0 {
            self.interval(k)
        } else {
            right
        };
        let step = right / self.glide;
        self.voicing = if m.voiced {
            (self.voicing + step).min(1.0)
        } else {
            (self.voicing - step).max(0.0)
        };
        // Unvoiced grains are never shifted: noise put back at another
        // spacing would buzz.
        let (ratio, fr) = if m.voiced {
            (
                pitch_ratio.powf(self.voicing),
                formant_ratio.powf(self.voicing),
            )
        } else {
            (1.0, 1.0)
        };
        let (hl, hr) = (left / fr, right / fr);
        // A grain wider than the previous one (onset, pitch going down)
        // may be due a little late: move it, never cut its rising edge.
        let s = s.max(c + hl);
        let start = (s - hl).ceil() as i64;
        let end = (s + hr).floor() as i64;
        let a = m.pos as f64;
        // More grains per second going up, fewer going down: keep the
        // loudness where it was (same for narrower or wider grains).
        let gain = (fr / ratio).sqrt();
        for j in start..=end {
            let d = j as f64 - s;
            let w = if d < 0.0 {
                0.5 + 0.5 * (PI * d / hl).cos()
            } else {
                0.5 + 0.5 * (PI * d / hr).cos()
            };
            let src = a + d * fr;
            let o = self.out_index(j);
            let w = (w * gain) as f32;
            self.acc[0][o] += w * self.read(0, src);
            self.acc[1][o] += w * self.read(1, src);
        }
        self.next_s = s + right / ratio;
        self.lead = left.max(right) / fr;
    }

    /// Distance from mark `k - 1` to mark `k`, clamped to a period.
    #[allow(clippy::cast_precision_loss)]
    fn interval(&self, k: usize) -> f64 {
        let d = (self.mark(k).pos - self.mark(k - 1).pos) as f64;
        d.clamp(2.0, self.max_period)
    }

    /// Mark nearest to `s` whose whole grain (up to the next mark, plus
    /// the interpolator's reach) is already in the input.
    #[allow(clippy::cast_precision_loss)]
    fn nearest_complete(&self, s: f64) -> Option<usize> {
        let oldest = self.count.saturating_sub(MARKS - 1);
        let mut best: Option<(usize, f64)> = None;
        for k in (oldest..self.count.saturating_sub(1)).rev() {
            let next = self.mark(k + 1).pos;
            if next + 2 >= self.n {
                continue;
            }
            let d = (self.mark(k).pos as f64 - s).abs();
            match best {
                Some((_, bd)) if d >= bd => break,
                _ => best = Some((k, d)),
            }
        }
        best.map(|b| b.0)
    }

    /// Input of `ch` at fractional index `x` (4-point Hermite).
    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn read(&self, ch: usize, x: f64) -> f32 {
        let i = x.floor();
        let t = (x - i) as f32;
        let i = i as i64;
        let buf = &self.input[ch];
        let y1 = buf[self.in_index(i)];
        if t == 0.0 {
            return y1;
        }
        let y0 = buf[self.in_index(i - 1)];
        let y2 = buf[self.in_index(i + 1)];
        let y3 = buf[self.in_index(i + 2)];
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{PitchDetector, Yin};

    const SR: f64 = 48_000.0;
    const LAT: usize = 480;

    /// Voice-like source through two formant resonances (vowel "a"),
    /// so the envelope does not move with the pitch.
    fn vowel(hz: f64, secs: f64) -> Vec<f32> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (SR * secs) as usize;
        let mut x = Vec::with_capacity(n);
        let mut phase = 0.0;
        for _ in 0..n {
            phase += hz / SR;
            phase -= phase.floor();
            // Glottal-like pulse: sharp closure once per period.
            x.push((1.0 - 2.0 * phase) as f32);
        }
        let mut out = vec![0.0; n];
        for (f, bw, g) in [(750.0, 90.0, 1.0), (1_200.0, 110.0, 0.6)] {
            let (b0, a1, a2) = resonator(f, bw);
            let (mut y1, mut y2) = (0.0, 0.0);
            for (o, &v) in out.iter_mut().zip(&x) {
                let y = b0 * f64::from(v) - a1 * y1 - a2 * y2;
                y2 = y1;
                y1 = y;
                #[allow(clippy::cast_possible_truncation)]
                let add = (g * y) as f32;
                *o += add;
            }
        }
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        out.iter().map(|v| 0.3 * v / peak).collect()
    }

    fn resonator(f: f64, bw: f64) -> (f64, f64, f64) {
        let r = (-PI * bw / SR).exp();
        let a1 = -2.0 * r * (2.0 * PI * f / SR).cos();
        let a2 = r * r;
        (1.0 - r, a1, a2)
    }

    /// Run a mono signal through the detector and the shifter.
    fn shift(x: &[f32], pitch_st: f32, formant_st: f32) -> Vec<f32> {
        let mut det = Yin::new(SR);
        let mut ps = Psola::new(SR, LAT);
        let (pr, fr) = (semitones_to_ratio(pitch_st), semitones_to_ratio(formant_st));
        x.iter()
            .map(|&v| {
                det.push_sample(v);
                #[allow(clippy::cast_possible_truncation)]
                let period = det.estimate().map(|e| (SR / f64::from(e.hz)) as f32);
                ps.tick(v, v, period, pr, fr).0
            })
            .collect()
    }

    fn median_pitch(x: &[f32]) -> f32 {
        let mut det = Yin::new(SR);
        let mut hz = Vec::new();
        for chunk in x.chunks(120) {
            det.push(chunk);
            if let Some(e) = det.estimate() {
                hz.push(e.hz);
            }
        }
        assert!(hz.len() > 20, "output unvoiced");
        hz.sort_by(f32::total_cmp);
        hz[hz.len() / 2]
    }

    /// Spectral centroid over 0-4 kHz (naive DFT, Hann window).
    fn centroid(x: &[f32]) -> f64 {
        let n = x.len();
        #[allow(clippy::cast_precision_loss)]
        let nf = n as f64;
        let (mut num, mut den) = (0.0, 0.0);
        let bins = (4_000.0 / SR * nf) as usize;
        for b in 1..bins {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in x.iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f64;
                let w = 0.5 - 0.5 * (2.0 * PI * t / nf).cos();
                let ph = 2.0 * PI * b as f64 * t / nf;
                re += w * f64::from(v) * ph.cos();
                im -= w * f64::from(v) * ph.sin();
            }
            let mag = re.hypot(im);
            num += b as f64 * mag;
            den += mag;
        }
        num / den * SR / nf
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn transparent_at_unity() {
        // Noise (unvoiced marks) and a vowel (voiced marks).
        let mut s: u32 = 7;
        let noise: Vec<f32> = (0..9_600)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                #[allow(clippy::cast_precision_loss)]
                let v = (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                0.3 * v
            })
            .collect();
        let y = shift(&noise, 0.0, 0.0);
        for i in LAT + 600..noise.len() {
            assert!((y[i] - noise[i - LAT]).abs() < 1e-5, "noise, sample {i}");
        }
        let x = vowel(220.0, 0.5);
        let y = shift(&x, 0.0, 0.0);
        // Voiced grains need two periods: at 220 Hz a little more than
        // the budget, so the content may come a few samples later.
        let tail = 12_000..x.len();
        let err = (0..120)
            .map(|extra| {
                let e: f32 = tail
                    .clone()
                    .map(|i| (y[i] - x[i - LAT - extra]).powi(2))
                    .sum();
                let s: f32 = tail.clone().map(|i| x[i - LAT - extra].powi(2)).sum();
                e / s
            })
            .fold(f32::MAX, f32::min);
        // What is left is a one-sample wobble of the grain placement.
        assert!(err < 5e-3, "vowel, error {err:.2e}");
    }

    #[test]
    fn transposes_by_the_asked_interval() {
        for hz in [130.0, 196.0, 330.0] {
            for st in [-12.0, -5.0, 3.0, 7.0, 12.0] {
                let x = vowel(hz, 0.6);
                let y = shift(&x, st, 0.0);
                let got = median_pitch(&y[9_600..]);
                #[allow(clippy::cast_possible_truncation)]
                let want = hz as f32 * semitones_to_ratio(st);
                let c = cents(got, want);
                assert!(
                    c.abs() < 10.0,
                    "{hz} Hz {st:+} st: {got:.1} Hz ({c:+.1} ct)"
                );
                // Loudness roughly kept (the harmonics move under fixed
                // formants, so the level does change a little).
                let rms =
                    |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
                let db = 20.0 * (rms(&y[9_600..]) / rms(&x[9_600..])).log10();
                assert!(db.abs() < 6.0, "{hz} Hz {st:+} st: {db:+.1} dB");
            }
        }
    }

    #[test]
    fn formants_stay_unless_asked() {
        let x = vowel(196.0, 0.6);
        let at = |y: &[f32]| centroid(&y[14_000..16_048]);
        let base = at(&shift(&x, 0.0, 0.0));
        for st in [-12.0, 12.0] {
            let kept = at(&shift(&x, st, 0.0));
            assert!(
                (kept / base - 1.0).abs() < 0.25,
                "{st:+} st: centroid {kept:.0} vs {base:.0} Hz"
            );
        }
        // Plain resampling (formants following the pitch) for scale.
        let chipmunk = at(&shift(&x, 12.0, 12.0));
        assert!(
            chipmunk / base > 1.6,
            "chipmunk: {chipmunk:.0} vs {base:.0}"
        );
        let up = at(&shift(&x, 0.0, 7.0));
        let down = at(&shift(&x, 0.0, -7.0));
        assert!(up / base > 1.25, "formant up: {up:.0} vs {base:.0}");
        assert!(down / base < 0.8, "formant down: {down:.0} vs {base:.0}");
        // The formant shift leaves the pitch alone.
        let y = shift(&x, 0.0, 7.0);
        assert!(cents(median_pitch(&y[9_600..]), 196.0).abs() < 10.0);
    }

    #[test]
    fn low_voices_and_resets_are_handled() {
        let x = vowel(70.0, 0.6);
        let y = shift(&x, 7.0, 0.0);
        #[allow(clippy::cast_possible_truncation)]
        let want = 70.0 * semitones_to_ratio(7.0);
        assert!(cents(median_pitch(&y[9_600..]), want).abs() < 15.0);
        let mut ps = Psola::new(96_000.0, 960);
        for i in 0..20_000 {
            #[allow(clippy::cast_precision_loss)]
            let v = (i as f32 * 0.01).sin();
            let (l, r) = ps.tick(v, -v, Some(400.0), 0.25, 2.0);
            assert!(l.is_finite() && r.is_finite());
        }
        ps.reset();
        assert_eq!(ps.tick(0.0, 0.0, None, 1.0, 1.0), (0.0, 0.0));
        assert_eq!(ps.latency_samples(), 960);
    }
    /// Rough sung onset: breath, then a jittery vowel scooping up to hz.
    fn sung(hz: f64, seed: u32) -> Vec<f32> {
        let sr = 48_000.0;
        let mut x = vec![0.0f32; 4800];
        let mut s = seed;
        let mut rnd = move || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f64 / (1u32 << 24) as f64 - 0.5
        };
        let n = 28_800;
        let mut ph = 0.0f64;
        let mut src = vec![0.0f64; n];
        let mut jit = 0.0;
        for (i, v) in src.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f64 / sr;
            let scoop = if t < 0.06 {
                -1.5 * (1.0 - t / 0.06)
            } else {
                0.0
            };
            let f = hz * ((scoop + jit) / 12.0f64).exp2();
            let prev = ph;
            ph += f / sr;
            if ph.floor() != prev.floor() {
                jit = 0.15 * rnd() * if t < 0.08 { 3.0 } else { 1.0 };
            }
            let env_v = (t / 0.03).min(1.0);
            let breath = 0.15 * (1.0 - (t / 0.08).min(1.0)) + 0.02;
            *v = env_v * (1.0 - 2.0 * (ph - ph.floor())) + breath * rnd() * 4.0;
        }
        let mut out = vec![0.0f64; src.len()];
        for (f, bw, g) in [
            (700.0, 90.0, 1.0),
            (1_150.0, 110.0, 0.6),
            (2_600.0, 160.0, 0.2),
        ] {
            let r = (-std::f64::consts::PI * bw / sr).exp();
            let a1 = -2.0 * r * (2.0 * std::f64::consts::PI * f / sr).cos();
            let a2 = r * r;
            let (mut y1, mut y2) = (0.0, 0.0);
            for (o, &v) in out.iter_mut().zip(&src) {
                let y = (1.0 - r) * v - a1 * y1 - a2 * y2;
                y2 = y1;
                y1 = y;
                *o += g * y;
            }
        }
        let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        x.extend(out.iter().map(|v| (0.3 * v / peak) as f32));
        x.extend(vec![0.0; 2400]);
        x
    }

    /// High-frequency energy (second difference) per 5 ms block.
    fn hf_blocks(y: &[f32]) -> Vec<f32> {
        let d2: Vec<f32> = y
            .windows(3)
            .map(|w| (w[2] - 2.0 * w[1] + w[0]).powi(2))
            .collect();
        d2.chunks(240).map(|c| c.iter().sum()).collect()
    }

    #[test]
    fn onsets_glide_without_clicks() {
        for (hz, st) in [(196.0, 7.0), (220.0, -12.0), (330.0, 12.0), (120.0, 5.0)] {
            let x = sung(hz, 3);
            let y = shift(&x, st, 0.0);
            let e = hf_blocks(&y);
            let mut steady = e[100..130].to_vec();
            steady.sort_by(f32::total_cmp);
            let med = steady[15];
            // Breath, scoop and the first 60 ms of the note.
            for (b, v) in e[20..44].iter().enumerate() {
                assert!(
                    v / med < 2.5,
                    "{hz} Hz {st:+} st, block {b}: {:.1}x",
                    v / med
                );
            }
            #[allow(clippy::cast_possible_truncation)]
            let want = hz as f32 * semitones_to_ratio(st);
            assert!(cents(median_pitch(&y[9_600..]), want).abs() < 20.0);
        }
    }
}
