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
//! Latency: the output is the input timeline delayed by the latency
//! set with [`Psola::set_latency`], so dry, wet and bypass line up. A
//! grain starts sounding as soon as its pulse mark is placed and reads
//! its right half while it plays, so a voice of period `P` needs
//! [`MARK_DELAY`] + 1 periods (1.6 P: 8.2 ms at 196 Hz, 14.6 ms at
//! 110 Hz), see [`Psola::latency_for`]. A voice lower than the latency
//! allows is delayed a little more (no click, no change of reported
//! latency).
//!
//! Real-time: every buffer is allocated in [`Psola::prepare`]; `tick`
//! does no allocation, no lock and a bounded amount of work.

use std::f64::consts::PI;

use crate::pitch::{Biquad, MAX_HZ, MIN_HZ};

/// Spacing of the unvoiced marks. Grains of unvoiced input are put back
/// unshifted; the spacing only has to be shorter than the latency.
const UNVOICED_S: f64 = 0.0025;
/// Peak search around the predicted mark, in periods.
const SEARCH: f64 = 0.3;
/// Longest time between a pulse and its mark being placed, in periods:
/// the search looks `SEARCH` past the predicted pulse, which can come
/// up to `SEARCH` early.
pub const MARK_DELAY: f64 = 2.0 * SEARCH;
/// Low-pass of the mark search signal: keeps the fundamental's peak.
const MARK_LOWPASS_HZ: f64 = 1_000.0;
/// Time the voicing glides over when the voice starts or stops: the
/// first grains of a note move from the sung pitch to the target (a
/// short scoop) instead of jumping, which would click.
const GLIDE_S: f64 = 0.012;
/// A detector dropout shorter than this keeps the last period, so a
/// rough onset does not flip between shifted and unshifted grains.
const HOLD_S: f64 = 0.010;
/// How fast the grain delay comes back once a long period no longer
/// needs it, as a fraction of the time played (3 %: 5 ms in ~170 ms).
const OFFSET_RECOVERY: f64 = 0.03;
/// Input whose high band (second difference) carries more than this
/// share of the energy, at 44.1 kHz, is noise or a consonant burst, even
/// if the detector calls it voiced: shifting it as voiced would double
/// the burst (one copy per overlapping grain).
const NOISY_RATIO_44K: f64 = 0.15;
/// Time constant of the noisiness measure.
const NOISY_S: f64 = 0.002;
/// Analysis marks remembered (several periods of the lowest voice).
const MARKS: usize = 256;
/// Grains sounding at once, at most (two octaves up with the formant
/// down still fit).
const GRAINS: usize = 48;
/// Widest formant shift supported by the buffers (ratio 0.5 to 2).
pub const MAX_FORMANT_RATIO: f32 = 2.0;
/// Widest pitch shift supported (ratio 0.25 to 4, two octaves).
pub const MAX_PITCH_RATIO: f32 = 4.0;

pub const CHANNELS: usize = 2;

/// Decides the pitch ratio of each grain. The autotune
/// ([`crate::autotune::Retuner`]) works here, on the grain's own period,
/// so its correction is in time with the waveform; a plain `f32` is a
/// fixed ratio.
pub trait PitchControl {
    /// Ratio for a voiced grain of `period` samples (distance between
    /// its pulse marks), `dt` seconds after the previous grain.
    fn voiced(&mut self, period: f64, dt: f64) -> f64;
    /// An unvoiced grain (never transposed), `dt` seconds after the
    /// previous one.
    fn unvoiced(&mut self, _dt: f64) {}
}

impl PitchControl for f32 {
    fn voiced(&mut self, _period: f64, _dt: f64) -> f64 {
        f64::from(*self)
    }
}

impl PitchControl for crate::autotune::Retuner {
    fn voiced(&mut self, period: f64, dt: f64) -> f64 {
        Self::voiced(self, period, dt)
    }
    fn unvoiced(&mut self, dt: f64) {
        Self::unvoiced(self, dt);
    }
}

/// Semitones to pitch ratio.
#[must_use]
pub fn semitones_to_ratio(st: f32) -> f32 {
    (st / 12.0).exp2()
}

/// A grain being played: placed as soon as its pulse mark is known,
/// read from the input while it sounds (its right half is still being
/// recorded when it starts).
#[derive(Debug, Clone, Copy, Default)]
struct Grain {
    /// Synthesis centre, on the output timeline.
    s: f64,
    /// Analysis centre (its mark), input index.
    a: f64,
    /// Left and right half widths, in output samples.
    hl: f64,
    hr: f64,
    /// Input samples read per output sample (the formant ratio).
    fr: f64,
    gain: f32,
    /// Share of each half that fades (1: Hann halves). Below 1 the
    /// grain rises over that share of the previous period only, and its
    /// right half stays flat until the next grain's rise.
    attack: f64,
    /// Its mark, to size the right half once the next mark is known.
    k: usize,
    right_known: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Mark {
    /// Input sample index.
    pos: i64,
    /// Expected distance to the next mark, until that one is placed.
    period: f32,
    voiced: bool,
}

/// Real-time pitch-synchronous shifter. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Psola {
    sample_rate: f64,
    /// Synthesis position of the previous grain (for `PitchControl`).
    last_s: f64,
    /// Share of a period the voiced grains fade in over (see
    /// [`Psola::set_attack`]).
    attack: f64,
    /// Mark and pitch ratio of the previous grain: the next synthesis
    /// mark is one of its periods later, measured once its next mark is
    /// placed.
    last_grain: Option<(usize, f64)>,
    latency: i64,
    max_latency: usize,
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
    /// Grains sounding, `active` of them.
    grains: Vec<Grain>,
    active: usize,
    /// Next synthesis mark, on the input timeline.
    next_s: f64,
    /// Synthesis spacing of the last grain (the time it played).
    step: f64,
    /// 0 (unvoiced, unshifted) to 1 (voiced, fully shifted).
    voicing: f64,
    glide: f64,
    /// Last detected period and how many samples it is still held for.
    held_period: f32,
    hold: usize,
    hold_len: usize,
    /// Delay of the grains behind the synthesis marks, in samples.
    offset: f64,
    /// Smoothed voiced period, kept through consonants and pauses.
    sung_period: f64,
    /// Noisiness measure: last two mono samples, band energies.
    prev: [f32; 2],
    e_full: f32,
    e_high: f32,
    noisy_coef: f32,
    noisy_ratio: f32,
}

impl Psola {
    #[must_use]
    pub fn new(sample_rate: f64, latency: usize) -> Self {
        let mut s = Self::default();
        s.prepare(sample_rate, latency);
        s
    }

    /// Latency a voice down to `lowest_hz` needs at `sample_rate`, in
    /// samples, with grains fading in over `attack` of a period (1 for
    /// the default grains).
    #[must_use]
    pub fn latency_for(sample_rate: f64, lowest_hz: f64, attack: f64) -> usize {
        let p = sample_rate / lowest_hz.max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (p * (attack.clamp(0.1, 1.0) + MARK_DELAY) + 3.0).ceil() as usize;
        n.max(Self::unvoiced_spacing(sample_rate) + 1)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn unvoiced_spacing(sample_rate: f64) -> usize {
        ((sample_rate * UNVOICED_S).round() as usize).max(16)
    }

    /// Allocates for `sample_rate` and output delays up to `latency`
    /// samples, and sets that delay. Not for the audio thread.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn prepare(&mut self, sample_rate: f64, latency: usize) {
        let sr = sample_rate.max(8_000.0);
        self.sample_rate = sr;
        if self.attack == 0.0 {
            self.attack = 1.0;
        }
        self.max_latency = latency;
        self.latency = latency as i64;
        self.unvoiced = Self::unvoiced_spacing(sr) as i64;
        self.min_period = sr / f64::from(MAX_HZ);
        self.max_period = sr / f64::from(MIN_HZ);
        let pmax = self.max_period.ceil() as usize;
        // Oldest read: an output delay, the extra delay of low voices
        // and a grain's left half, all at most a few long periods.
        let in_len = (2 * (latency + 6 * pmax)).next_power_of_two();
        self.input = [vec![0.0; in_len], vec![0.0; in_len]];
        self.search = vec![0.0; in_len];
        self.in_mask = in_len - 1;
        self.grains = vec![Grain::default(); GRAINS];
        self.marks = vec![Mark::default(); MARKS];
        self.lowpass = [Biquad::lowpass(sr, MARK_LOWPASS_HZ); 2];
        self.glide = sr * GLIDE_S;
        self.noisy_coef = (1.0 - (-1.0 / (sr * NOISY_S)).exp()) as f32;
        self.noisy_ratio = (NOISY_RATIO_44K * (44_100.0 / sr).powi(4)) as f32;
        self.hold_len = (sr * HOLD_S) as usize;
        self.reset();
    }

    /// Clears audio state. Real-time safe.
    pub fn reset(&mut self) {
        for b in &mut self.input {
            b.fill(0.0);
        }
        self.active = 0;
        self.search.fill(0.0);
        for b in &mut self.lowpass {
            b.reset();
        }
        self.n = 0;
        self.count = 0;
        self.next_s = 0.0;
        self.last_s = 0.0;
        self.last_grain = None;
        self.voicing = 0.0;
        self.hold = 0;
        self.offset = 0.0;
        self.sung_period = 0.0;
        self.prev = [0.0; 2];
        self.e_full = 0.0;
        self.e_high = 0.0;
        #[allow(clippy::cast_precision_loss)]
        let u = self.unvoiced as f64;
        self.step = u;
    }

    /// Output delay, up to the one given to [`Psola::prepare`]. Clears
    /// the audio state (the output timeline moves). Real-time safe.
    pub fn set_latency(&mut self, latency: usize) {
        #[allow(clippy::cast_possible_wrap)]
        let l = latency.min(self.max_latency) as i64;
        if l != self.latency {
            self.latency = l;
            self.reset();
        }
    }

    /// Output delay in samples.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let l = self.latency as usize;
        l
    }

    /// Share of the previous period a voiced grain fades in over, 0.1
    /// to 1 (1, the default: classic two-period Hann grains). Shorter
    /// attacks let a grain start sooner after its pulse is found, which
    /// is what low voices need to stay within the latency.
    pub fn set_attack(&mut self, attack: f64) {
        self.attack = attack.clamp(0.1, 1.0);
    }

    /// Extra delay of the grains behind the output delay, in samples
    /// (what low voices add to [`Psola::latency_samples`]).
    #[must_use]
    pub fn grain_delay(&self) -> f64 {
        self.offset
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
        let mut pr = pitch_ratio;
        self.tick_with(left, right, period, &mut pr, formant_ratio)
    }

    /// [`Psola::tick`] with the pitch ratio decided per grain by `pitch`.
    #[inline]
    pub fn tick_with(
        &mut self,
        left: f32,
        right: f32,
        period: Option<f32>,
        pitch: &mut impl PitchControl,
        formant_ratio: f32,
    ) -> (f32, f32) {
        let n = self.n;
        let i = self.in_index(n);
        self.input[0][i] = left;
        self.input[1][i] = right;
        let mono = 0.5 * (left + right);
        let lp = self.lowpass[0].tick(mono);
        self.search[i] = self.lowpass[1].tick(lp);
        // Noisiness: high band against full band, over ~2 ms.
        let d2 = mono - 2.0 * self.prev[0] + self.prev[1];
        self.prev = [mono, self.prev[0]];
        self.e_full += self.noisy_coef * (mono * mono - self.e_full);
        self.e_high += self.noisy_coef * (d2 * d2 - self.e_high);
        let noisy = self.e_high > self.noisy_ratio * self.e_full;
        let period = match period {
            _ if noisy => {
                self.hold = 0;
                None
            }
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

        let fr = formant_ratio.clamp(1.0 / MAX_FORMANT_RATIO, MAX_FORMANT_RATIO);
        #[allow(clippy::cast_precision_loss)]
        let c = (n - self.latency) as f64;
        // A grain is placed when its left half starts, not before: its
        // mark must be known by then, and no earlier.
        while self.due(c, f64::from(fr)) {
            if !self.place_grain(c, pitch, f64::from(fr)) {
                break;
            }
        }
        let out = self.render(c);
        self.n += 1;
        out
    }

    /// Output sample `c`: the sum of the grains sounding there.
    #[inline]
    fn render(&mut self, c: f64) -> (f32, f32) {
        // Never read input that is not recorded yet (the Hermite reads
        // two samples past the index).
        #[allow(clippy::cast_precision_loss)]
        let newest = (self.n - 2) as f64;
        let (mut l, mut r) = (0.0f32, 0.0f32);
        let mut i = 0;
        while i < self.active {
            let mut g = self.grains[i];
            let d = c - g.s;
            if d < -g.hl {
                i += 1;
                continue;
            }
            if !g.right_known && d >= (1.0 - g.attack) * g.hr {
                // The right half reaches the next mark, known by now
                // (or the expected period if the voice just stopped).
                if g.k + 1 < self.count {
                    g.hr = self.right(g.k) / g.fr;
                    g.right_known = true;
                }
                self.grains[i] = g;
            }
            if d > g.hr {
                self.active -= 1;
                self.grains[i] = self.grains[self.active];
                continue;
            }
            let fall = g.attack * g.hr;
            let w = if d < 0.0 {
                0.5 + 0.5 * (PI * d / g.hl).cos()
            } else if d <= g.hr - fall {
                1.0
            } else {
                0.5 + 0.5 * (PI * (d - (g.hr - fall)) / fall).cos()
            };
            let src = (g.a + d * g.fr).min(newest);
            #[allow(clippy::cast_possible_truncation)]
            let w = w as f32 * g.gain;
            l += w * self.read(0, src);
            r += w * self.read(1, src);
            i += 1;
        }
        (l, r)
    }

    #[inline]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn in_index(&self, i: i64) -> usize {
        (i as usize) & self.in_mask
    }

    fn mark(&self, k: usize) -> Mark {
        self.marks[k % MARKS]
    }

    fn push_mark(&mut self, pos: i64, period: f64, voiced: bool) {
        if voiced {
            let p = period;
            // Smoothed over ~10 periods, so a stray reading at the end of a
            // note does not move it.
            self.sung_period = if self.sung_period > 0.0 {
                (self.sung_period.ln() + 0.1 * (p.ln() - self.sung_period.ln())).exp()
            } else {
                p
            };
        }
        #[allow(clippy::cast_possible_truncation)]
        let period = period as f32;
        self.marks[self.count % MARKS] = Mark {
            pos,
            period,
            voiced,
        };
        self.count += 1;
    }

    /// Add an analysis mark when one is due.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn place_mark(&mut self, period: Option<f32>) {
        let n = self.n;
        let last = (self.count > 0).then(|| self.mark(self.count - 1).pos);
        let Some(p) = period.map(|p| f64::from(p).clamp(self.min_period, self.max_period)) else {
            // (the sung period is remembered through unvoiced parts)
            match last {
                Some(m) if n < m + self.unvoiced => {}
                #[allow(clippy::cast_precision_loss)]
                _ => self.push_mark(n, self.unvoiced as f64, false),
            }
            return;
        };
        let radius = ((SEARCH * p) as i64).max(1);
        let pred = match last {
            Some(m) if (n - m) as f64 <= 2.0 * p => m + p.round() as i64,
            _ => n - radius,
        };
        // Previous voiced mark close enough to follow by waveform
        // similarity rather than by peak.
        let prev_voiced = self.count > 0 && {
            let m = self.mark(self.count - 1);
            m.voiced && ((n - m.pos) as f64) <= 2.0 * p
        };
        let half = ((0.25 * p) as i64).max(4);
        if prev_voiced {
            if n < pred + radius {
                return;
            }
            let m = last.unwrap_or(0);
            let from = (pred - radius).max(m + (0.5 * p) as i64);
            let to = pred + radius;
            let at = |k: i64| self.search[self.in_index(k)];
            let peak = (from..=to).map(at).fold(f32::MIN, f32::max);
            // Candidates: the strong peaks (grains stay centred on the
            // pulses); the one whose last half period looks most like the
            // previous mark's wins, so the marks do not flip between two
            // similar peaks from one period to the next.
            let mut best = (from..=to).find(|&k| at(k) >= peak).unwrap_or(pred);
            let mut best_v = f32::MIN;
            for k in from..=to {
                let v = at(k);
                if v < 0.5 * peak || v < at(k - 1) || (k < n && v < at(k + 1)) {
                    continue;
                }
                let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
                for j in -2 * half..=0 {
                    let a = self.search[self.in_index(m + j)];
                    let b = self.search[self.in_index(k + j)];
                    ab += a * b;
                    aa += a * a;
                    bb += b * b;
                }
                let v = ab / (aa * bb).sqrt().max(1e-12);
                if v > best_v {
                    best_v = v;
                    best = k;
                }
            }
            self.push_mark(best, p, true);
            return;
        }
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
        self.push_mark(best, p, true);
    }

    /// Overlap-add the grain for the next synthesis mark, then move the
    /// mark on by one output period.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    /// Returns `false` when the grain must wait for more input.
    fn place_grain(&mut self, c: f64, pitch: &mut impl PitchControl, formant_ratio: f64) -> bool {
        self.next_s = self.next_centre();
        let s = self.next_s;
        // The grains play the input `offset` samples behind the synthesis
        // marks. A voiced grain needs two periods of input, an unvoiced
        // one much less: following that difference at every consonant
        // would skip or repeat a few milliseconds each time (a doubled
        // consonant, a jump). So the delay stays at what the sung period
        // needs, through consonants and pauses, and only comes back
        // slowly. When a grain needs more (first note, a lower note), the
        // output waits for it: a few milliseconds of gap instead of a
        // repeat.
        //
        // A grain starts sounding (its left half) as soon as its mark is
        // placed, and its right half is read while it plays: the input
        // it needs arrives as fast as it is played. So the delay only has
        // to cover the left half and the time a mark takes to be placed
        // (the peak search looks up to `SEARCH` periods past it, twice
        // that when the peak comes early), not the whole grain.
        let p = self.sung_period;
        let fr = formant_ratio;
        #[allow(clippy::cast_precision_loss)]
        let floor = ((p * (self.attack / fr + MARK_DELAY)).max(p * (1.0 - 1.0 / fr)) + 3.0
            - self.latency as f64)
            .max(0.0);
        self.offset = (self.offset - OFFSET_RECOVERY * self.step).max(floor);
        let target = s - self.offset;
        let Some(k) = self.nearest_mark(target) else {
            // No mark yet (just after a reset): silence.
            self.next_s += self.unvoiced as f64;
            self.last_grain = None;
            return true;
        };
        let m = self.mark(k);
        if k + 1 == self.count && target - (m.pos as f64) > 0.5 * f64::from(m.period) {
            // The next mark, nearer, is not placed yet: wait for it rather
            // than repeat this one.
            return false;
        }
        let right = self.right(k);
        let left = self.left(k);
        let step = right / self.glide;
        self.voicing = if m.voiced {
            (self.voicing + step).min(1.0)
        } else {
            (self.voicing - step).max(0.0)
        };
        // The formant applies from the first sample, voiced or not: the
        // timbre does not switch when the detector makes up its mind.
        // Only the pitch glides in, and unvoiced grains are never
        // transposed (noise put back at another spacing would buzz).
        // A grain due late (after waiting, or wider than the previous one)
        // is moved, never cut on its rising edge; the delay follows so the
        // input plays on from where it was.
        // (Sample `c` itself is still to be played: it is rendered after
        // this. Counting it as late would delay every grain by half a
        // sample on average and leave the output a few cents flat.)
        let hl = self.rise(k, fr);
        let late = (c - 1.0 + hl - s).max(0.0);
        let s = s + late;
        self.offset += late;
        let dt = ((s - self.last_s) / self.sample_rate).max(0.0);
        self.last_s = s;
        let (ratio, hr, right_known) = if m.voiced {
            let max = f64::from(MAX_PITCH_RATIO);
            // The grain's own period (the last one, its next mark is not
            // placed yet), unless a mark landed off the pulses (consonant
            // onsets): then the detector's.
            let last = if k + 1 < self.count { right } else { left };
            let own = if (last / f64::from(m.period)).ln().abs() < 0.2 {
                last
            } else {
                f64::from(m.period)
            };
            let pr = pitch.voiced(own, dt).clamp(1.0 / max, max);
            (pr.powf(self.voicing), right / fr, k + 1 < self.count)
        } else {
            pitch.unvoiced(dt);
            // Kept at the analysis spacing, so the grains still sum to
            // one; each is read faster or slower, which moves its
            // envelope, without reading past the recorded input.
            #[allow(clippy::cast_precision_loss)]
            let room = if fr > 1.0 {
                ((s - m.pos as f64 + self.latency as f64 - 3.0) / (fr - 1.0)).max(1.0)
            } else {
                f64::MAX
            };
            (1.0, right.min(room), true)
        };
        // More grains per second going up, fewer going down: keep the
        // loudness where it was (same for narrower or wider grains).
        let gain = if m.voiced { (fr / ratio).sqrt() } else { 1.0 };
        if self.active < GRAINS {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let g = Grain {
                s,
                a: m.pos as f64,
                hl,
                hr,
                fr,
                gain: gain as f32,
                attack: if m.voiced { self.attack } else { 1.0 },
                k,
                right_known,
            };
            self.grains[self.active] = g;
            self.active += 1;
        }
        self.next_s = s + right / ratio;
        self.last_grain = Some((k, ratio));
        self.step = right / ratio;
        true
    }

    /// Synthesis centre of the next grain: one period of the previous
    /// grain on, measured now that its next mark may be known (it was
    /// only expected when that grain started).
    fn next_centre(&self) -> f64 {
        match self.last_grain {
            Some((k, ratio)) if k + MARKS > self.count => self.last_s + self.right(k) / ratio,
            _ => self.next_s,
        }
    }

    /// Whether the next grain starts at output sample `c` (or should
    /// have): the grain it would play, its rise reaches `c`.
    fn due(&self, c: f64, formant_ratio: f64) -> bool {
        let s = self.next_centre();
        #[allow(clippy::cast_precision_loss)]
        let rise = match self.nearest_mark(s - self.offset) {
            Some(k) => self.rise(k, formant_ratio),
            None => self.unvoiced as f64,
        };
        s - rise <= c + 1.0
    }

    /// Left half of mark `k`'s grain: back to the previous mark.
    fn left(&self, k: usize) -> f64 {
        if k + MARKS > self.count && k > 0 {
            self.interval(k)
        } else {
            self.right(k)
        }
    }

    /// How long mark `k`'s grain rises before its centre, in output
    /// samples.
    fn rise(&self, k: usize, formant_ratio: f64) -> f64 {
        let left = self.left(k);
        if self.mark(k).voiced {
            self.attack * left
                / formant_ratio.clamp(
                    1.0 / f64::from(MAX_FORMANT_RATIO),
                    f64::from(MAX_FORMANT_RATIO),
                )
        } else {
            left
        }
    }

    /// Distance from mark `k - 1` to mark `k`, clamped to a period.
    #[allow(clippy::cast_precision_loss)]
    fn interval(&self, k: usize) -> f64 {
        let d = (self.mark(k).pos - self.mark(k - 1).pos) as f64;
        d.clamp(2.0, self.max_period)
    }

    /// Right half of mark `k`'s grain: up to the next mark, or the
    /// expected period while that one is not placed yet.
    fn right(&self, k: usize) -> f64 {
        if k + 1 < self.count {
            self.interval(k + 1)
        } else {
            f64::from(self.mark(k).period).clamp(2.0, self.max_period)
        }
    }

    /// Placed mark nearest to `s`.
    #[allow(clippy::cast_precision_loss)]
    fn nearest_mark(&self, s: f64) -> Option<usize> {
        let oldest = self.count.saturating_sub(MARKS - 1);
        let mut best: Option<(usize, f64)> = None;
        for k in (oldest..self.count).rev() {
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
    fn voice_is_delayed_by_its_latency_only() {
        // A grain plays as soon as its mark is known: about 1.6 periods,
        // where pre-rendered grains needed 3.3. Exact at unity, with
        // the default grains and with a short attack.
        for (hz, attack) in [(110.0, 1.0), (196.0, 1.0), (110.0, 0.5)] {
            let lat = Psola::latency_for(SR, hz, attack);
            assert!(lat as f64 <= (1.0 + MARK_DELAY) * SR / hz + 4.0);
            let x = vowel(hz, 0.5);
            let mut det = Yin::new(SR);
            let mut ps = Psola::new(SR, lat);
            ps.set_attack(attack);
            let mut extra = 0.0f64;
            let y: Vec<f32> = x
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    det.push_sample(v);
                    #[allow(clippy::cast_possible_truncation)]
                    let period = det.estimate().map(|e| (SR / f64::from(e.hz)) as f32);
                    let out = ps.tick(v, v, period, 1.0, 1.0).0;
                    if i > 12_000 {
                        extra = extra.max(ps.grain_delay());
                    }
                    out
                })
                .collect();
            assert!(extra < 1.0, "{hz} Hz: {extra} samples more");
            let tail = 12_000..x.len();
            let e: f32 = tail.clone().map(|i| (y[i] - x[i - lat]).powi(2)).sum();
            let s: f32 = tail.map(|i| x[i - lat].powi(2)).sum();
            assert!(
                e / s < 1e-4,
                "{hz} Hz, attack {attack}: error {:.2e}",
                e / s
            );
        }
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
    fn formant_moves_unvoiced_sounds_too() {
        // A whisper (noise through one resonance): never voiced, so the
        // formant must apply without waiting for the detector.
        let (b0, a1, a2) = resonator(1200.0, 300.0);
        let mut seed = 1u32;
        let (mut y1, mut y2) = (0.0, 0.0);
        let x: Vec<f32> = (0..24_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let v = f64::from(seed >> 8) / f64::from(1u32 << 24) - 0.5;
                let y = b0 * v - a1 * y1 - a2 * y2;
                (y2, y1) = (y1, y);
                #[allow(clippy::cast_possible_truncation)]
                let y = y as f32;
                y
            })
            .collect();
        let rms = |y: &[f32]| (y.iter().map(|v| v * v).sum::<f32>() / y.len() as f32).sqrt();
        let base = shift(&x, 0.0, 0.0);
        let up = shift(&x, 0.0, 7.0);
        let (b, u) = (
            centroid(&base[14_000..16_048]),
            centroid(&up[14_000..16_048]),
        );
        assert!(u / b > 1.2, "whisper formant up: {u:.0} vs {b:.0} Hz");
        let db = 20.0 * (rms(&up[4_000..]) / rms(&base[4_000..])).log10();
        assert!(db.abs() < 3.0, "whisper level {db:+.1} dB");
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
