//! Key detection: which key the song is in, heard from the voice's sung
//! notes and, when one comes in, the mix (side-chain or Quackapella
//! Link).
//!
//! No machine learning: the notes heard make a pitch-class profile
//! (how long each of the 12 notes sounds), matched against the 24 major
//! and minor key profiles of Krumhansl and Kessler, rotated to each
//! tonic. The best match is the key.
//!
//! - **Learn** listens until it is sure (about 6 s of singing or
//!   playing), then sets Key and Scale once and stops.
//! - **Auto** keeps listening with a memory of about 40 s and moves the
//!   key only when another one wins clearly for a few seconds.
//!
//! The voice counts by its detected pitch (what the autotune already
//! reads). The mix counts by its spectrum: low-passed, decimated to about
//! 11 kHz, a 2048-point FFT every 93 ms, its peaks between 100 Hz and
//! 2 kHz folded into the 12 notes. All of it reads only: nothing on the
//! signal path, no latency.

use crate::autotune::Scale;
use crate::pitch::{Biquad, PitchEstimate};

/// Key detection mode (parameter *Key Detect*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyDetect {
    #[default]
    Off,
    Learn,
    Auto,
}

impl KeyDetect {
    pub const COUNT: u32 = 3;
    pub const ALL: [Self; 3] = [Self::Off, Self::Learn, Self::Auto];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Learn => "Learn",
            Self::Auto => "Auto",
        }
    }
}

/// A key: tonic (0 = C) and mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyGuess {
    pub tonic: u8,
    pub minor: bool,
}

impl KeyGuess {
    /// The scale to set for this key, keeping the family the user chose
    /// where it fits (a pentatonic stays pentatonic, a Dorian or blues
    /// stays as it is in a minor key, ...).
    #[must_use]
    pub fn scale_from(self, current: Scale) -> Scale {
        use Scale::{
            Blues, Chromatic, Dorian, HarmonicMinor, Major, MajorPentatonic, MelodicMinor,
            MinorPentatonic, Mixolydian, NaturalMinor,
        };
        match (self.minor, current) {
            (false, Major | Mixolydian | MajorPentatonic) => current,
            (false, MinorPentatonic) => MajorPentatonic,
            (false, _) => Major,
            (
                true,
                NaturalMinor | HarmonicMinor | MelodicMinor | Dorian | MinorPentatonic | Blues,
            ) => current,
            (true, MajorPentatonic) => MinorPentatonic,
            (true, Chromatic | Major | Mixolydian) => NaturalMinor,
        }
    }
}

/// What the finder reports to the editor.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KeyReading {
    /// Best key so far (None until there is something to go on).
    pub guess: Option<KeyGuess>,
    /// How clearly it wins, 0 to 1.
    pub confidence: f32,
    /// Learn: how far the listening is, 0 to 1 (1 once learned).
    pub progress: f32,
    /// A mix is heard as well as the voice.
    pub mix: bool,
    /// Learn has set the key.
    pub learned: bool,
}

/// Krumhansl-Kessler probe-tone profiles, tonic first.
const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Seconds of singing or playing Learn waits for at least.
pub const LEARN_S: f64 = 6.0;
/// Learn takes its best guess anyway after this much.
const LEARN_MAX_S: f64 = 24.0;
/// Auto's memory (time constant of the profile's decay).
const AUTO_MEMORY_S: f64 = 40.0;
/// Auto's first key after this much.
const AUTO_FIRST_S: f64 = 5.0;
/// Auto: how much better another key must match, and for how long.
const AUTO_MARGIN: f32 = 0.06;
const AUTO_HOLD_S: f64 = 3.0;
/// Evaluation period.
const EVAL_S: f64 = 0.25;

/// Mix analysis: target decimated rate, FFT size, band.
const MIX_RATE: f64 = 11_025.0;
const FFT_N: usize = 2048;
const HOP: usize = FFT_N / 2;
const MIX_LO_HZ: f64 = 100.0;
const MIX_HI_HZ: f64 = 2000.0;
/// Below this RMS (decimated frame) the mix is silence.
const MIX_FLOOR: f32 = 1e-4;
/// Weight of the mix against the voice when both are heard.
const MIX_WEIGHT: f32 = 0.65;

#[derive(Debug, Clone, Default)]
pub struct KeyFinder {
    sample_rate: f64,
    mode: KeyDetect,
    /// Note durations (weighted seconds) from the voice and the mix, and
    /// the evidence (seconds) behind each.
    voice: [f64; 12],
    voice_s: f64,
    mix: [f64; 12],
    mix_s: f64,
    /// Samples to the next evaluation.
    eval_in: usize,
    eval_len: usize,
    /// The key in use (set by this finder) and a challenger with its
    /// time ahead.
    current: Option<KeyGuess>,
    challenger: Option<(KeyGuess, f64)>,
    /// A key to write to Key and Scale.
    pending: Option<KeyGuess>,
    reading: KeyReading,
    /// Mix analysis.
    lowpass: [Biquad; 2],
    decim: usize,
    decim_at: usize,
    ring: Vec<f32>,
    ring_pos: usize,
    hop_in: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
    /// Per FFT bin in the band: lower note class, upper class, share of
    /// the upper one (bins out of the band: class 255).
    bins: Vec<(u8, u8, f32)>,
    /// The mix was fed this block (by Link or the side-chain).
    mix_fed: bool,
}

impl KeyFinder {
    /// Allocate for `sample_rate` (not real-time).
    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            self.decim = ((sample_rate / MIX_RATE).round() as usize).clamp(1, 16);
            self.eval_len = (EVAL_S * sample_rate) as usize;
        }
        #[allow(clippy::cast_precision_loss)]
        let rate = sample_rate / self.decim as f64;
        let cutoff = (0.36 * rate).min(4000.0);
        self.lowpass = [Biquad::lowpass(sample_rate, cutoff); 2];
        self.ring = vec![0.0; FFT_N];
        self.re = vec![0.0; FFT_N];
        self.im = vec![0.0; FFT_N];
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        {
            self.window = (0..FFT_N)
                .map(|i| {
                    let x = std::f64::consts::TAU * i as f64 / FFT_N as f64;
                    (0.5 - 0.5 * x.cos()) as f32
                })
                .collect();
            self.bins = (0..FFT_N / 2)
                .map(|k| {
                    let hz = k as f64 * rate / FFT_N as f64;
                    if !(MIX_LO_HZ..=MIX_HI_HZ).contains(&hz) {
                        return (255, 255, 0.0);
                    }
                    let note = 69.0 + 12.0 * (hz / 440.0).log2();
                    let lo = note.floor();
                    let frac = (note - lo) as f32;
                    let pc = lo.rem_euclid(12.0) as u8;
                    (pc, (pc + 1) % 12, frac)
                })
                .collect();
        }
        self.reset();
    }

    /// Forget everything heard (a new Learn, Auto from scratch).
    pub fn reset(&mut self) {
        self.voice = [0.0; 12];
        self.mix = [0.0; 12];
        self.voice_s = 0.0;
        self.mix_s = 0.0;
        self.eval_in = self.eval_len;
        self.current = None;
        self.challenger = None;
        self.pending = None;
        self.reading = KeyReading::default();
        for b in &mut self.lowpass {
            b.reset();
        }
        self.ring.iter_mut().for_each(|x| *x = 0.0);
        self.ring_pos = 0;
        self.hop_in = HOP;
        self.decim_at = 0;
    }

    /// Mode from the parameter. Turning Learn or Auto on starts afresh.
    pub fn set_mode(&mut self, mode: KeyDetect) {
        if mode != self.mode {
            self.mode = mode;
            self.reset();
        }
    }

    #[must_use]
    pub fn mode(&self) -> KeyDetect {
        self.mode
    }

    fn listening(&self) -> bool {
        match self.mode {
            KeyDetect::Off => false,
            KeyDetect::Learn => !self.reading.learned,
            KeyDetect::Auto => true,
        }
    }

    /// One sample of the voice's pitch reading.
    #[inline]
    pub fn voice(&mut self, est: Option<PitchEstimate>) {
        if !self.listening() {
            return;
        }
        if let Some(e) = est
            && e.confidence > 0.6
            && (60.0..1500.0).contains(&e.hz)
        {
            let note = f64::from(e.note());
            let lo = note.floor();
            let frac = note - lo;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let pc = lo.rem_euclid(12.0) as usize;
            let w = f64::from(e.confidence) / self.sample_rate;
            self.voice[pc] += w * (1.0 - frac);
            self.voice[(pc + 1) % 12] += w * frac;
            self.voice_s += 1.0 / self.sample_rate;
        }
        self.eval_in = self.eval_in.saturating_sub(1);
        if self.eval_in == 0 {
            self.eval_in = self.eval_len.max(1);
            self.evaluate();
        }
    }

    /// One sample of the mix (mono).
    #[inline]
    pub fn mix(&mut self, x: f32) {
        if !self.listening() || self.ring.is_empty() {
            return;
        }
        self.mix_fed = true;
        let y = self.lowpass[0].tick(x);
        let y = self.lowpass[1].tick(y);
        self.decim_at += 1;
        if self.decim_at < self.decim {
            return;
        }
        self.decim_at = 0;
        self.ring[self.ring_pos] = y;
        self.ring_pos = (self.ring_pos + 1) % FFT_N;
        self.hop_in -= 1;
        if self.hop_in == 0 {
            self.hop_in = HOP;
            self.mix_frame();
        }
    }

    /// Whether the mix was fed since the last call (and clear it).
    pub fn take_mix_fed(&mut self) -> bool {
        std::mem::take(&mut self.mix_fed)
    }

    /// One FFT frame of the mix into its profile.
    fn mix_frame(&mut self) {
        let mut energy = 0.0_f32;
        for i in 0..FFT_N {
            let x = self.ring[(self.ring_pos + i) % FFT_N];
            energy += x * x;
            self.re[i] = x * self.window[i];
            self.im[i] = 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        if (energy / FFT_N as f32).sqrt() < MIX_FLOOR {
            return;
        }
        crate::assist::fft(&mut self.re, &mut self.im);
        let mag = |k: usize, re: &[f32], im: &[f32]| (re[k] * re[k] + im[k] * im[k]).sqrt();
        let mut chroma = [0.0_f32; 12];
        let mut total = 0.0_f32;
        for k in 1..FFT_N / 2 - 1 {
            let (lo, hi, frac) = self.bins[k];
            if lo == 255 {
                continue;
            }
            let m = mag(k, &self.re, &self.im);
            // Peaks only: the partials, not the leakage around them.
            if m <= mag(k - 1, &self.re, &self.im) || m < mag(k + 1, &self.re, &self.im) {
                continue;
            }
            let w = m.sqrt();
            chroma[lo as usize] += w * (1.0 - frac);
            chroma[hi as usize] += w * frac;
            total += w;
        }
        if total <= 0.0 {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let hop_s = (HOP * self.decim) as f64 / self.sample_rate;
        for (m, c) in self.mix.iter_mut().zip(chroma) {
            *m += f64::from(c / total) * hop_s;
        }
        self.mix_s += hop_s;
    }

    /// The combined, normalised profile.
    fn profile(&self) -> Option<[f32; 12]> {
        let norm = |h: &[f64; 12]| {
            let s: f64 = h.iter().sum();
            #[allow(clippy::cast_possible_truncation)]
            (s > 0.0).then(|| h.map(|x| (x / s) as f32))
        };
        let v = norm(&self.voice).filter(|_| self.voice_s > 0.5);
        let m = norm(&self.mix).filter(|_| self.mix_s > 1.0);
        match (v, m) {
            (Some(v), Some(m)) => Some(std::array::from_fn(|i| {
                v[i] * (1.0 - MIX_WEIGHT) + m[i] * MIX_WEIGHT
            })),
            (a, b) => a.or(b),
        }
    }

    fn evaluate(&mut self) {
        if self.mode == KeyDetect::Auto {
            let k = (-EVAL_S / AUTO_MEMORY_S).exp();
            self.voice
                .iter_mut()
                .chain(self.mix.iter_mut())
                .for_each(|x| *x *= k);
            self.voice_s *= k;
            self.mix_s *= k;
        }
        let evidence = self.voice_s + self.mix_s;
        self.reading.mix = self.mix_s > 1.0;
        let Some(profile) = self.profile() else {
            self.reading.progress = 0.0;
            return;
        };
        let scores = scores(&profile);
        let (best, best_r, second_r) = rank(&scores);
        let confidence =
            ((best_r - second_r) / 0.1).clamp(0.0, 1.0) * ((best_r - 0.3) / 0.3).clamp(0.0, 1.0);
        self.reading.guess = Some(best);
        self.reading.confidence = confidence;
        match self.mode {
            KeyDetect::Off => {}
            KeyDetect::Learn => {
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.reading.progress = (evidence / LEARN_S).min(0.99) as f32;
                }
                let sure = evidence >= LEARN_S && confidence >= 0.5;
                if sure || (evidence >= LEARN_MAX_S && best_r > 0.3) {
                    self.reading.learned = true;
                    self.reading.progress = 1.0;
                    self.current = Some(best);
                    self.pending = Some(best);
                }
            }
            KeyDetect::Auto => {
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.reading.progress = (evidence / AUTO_FIRST_S).min(1.0) as f32;
                }
                match self.current {
                    None => {
                        if evidence >= AUTO_FIRST_S && confidence >= 0.4 {
                            self.current = Some(best);
                            self.pending = Some(best);
                        }
                    }
                    Some(cur) if cur == best => self.challenger = None,
                    Some(cur) => {
                        let cur_r = scores[index(cur)];
                        if best_r - cur_r < AUTO_MARGIN {
                            self.challenger = None;
                        } else {
                            let held = match self.challenger {
                                Some((g, t)) if g == best => t + EVAL_S,
                                _ => EVAL_S,
                            };
                            if held >= AUTO_HOLD_S {
                                self.current = Some(best);
                                self.pending = Some(best);
                                self.challenger = None;
                            } else {
                                self.challenger = Some((best, held));
                            }
                        }
                    }
                }
            }
        }
    }

    /// A key to write to the Key and Scale parameters, once.
    pub fn take_pending(&mut self) -> Option<KeyGuess> {
        self.pending.take()
    }

    #[must_use]
    pub fn reading(&self) -> KeyReading {
        if self.mode == KeyDetect::Off {
            return KeyReading::default();
        }
        let mut r = self.reading;
        if let Some(cur) = self.current {
            r.guess = Some(cur);
        }
        r
    }
}

fn index(g: KeyGuess) -> usize {
    usize::from(g.tonic) + if g.minor { 12 } else { 0 }
}

/// Correlation of the profile with each key (0-11 major, 12-23 minor).
fn scores(profile: &[f32; 12]) -> [f32; 24] {
    std::array::from_fn(|k| {
        let tonic = k % 12;
        let model = if k < 12 { &MAJOR } else { &MINOR };
        let rotated: [f32; 12] = std::array::from_fn(|pc| model[(pc + 12 - tonic) % 12]);
        pearson(profile, &rotated)
    })
}

/// Best key, its score and the runner-up's.
fn rank(scores: &[f32; 24]) -> (KeyGuess, f32, f32) {
    let mut best = 0;
    for (k, s) in scores.iter().enumerate() {
        if *s > scores[best] {
            best = k;
        }
    }
    let second = scores
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != best)
        .fold(f32::MIN, |m, (_, s)| m.max(*s));
    #[allow(clippy::cast_possible_truncation)]
    let guess = KeyGuess {
        tonic: (best % 12) as u8,
        minor: best >= 12,
    };
    (guess, scores[best], second)
}

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, y - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn sing(f: &mut KeyFinder, notes: &[f32], each_s: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (each_s * SR) as usize;
        for &note in notes {
            let hz = 440.0 * ((note - 69.0) / 12.0).exp2();
            for _ in 0..n {
                f.voice(Some(PitchEstimate {
                    hz,
                    confidence: 0.9,
                }));
            }
        }
    }

    #[test]
    fn learns_a_major_from_a_melody() {
        let mut f = KeyFinder::default();
        f.prepare(SR);
        f.set_mode(KeyDetect::Learn);
        // D major: D E F# G A B C# D, tonic and fifth longer.
        let melody = [
            62.0, 66.0, 69.0, 67.0, 66.0, 64.0, 62.0, 69.0, 71.0, 73.0, 74.0, 69.0,
        ];
        for _ in 0..3 {
            sing(&mut f, &melody, 0.3);
            sing(&mut f, &[62.0, 69.0], 0.5);
        }
        let key = f.take_pending().expect("learned");
        assert_eq!(
            key,
            KeyGuess {
                tonic: 2,
                minor: false
            }
        );
        assert!(f.reading().learned);
    }

    #[test]
    fn learns_a_minor_key_from_a_melody() {
        let mut f = KeyFinder::default();
        f.prepare(SR);
        f.set_mode(KeyDetect::Learn);
        // A minor, leaning on A, C and E.
        let melody = [
            69.0, 72.0, 76.0, 74.0, 72.0, 71.0, 69.0, 64.0, 65.0, 64.0, 69.0, 72.0,
        ];
        for _ in 0..3 {
            sing(&mut f, &melody, 0.3);
            sing(&mut f, &[69.0, 64.0], 0.5);
        }
        assert_eq!(
            f.take_pending(),
            Some(KeyGuess {
                tonic: 9,
                minor: true
            })
        );
    }

    #[test]
    fn hears_the_key_of_a_mix() {
        let mut f = KeyFinder::default();
        f.prepare(SR);
        f.set_mode(KeyDetect::Learn);
        // Chords of F major (F, Bb, C, F), two seconds each, harmonics.
        let chords: [[f32; 3]; 4] = [
            [53.0, 57.0, 60.0],
            [58.0, 62.0, 65.0],
            [60.0, 64.0, 67.0],
            [53.0, 57.0, 60.0],
        ];
        let mut t = 0usize;
        for _ in 0..2 {
            for c in chords {
                for _ in 0..(2.0 * SR) as usize {
                    #[allow(clippy::cast_precision_loss)]
                    let time = t as f64 / SR;
                    let mut x = 0.0;
                    for n in c {
                        let hz = 440.0 * ((f64::from(n) - 69.0) / 12.0).exp2();
                        for h in 1..4 {
                            x += (std::f64::consts::TAU * hz * f64::from(h) * time).sin()
                                / f64::from(h);
                        }
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    f.mix(0.05 * x as f32);
                    f.voice(None);
                    t += 1;
                }
            }
        }
        assert_eq!(
            f.take_pending(),
            Some(KeyGuess {
                tonic: 5,
                minor: false
            })
        );
    }

    #[test]
    fn auto_moves_only_on_a_clear_change() {
        let mut f = KeyFinder::default();
        f.prepare(SR);
        f.set_mode(KeyDetect::Auto);
        let c_major = [
            60.0, 64.0, 67.0, 65.0, 64.0, 62.0, 60.0, 67.0, 69.0, 71.0, 72.0, 67.0,
        ];
        for _ in 0..3 {
            sing(&mut f, &c_major, 0.3);
        }
        assert_eq!(
            f.take_pending(),
            Some(KeyGuess {
                tonic: 0,
                minor: false
            })
        );
        // A short excursion does not move it.
        sing(&mut f, &[61.0, 66.0, 68.0], 0.4);
        assert_eq!(f.take_pending(), None);
        // A long stretch in E major does.
        let e_major = [
            64.0, 68.0, 71.0, 69.0, 68.0, 66.0, 64.0, 71.0, 73.0, 75.0, 76.0, 71.0,
        ];
        for _ in 0..12 {
            sing(&mut f, &e_major, 0.3);
        }
        assert_eq!(
            f.reading().guess,
            Some(KeyGuess {
                tonic: 4,
                minor: false
            })
        );
    }

    #[test]
    fn scale_family_is_kept() {
        let minor = KeyGuess {
            tonic: 9,
            minor: true,
        };
        let major = KeyGuess {
            tonic: 0,
            minor: false,
        };
        assert_eq!(minor.scale_from(Scale::Chromatic), Scale::NaturalMinor);
        assert_eq!(minor.scale_from(Scale::Dorian), Scale::Dorian);
        assert_eq!(
            minor.scale_from(Scale::MajorPentatonic),
            Scale::MinorPentatonic
        );
        assert_eq!(major.scale_from(Scale::Blues), Scale::Major);
        assert_eq!(
            major.scale_from(Scale::MajorPentatonic),
            Scale::MajorPentatonic
        );
    }
}
