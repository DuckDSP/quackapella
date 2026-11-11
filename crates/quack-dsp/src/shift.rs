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
/// Widest formant shift supported by the buffers (ratio 0.5 to 2).
pub const MAX_FORMANT_RATIO: f32 = 2.0;
/// Widest pitch shift supported (ratio 0.25 to 4, two octaves).
pub const MAX_PITCH_RATIO: f32 = 4.0;

pub const CHANNELS: usize = 2;

/// Extra voices built on the main voice's grains, at a fixed interval
/// from it: octave down, two octaves down (sub), octave up. They follow
/// the main voice's pitch (correction included), only sound on voiced
/// grains, and fade in with the voicing.
pub const LAYERS: usize = 3;
/// Pitch ratio of each layer to the main voice.
pub const LAYER_RATIOS: [f64; LAYERS] = [0.5, 0.25, 2.0];
/// Freeze crossfade, in and out.
const FREEZE_FADE_S: f64 = 0.025;
/// How far back a freeze looks for a voiced grain to hold (marks).
const FREEZE_LOOKBACK: usize = 48;

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

#[derive(Debug, Clone, Copy, Default)]
struct Mark {
    /// Input sample index.
    pos: i64,
    /// Expected distance to the next mark, until that one is placed.
    period: f32,
    voiced: bool,
}

/// One synthesis stream: grains overlap-added on its own marks.
#[derive(Debug, Clone, Default)]
struct Voice {
    /// Output accumulators, indexed by output-timeline sample.
    acc: [Vec<f32>; CHANNELS],
    /// Next synthesis mark, on the input timeline.
    next_s: f64,
    /// Synthesis position of the previous grain (for `PitchControl`).
    last_s: f64,
    /// How far ahead of the output the next grain must be placed.
    lead: f64,
    /// 0 (unvoiced, unshifted) to 1 (voiced, fully shifted).
    voicing: f64,
    /// Delay of the grains behind the synthesis marks, in samples.
    offset: f64,
    /// Layers and freeze only run while they are heard.
    active: bool,
}

impl Voice {
    fn alloc(&mut self, len: usize) {
        self.acc = [vec![0.0; len], vec![0.0; len]];
    }

    fn clear(&mut self, lead: f64) {
        for b in &mut self.acc {
            b.fill(0.0);
        }
        self.next_s = 0.0;
        self.last_s = 0.0;
        self.voicing = 0.0;
        self.offset = 0.0;
        self.lead = lead;
    }

    /// Read and clear output sample `o`.
    #[inline]
    fn take(&mut self, o: usize) -> (f32, f32) {
        let out = (self.acc[0][o], self.acc[1][o]);
        self.acc[0][o] = 0.0;
        self.acc[1][o] = 0.0;
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FreezeState {
    #[default]
    Off,
    /// Asked for, waiting for a voiced grain to hold.
    Waiting,
    On,
}

/// The held grain of a freeze and its own synthesis stream.
#[derive(Debug, Clone, Default)]
struct Frozen {
    state: FreezeState,
    /// The grain (two periods plus the interpolator's reach).
    buf: [Vec<f32>; CHANNELS],
    centre: f64,
    period: f64,
    /// Output ratio and transposition when it was taken.
    ratio: f64,
    transpose: f64,
    /// Crossfade from the live voice (0) to the frozen one (1).
    fade: f32,
    voice: Voice,
}

/// Real-time pitch-synchronous shifter. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Psola {
    sample_rate: f64,
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
    out_mask: usize,
    main: Voice,
    layers: [Voice; LAYERS],
    layer_level: [f32; LAYERS],
    /// Last pitch ratio of the main voice (the freeze starts from it).
    main_ratio: f64,
    /// Voiced main grains placed (the layers take every n-th).
    grain_n: u64,
    /// How much the formants follow the pitch (0 = kept in place, 1 =
    /// like resampling: chipmunk up, monster down).
    follow: f64,
    frozen: Frozen,
    freeze_step: f32,
    /// Freeze pitch: a played note (Hz), or the transposition now.
    freeze_hz: Option<f32>,
    freeze_transpose: f64,
    frozen_on: bool,
    glide: f64,
    /// Last detected period and how many samples it is still held for.
    held_period: f32,
    hold: usize,
    hold_len: usize,
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

    /// Allocates for `sample_rate` and an output delay of `latency`
    /// samples. Not for the audio thread.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn prepare(&mut self, sample_rate: f64, latency: usize) {
        let sr = sample_rate.max(8_000.0);
        self.sample_rate = sr;
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
        self.main.alloc(out_len);
        for v in &mut self.layers {
            v.alloc(out_len);
        }
        self.frozen.voice.alloc(out_len);
        let grain = 2 * pmax + 8;
        self.frozen.buf = [vec![0.0; grain], vec![0.0; grain]];
        self.freeze_step = (1.0 / (sr * FREEZE_FADE_S)) as f32;
        self.marks = vec![Mark::default(); MARKS];
        self.out_mask = out_len - 1;
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
        self.search.fill(0.0);
        for b in &mut self.lowpass {
            b.reset();
        }
        self.n = 0;
        self.count = 0;
        self.hold = 0;
        self.main_ratio = 1.0;
        self.sung_period = 0.0;
        self.prev = [0.0; 2];
        self.e_full = 0.0;
        self.e_high = 0.0;
        #[allow(clippy::cast_precision_loss)]
        let u = self.unvoiced as f64;
        self.main.clear(u);
        self.main.active = true;
        for v in &mut self.layers {
            v.clear(u);
            v.active = false;
        }
        self.frozen.voice.clear(u);
        self.frozen.voice.active = false;
        self.frozen.state = FreezeState::Off;
        self.frozen.fade = 0.0;
    }

    /// Levels of the layers (see [`LAYER_RATIOS`]), 0 to 1, already
    /// smoothed by the caller. A layer starts on the main voice's marks
    /// when its level leaves 0. Real-time safe.
    pub fn set_layers(&mut self, levels: [f32; LAYERS]) {
        for (i, &l) in levels.iter().enumerate() {
            let l = l.clamp(0.0, 1.0);
            let v = &mut self.layers[i];
            if l > 0.0 && !v.active {
                for b in &mut v.acc {
                    b.fill(0.0);
                }
                v.active = true;
            } else if l <= 0.0 {
                v.active = false;
            }
            self.layer_level[i] = l;
        }
    }

    /// How much the formants follow the pitch shift, 0 to 1.
    pub fn set_formant_follow(&mut self, follow: f32) {
        self.follow = f64::from(follow.clamp(0.0, 1.0));
    }

    /// Freeze: hold the latest voiced grain and play it on and on (it
    /// crossfades in once a voiced grain is there, and back out when
    /// released). `note_hz` plays it at that pitch (a MIDI note);
    /// otherwise it keeps the pitch it had, moved by any change of
    /// `transpose_st` since. Real-time safe.
    pub fn set_freeze(&mut self, on: bool, note_hz: Option<f32>, transpose_st: f32) {
        self.freeze_hz = note_hz;
        self.freeze_transpose = f64::from(transpose_st);
        match (on, self.frozen.state) {
            (true, FreezeState::Off) => self.frozen.state = FreezeState::Waiting,
            (false, FreezeState::Waiting) => self.frozen.state = FreezeState::Off,
            _ => {}
        }
        if !on && self.frozen.state == FreezeState::On && self.frozen.fade <= 0.0 {
            self.frozen.state = FreezeState::Off;
            self.frozen.voice.active = false;
        }
        self.frozen_on = on;
    }

    /// Whether a frozen grain is sounding.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.frozen.state == FreezeState::On
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
        let fr = f64::from(fr);
        let mut v = std::mem::take(&mut self.main);
        while v.next_s <= c + v.lead {
            if !self.place_grain(&mut v, c, pitch, fr) {
                break;
            }
        }
        self.main = v;
        self.run_freeze(c, fr);
        #[allow(clippy::cast_possible_truncation)]
        let o = self.out_index(n - self.latency);
        let (mut l, mut r) = self.main.take(o);
        let mut power = 1.0;
        for i in 0..LAYERS {
            if self.layers[i].active {
                let (a, b) = self.layers[i].take(o);
                let g = self.layer_level[i];
                l += g * a;
                r += g * b;
                power += g * g;
            }
        }
        if power > 1.0 {
            // Layers stack up without pushing the level (or clipping):
            // the sum keeps about the main voice's loudness.
            let k = power.sqrt().recip();
            l *= k;
            r *= k;
        }
        if self.frozen.voice.active {
            let (a, b) = self.frozen.voice.take(o);
            let f = self.frozen.fade;
            l += f * (a - l);
            r += f * (b - r);
        }
        self.n += 1;
        (l, r)
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
    /// mark on by one output period. Returns `false` when the grain must
    /// wait for more input.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn place_grain(
        &mut self,
        v: &mut Voice,
        c: f64,
        pitch: &mut impl PitchControl,
        formant_ratio: f64,
    ) -> bool {
        let s = v.next_s;
        // The grains play the input `offset` samples behind the synthesis
        // marks. A voiced grain needs two periods of input, an unvoiced
        // one much less: following that difference at every consonant
        // would skip or repeat a few milliseconds each time (a doubled
        // consonant, a jump). So the delay stays at what the sung period
        // needs, through consonants and pauses, and only comes back
        // slowly. When a grain needs more (first note, a lower note), the
        // output waits for it: a few milliseconds of gap instead of a
        // repeat.
        #[allow(clippy::cast_precision_loss)]
        let floor = (self.sung_period * (2.0 + SEARCH + 1.0 / formant_ratio) + 2.0
            - self.latency as f64)
            .max(0.0);
        v.offset = (v.offset - OFFSET_RECOVERY * v.lead).max(floor);
        let target = s - v.offset;
        let Some(k) = self.nearest_complete(target) else {
            // Nothing complete yet (just after a reset): silence.
            v.next_s += self.unvoiced as f64;
            return true;
        };
        if k + 1 < self.count && target - (self.mark(k).pos as f64) > 0.5 * self.right(k) {
            // A nearer grain is still being recorded.
            return false;
        }
        let m = self.mark(k);
        let right = self.right(k);
        let left = if k + MARKS > self.count && k > 0 {
            self.interval(k)
        } else {
            right
        };
        let step = right / self.glide;
        v.voicing = if m.voiced {
            (v.voicing + step).min(1.0)
        } else {
            (v.voicing - step).max(0.0)
        };
        // The formant applies from the first sample, voiced or not: the
        // timbre does not switch when the detector makes up its mind.
        // Only the pitch glides in, and unvoiced grains are never
        // transposed (noise put back at another spacing would buzz).
        let fr = formant_ratio;
        let dt = ((s - v.last_s) / self.sample_rate).max(0.0);
        v.last_s = s;
        let (ratio, fr, hl, hr) = if m.voiced {
            let max = f64::from(MAX_PITCH_RATIO);
            // The grain's own period, unless a mark landed off the
            // pulses (consonant onsets): then the detector's.
            let own = if (right / f64::from(m.period)).ln().abs() < 0.2 {
                right
            } else {
                f64::from(m.period)
            };
            let pr = pitch.voiced(own, dt).clamp(1.0 / max, max);
            self.main_ratio = pr;
            let ratio = pr.powf(v.voicing);
            let fr = self.followed(fr, ratio);
            (ratio, fr, left / fr, right / fr)
        } else {
            pitch.unvoiced(dt);
            // Kept at the analysis spacing, so the grains still sum to
            // one; each is read faster or slower, which moves its
            // envelope, without reading past the recorded input.
            #[allow(clippy::cast_precision_loss)]
            let room = ((self.n - 2 - m.pos) as f64 / fr).max(1.0);
            (1.0, fr, left, right.min(room))
        };
        // A grain due late (after waiting, or wider than the previous one)
        // is moved, never cut on its rising edge; the delay follows so the
        // input plays on from where it was.
        // (Sample `c` itself is still writable: it is read after this.
        // Counting it as late would delay every grain by half a sample
        // on average and leave the output a few cents flat.)
        let late = (c - 1.0 + hl - s).max(0.0);
        let s = s + late;
        v.offset += late;
        let a = m.pos as f64;
        // More grains per second going up, fewer going down: keep the
        // loudness where it was (same for narrower or wider grains).
        let gain = if m.voiced { (fr / ratio).sqrt() } else { 1.0 };
        self.add_grain(&mut v.acc, s, hl, hr, a, fr, gain);
        let spacing = right / ratio;
        if m.voiced {
            self.layer_grains(s, a, left, right, formant_ratio, ratio, spacing, v.voicing);
        }
        v.next_s = s + spacing;
        v.lead = hl.max(hr);
        true
    }

    /// Formant ratio once it follows the pitch ratio (chipmunk going up,
    /// monster going down), by the follow amount.
    fn followed(&self, fr: f64, ratio: f64) -> f64 {
        if self.follow <= 0.0 {
            return fr;
        }
        (fr * ratio.powf(self.follow)).clamp(
            1.0 / f64::from(MAX_FORMANT_RATIO),
            f64::from(MAX_FORMANT_RATIO),
        )
    }

    /// The layers' grains for a voiced main grain at `s`. They sit on
    /// the main voice's own synthesis marks (every second one an octave
    /// down, every fourth two octaves down, twice as many an octave up),
    /// with the same analysis grain: their pulses line up with the main
    /// voice's, so where their harmonics meet they add up instead of
    /// cancelling, and they follow its pitch and correction.
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn layer_grains(
        &mut self,
        s: f64,
        a: f64,
        left: f64,
        right: f64,
        formant_ratio: f64,
        ratio: f64,
        spacing: f64,
        voicing: f64,
    ) {
        let n = self.grain_n;
        self.grain_n = self.grain_n.wrapping_add(1);
        if !self.layers.iter().any(|l| l.active) || voicing <= 0.0 {
            return;
        }
        let max = f64::from(MAX_PITCH_RATIO);
        let mut layers = std::mem::take(&mut self.layers);
        for (l, &mult) in layers.iter_mut().zip(&LAYER_RATIOS) {
            if !l.active {
                continue;
            }
            let every = (1.0 / mult).round().max(1.0) as u64;
            if !n.is_multiple_of(every) {
                continue;
            }
            let lr = (ratio * mult).clamp(1.0 / max, max);
            let fr = self.followed(formant_ratio, lr);
            let gain = (fr / lr).sqrt() * voicing;
            let (hl, hr) = (left / fr, right / fr);
            self.add_grain(&mut l.acc, s, hl, hr, a, fr, gain);
            if mult > 1.0 {
                self.add_grain(&mut l.acc, s + 0.5 * spacing, hl, hr, a, fr, gain);
            }
        }
        self.layers = layers;
    }

    /// Overlap-add one grain of the input around analysis position `a`
    /// at output position `s` (asymmetric Hann, read `fr` times faster).
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss
    )]
    fn add_grain(
        &self,
        acc: &mut [Vec<f32>; CHANNELS],
        s: f64,
        hl: f64,
        hr: f64,
        a: f64,
        fr: f64,
        gain: f64,
    ) {
        let start = (s - hl).ceil() as i64;
        let end = (s + hr).floor() as i64;
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
            acc[0][o] += w * self.read(0, src);
            acc[1][o] += w * self.read(1, src);
        }
    }

    /// Take the newest voiced grain still in the input as the frozen
    /// one. `false` while there is none (unvoiced, silence).
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn capture(&mut self) -> bool {
        let oldest = self.count.saturating_sub(FREEZE_LOOKBACK);
        let Some(k) = (oldest..self.count)
            .rev()
            .find(|&k| self.mark(k).voiced && self.complete(k))
        else {
            return false;
        };
        let p = self.right(k).min((self.frozen.buf[0].len() / 2 - 4) as f64);
        let start = self.mark(k).pos - p.ceil() as i64 - 2;
        if start < self.n - self.in_mask as i64 {
            return false;
        }
        for ch in 0..CHANNELS {
            for (j, x) in (start..).zip(self.frozen.buf[ch].iter_mut()) {
                *x = self.input[ch][(j as usize) & self.in_mask];
            }
        }
        let pos = self.mark(k).pos;
        let f = &mut self.frozen;
        f.centre = (pos - start) as f64;
        f.period = p;
        f.ratio = self.main_ratio;
        f.transpose = self.freeze_transpose;
        true
    }

    /// The frozen voice: crossfade and grains.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn run_freeze(&mut self, c: f64, formant_ratio: f64) {
        match self.frozen.state {
            FreezeState::Off => return,
            FreezeState::Waiting => {
                if !self.capture() {
                    return;
                }
                self.frozen.state = FreezeState::On;
                let v = &mut self.frozen.voice;
                if !v.active {
                    for b in &mut v.acc {
                        b.fill(0.0);
                    }
                    v.next_s = c;
                    v.lead = 0.0;
                    v.active = true;
                }
            }
            FreezeState::On => {}
        }
        let f = &mut self.frozen;
        if self.frozen_on {
            f.fade = (f.fade + self.freeze_step).min(1.0);
        } else {
            f.fade = (f.fade - self.freeze_step).max(0.0);
            if f.fade <= 0.0 {
                f.state = FreezeState::Off;
                f.voice.active = false;
                return;
            }
        }
        let max = f64::from(MAX_PITCH_RATIO);
        while f.voice.next_s <= c + f.voice.lead {
            let p = f.period;
            let ratio = match self.freeze_hz {
                Some(hz) => f64::from(hz) * p / self.sample_rate,
                None => f.ratio * ((self.freeze_transpose - f.transpose) / 12.0).exp2(),
            }
            .clamp(1.0 / max, max);
            let fr = (formant_ratio * ratio.powf(self.follow)).clamp(
                1.0 / f64::from(MAX_FORMANT_RATIO),
                f64::from(MAX_FORMANT_RATIO),
            );
            let h = p / fr;
            let s = f.voice.next_s.max(c - 1.0 + h);
            let gain = ((fr / ratio).sqrt()) as f32;
            let start = (s - h).ceil() as i64;
            let end = (s + h).floor() as i64;
            for j in start..=end {
                let d = j as f64 - s;
                let w = (0.5 + 0.5 * (PI * d / h).cos()) as f32 * gain;
                let src = f.centre + d * fr;
                let o = (j as usize) & self.out_mask;
                for ch in 0..CHANNELS {
                    f.voice.acc[ch][o] += w * read_buf(&f.buf[ch], src);
                }
            }
            f.voice.next_s = s + p / ratio;
            f.voice.lead = h;
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

    /// Whether mark `k`'s whole grain (plus the interpolator's reach) is
    /// already in the input.
    #[allow(clippy::cast_precision_loss)]
    fn complete(&self, k: usize) -> bool {
        k + 1 < self.count && (self.mark(k).pos as f64 + self.right(k) + 2.0) < self.n as f64
    }

    /// Complete mark nearest to `s`.
    #[allow(clippy::cast_precision_loss)]
    fn nearest_complete(&self, s: f64) -> Option<usize> {
        let oldest = self.count.saturating_sub(MARKS - 1);
        let mut best: Option<(usize, f64)> = None;
        for k in (oldest..self.count).rev() {
            if !self.complete(k) {
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

/// `buf` at fractional index `x` (4-point Hermite), zero outside.
#[inline]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn read_buf(buf: &[f32], x: f64) -> f32 {
    let i = x.floor();
    let t = (x - i) as f32;
    let i = i as i64;
    let at = |k: i64| {
        if k < 0 {
            0.0
        } else {
            buf.get(k as usize).copied().unwrap_or(0.0)
        }
    };
    let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
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

    /// Like [`shift`], with the Creative controls: layer levels, formant
    /// follow, and a freeze switched on at `freeze_at` samples.
    fn creative(
        x: &[f32],
        pitch_st: f32,
        layers: [f32; LAYERS],
        follow: f32,
        freeze_at: Option<usize>,
    ) -> Vec<f32> {
        let mut det = Yin::new(SR);
        let mut ps = Psola::new(SR, LAT);
        ps.set_layers(layers);
        ps.set_formant_follow(follow);
        let pr = semitones_to_ratio(pitch_st);
        x.iter()
            .enumerate()
            .map(|(i, &v)| {
                ps.set_freeze(freeze_at.is_some_and(|f| i >= f), None, 0.0);
                det.push_sample(v);
                #[allow(clippy::cast_possible_truncation)]
                let period = det.estimate().map(|e| (SR / f64::from(e.hz)) as f32);
                ps.tick(v, v, period, pr, 1.0).0
            })
            .collect()
    }

    #[test]
    fn layers_add_octaves_on_the_same_grains() {
        let x = vowel(220.0, 0.6);
        // Main voice silent is not possible, so compare spectra: an
        // octave-down layer puts energy at 110 Hz, absent from the input.
        let band = |y: &[f32], hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in y.iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let ph = 2.0 * PI * hz * i as f64 / SR;
                re += f64::from(v) * ph.cos();
                im += f64::from(v) * ph.sin();
            }
            re.hypot(im)
        };
        let plain = creative(&x, 0.0, [0.0; LAYERS], 0.0, None);
        let down = creative(&x, 0.0, [1.0, 0.0, 0.0], 0.0, None);
        let sub = creative(&x, 0.0, [0.0, 1.0, 0.0], 0.0, None);
        let w = 12_000..28_800;
        let at = |y: &[f32], hz| band(&y[w.clone()], hz);
        assert!(at(&down, 110.0) > 8.0 * at(&plain, 110.0), "octave down");
        assert!(at(&sub, 55.0) > 8.0 * at(&plain, 55.0), "sub");
        // The main voice is still there, at its pitch.
        assert!(
            at(&down, 220.0) > 0.7 * at(&plain, 220.0),
            "fundamental kept"
        );
        // Octave up: the main pitch is still detected (the layer sits on
        // its second harmonic), and transposition carries the layers.
        let up = creative(&x, -12.0, [0.0, 0.0, 1.0], 0.0, None);
        let base = at(&creative(&x, -12.0, [0.0; LAYERS], 0.0, None), 220.0);
        assert!(at(&up, 220.0) > 1.5 * base, "octave up");
    }

    #[test]
    fn freeze_holds_the_grain_after_the_voice_stops() {
        // Voice for 0.3 s, then a different note, then silence: the freeze
        // taken at 0.25 s keeps 220 Hz through both.
        let mut x = vowel(220.0, 0.3);
        x.extend(vowel(330.0, 0.3));
        x.extend(vec![0.0; 14_400]);
        let y = creative(&x, 0.0, [0.0; LAYERS], 0.0, Some(12_000));
        let note2 = median_pitch(&y[18_000..28_000]);
        assert!(
            cents(note2, 220.0).abs() < 15.0,
            "over the next note: {note2}"
        );
        let quiet = median_pitch(&y[30_000..42_000]);
        assert!(cents(quiet, 220.0).abs() < 15.0, "over silence: {quiet}");
        // Released: it fades back to the live input.
        let mut det = Yin::new(SR);
        let mut ps = Psola::new(SR, LAT);
        let mut tail = 0.0f32;
        for (i, &v) in x.iter().enumerate() {
            ps.set_freeze((12_000..20_000).contains(&i), None, 0.0);
            det.push_sample(v);
            #[allow(clippy::cast_possible_truncation)]
            let period = det.estimate().map(|e| (SR / f64::from(e.hz)) as f32);
            let o = ps.tick(v, v, period, 1.0, 1.0).0;
            if i > 30_000 {
                tail = tail.max(o.abs());
            }
        }
        assert!(tail < 1e-3, "silence after release: {tail}");
        assert!(!ps.is_frozen());
    }

    #[test]
    fn freeze_plays_a_midi_note() {
        let x = vowel(220.0, 0.6);
        let mut det = Yin::new(SR);
        let mut ps = Psola::new(SR, LAT);
        let y: Vec<f32> = x
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let hz = if i > 14_000 { Some(392.0) } else { None };
                ps.set_freeze(i >= 9_600, hz, 0.0);
                det.push_sample(v);
                #[allow(clippy::cast_possible_truncation)]
                let period = det.estimate().map(|e| (SR / f64::from(e.hz)) as f32);
                ps.tick(v, v, period, 1.0, 1.0).0
            })
            .collect();
        let got = median_pitch(&y[17_000..]);
        assert!(cents(got, 392.0).abs() < 10.0, "{got}");
    }

    #[test]
    fn formant_follow_makes_chipmunks_and_monsters() {
        let x = vowel(196.0, 0.6);
        let at = |y: &[f32]| centroid(&y[14_000..16_048]);
        let kept = at(&creative(&x, 12.0, [0.0; LAYERS], 0.0, None));
        let chip = at(&creative(&x, 12.0, [0.0; LAYERS], 1.0, None));
        assert!(chip / kept > 1.5, "chipmunk {chip:.0} vs {kept:.0}");
        let kept = at(&creative(&x, -12.0, [0.0; LAYERS], 0.0, None));
        let monster = at(&creative(&x, -12.0, [0.0; LAYERS], 1.0, None));
        assert!(monster / kept < 0.75, "monster {monster:.0} vs {kept:.0}");
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
