//! The mixing assistant: it listens to a few seconds of singing, says
//! what it heard and proposes settings for the voice effects.
//!
//! Nothing here runs on the audio thread. A [`Listener`] is fed, by the
//! editor or an offline tool, with the samples of the two analysis taps
//! ([`crate::tap`]) and the detected pitch:
//!
//! - the **input** tap (the voice as it comes in) is where the source is
//!   cleaned: range of the voice for the low cut, noise floor for the
//!   gate, sibilants for the de-esser, dynamics for the compressor;
//! - the **layers** tap (after the formants, harmonies and vocoder,
//!   before the output EQ) is where the colour is judged: its balance
//!   against the style's target sets the output EQ.
//!
//! [`propose`] is a pure function from what was measured, a [`Style`]
//! and [`Amounts`] to parameter values; [`describe`] says the same in
//! words. Part of what is measured is physical (range, sibilants,
//! dynamics, noise); the targets of a style are taste, kept in one
//! table ([`Style::rules`]) so they can be tuned by ear.

use crate::fx::{Division, LOW_CUT_OFF_HZ};
use crate::params::{DELAY_SEND_PARAMS, EQ_BELL_PARAMS, ParamId, REVERB_SEND_PARAMS};

/// Third-octave bands of the analysis, from [`BAND_BASE_HZ`] up.
pub const BANDS: usize = 27;
const BAND_BASE_HZ: f32 = 50.0;
/// Samples per analysed frame, and the step between frames.
const FRAME: usize = 2048;
const HOP: usize = 1024;
/// Voiced singing needed for a diagnosis, and what a full listen takes.
pub const MIN_VOICED_S: f32 = 2.0;
pub const FULL_VOICED_S: f32 = 6.0;

/// Centre of band `i`, Hz.
#[must_use]
pub fn band_hz(i: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let i = i as f32;
    BAND_BASE_HZ * (i / 3.0).exp2()
}

/// In-place radix-2 FFT (`re.len()` a power of two).
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        #[allow(clippy::cast_precision_loss)]
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (ws, wc) = ang.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0_f32, 0.0_f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wc - ci * ws;
                ci = cr * ws + ci * wc;
                cr = next;
            }
        }
        len <<= 1;
    }
}

/// What a style asks of a voice. Balances are levels of a zone against
/// the mids (500 Hz to 1.6 kHz), in dB per third-octave band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleRules {
    /// Low mids (200 to 400 Hz), presence (2.5 to 5 kHz) and air (8 to
    /// 14 kHz) against the mids.
    pub mud: f32,
    pub presence: f32,
    pub air: f32,
    /// Most the EQ may move a band, dB.
    pub reach: f32,
    /// Compressor: gain reduction on the loud phrases (dB), ratio,
    /// attack and release (ms).
    pub reduction: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// De-esser: more (positive) or less than what the sibilants call
    /// for, in dB of sibilance.
    pub deess_bias: f32,
    /// Low cut of the output EQ, Hz (a telephone band for lo-fi).
    pub low_cut_hz: f32,
    /// Exciter, % (left off when the voice is already bright).
    pub exciter: f32,
    /// Reverb: return level (%), decay (s) and pre-delay (ms). The
    /// layers' sends are dosed by the Space amount.
    pub reverb: (f32, f32, f32),
    /// Delay, on the host's tempo: return level (%), note value and
    /// feedback (%).
    pub delay: (f32, Division, f32),
}

/// The styles the assistant knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Natural,
    Pop,
    Rnb,
    Trap,
    Rock,
    Electro,
    Ballad,
    LoFi,
}

impl Style {
    pub const ALL: [Self; 8] = [
        Self::Natural,
        Self::Pop,
        Self::Rnb,
        Self::Trap,
        Self::Rock,
        Self::Electro,
        Self::Ballad,
        Self::LoFi,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Natural => "Natural",
            Self::Pop => "Pop",
            Self::Rnb => "R&B",
            Self::Trap => "Trap",
            Self::Rock => "Rock",
            Self::Electro => "Electro",
            Self::Ballad => "Ballad",
            Self::LoFi => "Lo-Fi",
        }
    }

    /// The style's targets. Conventions of the trade, not measurements:
    /// starting points, tuned by ear.
    #[must_use]
    pub const fn rules(self) -> StyleRules {
        // A close-miked voice, untreated and well recorded, sits
        // around these figures (the low mids hold the fundamental).
        const BASE: StyleRules = StyleRules {
            mud: 3.5,
            presence: -7.0,
            air: -16.5,
            reach: 4.0,
            reduction: 3.0,
            ratio: 2.0,
            attack_ms: 15.0,
            release_ms: 150.0,
            deess_bias: 0.0,
            low_cut_hz: LOW_CUT_OFF_HZ,
            exciter: 0.0,
            reverb: (30.0, 1.2, 20.0),
            delay: (0.0, Division::Eighth, 20.0),
        };
        match self {
            // Clean the source, keep the voice as it is.
            Self::Natural => BASE,
            // Bright and airy, mud cleaned, firmly held.
            Self::Pop => StyleRules {
                mud: 1.5,
                presence: -4.0,
                air: -10.5,
                reach: 6.0,
                reduction: 7.0,
                ratio: 4.0,
                attack_ms: 8.0,
                release_ms: 100.0,
                deess_bias: 2.0,
                exciter: 25.0,
                reverb: (45.0, 1.6, 40.0),
                delay: (25.0, Division::Eighth, 25.0),
                ..BASE
            },
            // Warm lows, silky top.
            Self::Rnb => StyleRules {
                mud: 3.0,
                presence: -5.5,
                air: -11.0,
                reach: 6.0,
                reduction: 5.0,
                ratio: 3.0,
                attack_ms: 10.0,
                release_ms: 120.0,
                deess_bias: 2.0,
                exciter: 20.0,
                reverb: (50.0, 2.0, 50.0),
                delay: (25.0, Division::Quarter, 30.0),
                ..BASE
            },
            // Very present, pinned in place, sibilants tamed.
            Self::Trap => StyleRules {
                mud: 0.5,
                presence: -3.0,
                air: -9.5,
                reach: 6.0,
                reduction: 9.0,
                ratio: 6.0,
                attack_ms: 3.0,
                release_ms: 80.0,
                deess_bias: 4.0,
                exciter: 30.0,
                reverb: (35.0, 1.2, 30.0),
                delay: (40.0, Division::Quarter, 35.0),
                ..BASE
            },
            // Mids forward, little air, hit hard.
            Self::Rock => StyleRules {
                mud: 1.5,
                presence: -3.5,
                air: -15.5,
                reach: 6.0,
                reduction: 8.0,
                ratio: 4.0,
                attack_ms: 5.0,
                release_ms: 90.0,
                deess_bias: 1.0,
                exciter: 15.0,
                reverb: (30.0, 1.0, 20.0),
                delay: (30.0, Division::Sixteenth, 10.0),
                ..BASE
            },
            // Cuts through a dense, bright mix: lean lows, crisp top.
            Self::Electro => StyleRules {
                mud: -0.5,
                presence: -3.0,
                air: -8.5,
                reach: 6.0,
                reduction: 8.0,
                ratio: 5.0,
                attack_ms: 4.0,
                release_ms: 80.0,
                deess_bias: 3.0,
                exciter: 35.0,
                reverb: (50.0, 2.4, 30.0),
                delay: (40.0, Division::DottedEighth, 40.0),
                ..BASE
            },
            // Close to natural, a little polish, barely compressed.
            Self::Ballad => StyleRules {
                mud: 3.0,
                presence: -6.0,
                air: -13.5,
                reach: 4.0,
                reduction: 4.0,
                ratio: 2.5,
                attack_ms: 15.0,
                release_ms: 180.0,
                deess_bias: 1.0,
                exciter: 10.0,
                reverb: (60.0, 2.8, 60.0),
                delay: (15.0, Division::Quarter, 20.0),
                ..BASE
            },
            // A narrow band: no lows, no air, squashed.
            Self::LoFi => StyleRules {
                mud: 2.5,
                presence: -5.0,
                air: -36.0,
                reach: 12.0,
                reduction: 9.0,
                ratio: 6.0,
                attack_ms: 3.0,
                release_ms: 100.0,
                deess_bias: 0.0,
                low_cut_hz: 280.0,
                exciter: 0.0,
                reverb: (35.0, 0.8, 0.0),
                delay: (20.0, Division::Eighth, 20.0),
            },
        }
    }
}

/// Zones of the spectrum the assistant reasons in, Hz.
const MID: (f32, f32) = (500.0, 1600.0);
const MUD: (f32, f32) = (200.0, 400.0);
/// Where the low mids are corrected: a wide bell, so that it does not
/// single out the note sung.
const MUD_HZ: f32 = 300.0;
const PRESENCE: (f32, f32) = (2500.0, 5000.0);
const AIR: (f32, f32) = (8000.0, 14_000.0);
const SIBILANT: (f32, f32) = (4000.0, 12_000.0);

/// What a listen measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Analysis {
    /// Range sung: 5th percentile, median and 95th of the pitch, Hz.
    pub low_hz: f32,
    pub median_hz: f32,
    pub high_hz: f32,
    /// Average spectrum of the sung parts, dB per band: the voice as it
    /// comes in, and the sound after the layers.
    pub input: [f32; BANDS],
    pub layers: [f32; BANDS],
    /// Sibilants: their level against the sung parts (dB) and where
    /// they sit (Hz). `None`: none heard.
    pub sibilance: Option<(f32, f32)>,
    /// Level of the loud and of the soft sung parts (95th and 20th
    /// percentile), dBFS RMS.
    pub loud_db: f32,
    pub soft_db: f32,
    /// Level between the phrases, dBFS RMS. `None`: no silence heard.
    pub noise_db: Option<f32>,
    /// Seconds of singing the figures rest on.
    pub voiced_s: f32,
}

/// Average level of the bands inside `zone`, dB.
fn zone_db(bands: &[f32; BANDS], zone: (f32, f32)) -> f32 {
    let (mut sum, mut n) = (0.0_f32, 0.0_f32);
    for (i, db) in bands.iter().enumerate() {
        let hz = band_hz(i);
        if hz >= zone.0 * 0.95 && hz <= zone.1 * 1.05 {
            sum += 10.0_f32.powf(db / 10.0);
            n += 1.0;
        }
    }
    10.0 * (sum / n.max(1.0)).max(1e-20).log10()
}

impl Analysis {
    /// Level of a zone against the mids, on the sound after the layers.
    fn balance(&self, zone: (f32, f32)) -> f32 {
        zone_db(&self.layers, zone) - zone_db(&self.layers, MID)
    }

    /// A band that stands out of its neighbours between 1 and 5 kHz:
    /// its frequency and by how much (dB).
    fn resonance(&self) -> Option<(f32, f32)> {
        let mut best: Option<(f32, f32)> = None;
        for i in 2..BANDS - 2 {
            let hz = band_hz(i);
            if !(1000.0..=5000.0).contains(&hz) {
                continue;
            }
            let around = 0.5 * (self.layers[i - 2] + self.layers[i + 2]);
            let over = self.layers[i] - around;
            if over > 4.0 && best.is_none_or(|b| over > b.1) {
                best = Some((hz, over));
            }
        }
        best
    }

    /// The voice's type, from where it sings most.
    #[must_use]
    pub fn voice_type(&self) -> &'static str {
        match self.median_hz {
            hz if hz < 125.0 => "Bass",
            hz if hz < 175.0 => "Baritone",
            hz if hz < 250.0 => "Tenor",
            hz if hz < 370.0 => "Alto",
            _ => "Soprano",
        }
    }
}

/// Accumulates what a listen hears. Off the audio thread: it allocates
/// when made and is fed in blocks of any size.
#[derive(Debug, Clone)]
pub struct Listener {
    sample_rate: f32,
    input: Vec<f32>,
    layers: Vec<f32>,
    fill: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
    /// Band powers summed over the sung frames (input, layers), and
    /// over the sibilant frames (input).
    sung_in: [f64; BANDS],
    sung_lay: [f64; BANDS],
    sib: [f64; BANDS],
    sung_frames: u32,
    sib_frames: u32,
    /// Frame levels in 1 dB bins (bin n = -n dBFS): sung, sibilant, and
    /// neither.
    sung_levels: [u32; 100],
    sib_level_sum: f64,
    quiet_levels: [u32; 100],
    quiet_frames: u32,
    /// Sung notes in semitone bins (MIDI numbers).
    notes: [u32; 128],
    frames: u32,
}

fn level_bin(db: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let b = (-db).round().clamp(0.0, 99.0) as usize;
    b
}

/// Level (dB, as a positive bin count from the top) under which
/// `share` of the histogram's frames sit.
fn percentile(hist: &[u32], share: f32) -> f32 {
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return -99.0;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let want = (share * total as f32).ceil().max(1.0) as u32;
    // Bins run from loud (0) to soft (99): count from the soft end.
    let mut seen = 0;
    for (bin, n) in hist.iter().enumerate().rev() {
        seen += n;
        if seen >= want {
            #[allow(clippy::cast_precision_loss)]
            return -(bin as f32);
        }
    }
    0.0
}

impl Listener {
    #[must_use]
    pub fn new(sample_rate: f64) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let window = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME - 1) as f32).cos())
            .collect();
        #[allow(clippy::cast_possible_truncation)]
        let sample_rate = (sample_rate as f32).max(8000.0);
        Self {
            sample_rate,
            input: vec![0.0; FRAME],
            layers: vec![0.0; FRAME],
            fill: 0,
            re: vec![0.0; FRAME],
            im: vec![0.0; FRAME],
            window,
            sung_in: [0.0; BANDS],
            sung_lay: [0.0; BANDS],
            sib: [0.0; BANDS],
            sung_frames: 0,
            sib_frames: 0,
            sung_levels: [0; 100],
            sib_level_sum: 0.0,
            quiet_levels: [0; 100],
            quiet_frames: 0,
            notes: [0; 128],
            frames: 0,
        }
    }

    fn frame_s(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let s = HOP as f32 / self.sample_rate;
        s
    }

    /// Seconds heard, and seconds of singing among them.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let s = self.frames as f32 * self.frame_s();
        s
    }

    #[must_use]
    pub fn voiced_seconds(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let s = self.sung_frames as f32 * self.frame_s();
        s
    }

    /// The same stretch of both taps (equal lengths) and the pitch
    /// detected over it (0 while unvoiced).
    pub fn feed(&mut self, input: &[f32], layers: &[f32], pitch_hz: f32) {
        for (a, b) in input.iter().zip(layers) {
            self.input[self.fill] = *a;
            self.layers[self.fill] = *b;
            self.fill += 1;
            if self.fill == FRAME {
                self.frame(pitch_hz);
                self.input.copy_within(HOP.., 0);
                self.layers.copy_within(HOP.., 0);
                self.fill = FRAME - HOP;
            }
        }
    }

    /// Band powers of one windowed frame.
    fn bands(&mut self, layers: bool) -> [f64; BANDS] {
        let src = if layers { &self.layers } else { &self.input };
        for (i, w) in self.window.iter().enumerate() {
            self.re[i] = src[i] * w;
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        let mut out = [0.0_f64; BANDS];
        #[allow(clippy::cast_precision_loss)]
        let bin_hz = self.sample_rate / FRAME as f32;
        for k in 1..FRAME / 2 {
            #[allow(clippy::cast_precision_loss)]
            let hz = k as f32 * bin_hz;
            let b = (3.0 * (hz / BAND_BASE_HZ).log2()).round();
            if b < 0.0 {
                continue;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let b = b as usize;
            if b >= BANDS {
                break;
            }
            out[b] += f64::from(self.re[k].powi(2) + self.im[k].powi(2));
        }
        out
    }

    fn frame(&mut self, pitch_hz: f32) {
        self.frames += 1;
        #[allow(clippy::cast_precision_loss)]
        let mean_sq = self.input.iter().map(|x| x * x).sum::<f32>() / FRAME as f32;
        let level = 10.0 * mean_sq.max(1e-12).log10();
        let bands = self.bands(false);
        let total: f64 = bands.iter().sum::<f64>().max(1e-30);
        let top: f64 = bands
            .iter()
            .enumerate()
            .filter(|(i, _)| band_hz(*i) >= SIBILANT.0)
            .map(|(_, p)| *p)
            .sum();
        if pitch_hz > 0.0 && level > -60.0 {
            let lay = self.bands(true);
            for (sum, p) in self.sung_in.iter_mut().zip(&bands) {
                *sum += p;
            }
            for (sum, p) in self.sung_lay.iter_mut().zip(&lay) {
                *sum += p;
            }
            self.sung_frames += 1;
            self.sung_levels[level_bin(level)] += 1;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let note = (69.0 + 12.0 * (pitch_hz / 440.0).log2())
                .round()
                .clamp(0.0, 127.0) as usize;
            self.notes[note] += 1;
        } else if level > -55.0 && top / total > 0.5 {
            // No pitch and most of the energy at the top: a sibilant.
            for (sum, p) in self.sib.iter_mut().zip(&bands) {
                *sum += p;
            }
            self.sib_frames += 1;
            self.sib_level_sum += f64::from(level);
        } else if pitch_hz <= 0.0 {
            self.quiet_levels[level_bin(level)] += 1;
            self.quiet_frames += 1;
        }
    }

    /// What was measured, once enough singing was heard.
    #[must_use]
    pub fn finish(&self) -> Option<Analysis> {
        if self.voiced_seconds() < MIN_VOICED_S {
            return None;
        }
        let n = f64::from(self.sung_frames);
        let db = |sum: &[f64; BANDS]| {
            let mut out = [-120.0_f32; BANDS];
            for (o, p) in out.iter_mut().zip(sum) {
                #[allow(clippy::cast_possible_truncation)]
                let v = (10.0 * (p / n).max(1e-12).log10()) as f32;
                *o = v;
            }
            out
        };
        let note_at = |share: f32| {
            let total: u32 = self.notes.iter().sum();
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let want = (share * total as f32).ceil().max(1.0) as u32;
            let mut seen = 0;
            for (note, c) in self.notes.iter().enumerate() {
                seen += c;
                if seen >= want {
                    #[allow(clippy::cast_precision_loss)]
                    return 440.0 * ((note as f32 - 69.0) / 12.0).exp2();
                }
            }
            220.0
        };
        let loud_db = percentile(&self.sung_levels, 0.95);
        let soft_db = percentile(&self.sung_levels, 0.20);
        let mean_db = percentile(&self.sung_levels, 0.60);
        // Sibilants: at least a quarter of a second of them.
        let sibilance = (self.sib_frames as f32 * self.frame_s() > 0.25).then(|| {
            #[allow(clippy::cast_possible_truncation)]
            let level = (self.sib_level_sum / f64::from(self.sib_frames)) as f32;
            let (mut num, mut den) = (0.0_f64, 0.0_f64);
            for (i, p) in self.sib.iter().enumerate() {
                let hz = band_hz(i);
                if (SIBILANT.0..=SIBILANT.1).contains(&hz) {
                    num += p * f64::from(hz.ln());
                    den += p;
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            let hz = (num / den.max(1e-30)).exp() as f32;
            (level - mean_db, hz)
        });
        // Silence: at least half a second between the phrases.
        let noise_db = (self.quiet_frames as f32 * self.frame_s() > 0.5)
            .then(|| percentile(&self.quiet_levels, 0.5));
        Some(Analysis {
            low_hz: note_at(0.05),
            median_hz: note_at(0.5),
            high_hz: note_at(0.95),
            input: db(&self.sung_in),
            layers: db(&self.sung_lay),
            sibilance,
            loud_db,
            soft_db,
            noise_db,
            voiced_s: self.voiced_seconds(),
        })
    }
}

/// How far towards the style, 0 to 1, in three parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Amounts {
    /// Tone: the output EQ, the de-esser, the exciter.
    pub clarity: f32,
    /// Dynamics: the compressor.
    pub control: f32,
    /// Reverb and delay: the layers' sends. At 0 they are left as
    /// they are.
    pub space: f32,
}

impl Amounts {
    /// The same amount everywhere.
    #[must_use]
    pub const fn all(amount: f32) -> Self {
        Self {
            clarity: amount,
            control: amount,
            space: amount,
        }
    }
}

/// Every parameter a proposal may set (what Undo has to remember).
pub const PARAMS: [ParamId; 47] = [
    ParamId::FxOn,
    ParamId::ToneOn,
    ParamId::LowCut,
    ParamId::DynamicsOn,
    ParamId::Gate,
    ParamId::DeEss,
    ParamId::DeEssFreq,
    ParamId::CompThreshold,
    ParamId::CompRatio,
    ParamId::CompAttack,
    ParamId::CompRelease,
    ParamId::CompMakeup,
    ParamId::EqOn,
    ParamId::EqLowCut,
    ParamId::EqLowShelfGain,
    EQ_BELL_PARAMS[0][0],
    EQ_BELL_PARAMS[0][1],
    EQ_BELL_PARAMS[0][2],
    EQ_BELL_PARAMS[1][0],
    EQ_BELL_PARAMS[1][1],
    EQ_BELL_PARAMS[1][2],
    EQ_BELL_PARAMS[2][0],
    EQ_BELL_PARAMS[2][1],
    EQ_BELL_PARAMS[2][2],
    ParamId::EqHighShelfFreq,
    ParamId::EqHighShelfGain,
    ParamId::ExciterAmount,
    ParamId::ReverbOn,
    ParamId::ReverbLevel,
    ParamId::ReverbDecay,
    ParamId::ReverbPredelay,
    ParamId::ReverbLowCut,
    ParamId::DelayOn,
    ParamId::DelaySync,
    ParamId::DelayDivision,
    ParamId::DelayFeedback,
    ParamId::DelayLevel,
    ParamId::DelayLowCut,
    ParamId::DelayDuck,
    REVERB_SEND_PARAMS[0],
    REVERB_SEND_PARAMS[1],
    REVERB_SEND_PARAMS[2],
    REVERB_SEND_PARAMS[3],
    DELAY_SEND_PARAMS[0],
    DELAY_SEND_PARAMS[1],
    DELAY_SEND_PARAMS[2],
    DELAY_SEND_PARAMS[3],
];

/// Parameter values the assistant proposes, in the order they are set.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proposal {
    pub changes: Vec<(ParamId, f64)>,
}

impl Proposal {
    #[must_use]
    pub fn get(&self, id: ParamId) -> Option<f64> {
        self.changes.iter().find(|(p, _)| *p == id).map(|(_, v)| *v)
    }
}

/// How far the EQ moves a zone: the gap to the target, scaled by the
/// amount, with nothing done under 1 dB of gap and never more than
/// the style's reach.
fn correction(target: f32, measured: f32, amount: f32, reach: f32) -> f32 {
    let gap = target - measured;
    if gap.abs() < 1.0 {
        return 0.0;
    }
    ((gap - gap.signum()) * amount).clamp(-reach, reach)
}

/// De-esser amount (0-100) for sibilants `rel` dB against the voice.
fn deess_amount(rel: f32, bias: f32, amount: f32) -> f32 {
    // Sibilants 12 dB under the voice need nothing; level with it, a
    // lot.
    (((rel + 12.0 + bias) * 5.0).clamp(0.0, 70.0) * amount).round()
}

struct Plan {
    low_cut: f32,
    gate: Option<f32>,
    deess: Option<(f32, f32)>,
    /// Threshold, ratio, make-up, reduction aimed at.
    comp: Option<(f32, f32, f32, f32)>,
    mud: (f32, f32),
    resonance: Option<(f32, f32)>,
    presence: f32,
    air: f32,
    exciter: f32,
    /// The lead voice's reverb and delay sends, %; `None`: the space
    /// is left alone.
    space: Option<(f32, f32)>,
}

/// The lead voice's sends with Space at 100 %, and each layer's share
/// of them (voice, creative, harmony, vocoder): the lead stays the
/// driest in reverb and carries the echoes, the harmonies sit further
/// back, the vocoder stays close to dry.
const LEAD_SEND: f32 = 20.0;
const REVERB_SHARE: [f32; 4] = [1.0, 0.8, 1.5, 0.5];
const DELAY_SHARE: [f32; 4] = [1.0, 0.6, 0.4, 0.3];

fn plan(a: &Analysis, style: Style, amounts: Amounts) -> Plan {
    let r = style.rules();
    let amount = amounts.clarity.clamp(0.0, 1.0);
    let control = amounts.control.clamp(0.0, 1.0);
    let space = amounts.space.clamp(0.0, 1.0);
    // Under the lowest note there is only rumble and breath.
    let low_cut = (0.8 * a.low_hz).clamp(40.0, 200.0).round();
    // The gate opens over the noise and the breaths, well under the
    // soft phrases.
    let gate = a
        .noise_db
        .filter(|n| *n > -72.0)
        .map(|n| (n + 12.0).min(a.soft_db - 12.0).clamp(-78.0, -35.0).round());
    let deess = a.sibilance.and_then(|(rel, hz)| {
        let amt = deess_amount(rel, r.deess_bias, amount);
        (amt >= 5.0).then_some((amt, (hz / 100.0).round() * 100.0))
    });
    // Compressor: the style's reduction on the loud phrases, less when
    // the voice is already even.
    let spread = (a.loud_db - a.soft_db).max(0.0);
    let even = (spread / 8.0).clamp(0.5, 1.0);
    let reduction = r.reduction * control * even;
    let comp = (reduction >= 1.0).then(|| {
        // The detector follows peaks: a few dB over the RMS level.
        let peak = a.loud_db + 4.0;
        let threshold = (peak - reduction / (1.0 - 1.0 / r.ratio)).clamp(-48.0, 0.0);
        (
            threshold.round(),
            r.ratio,
            (0.5 * reduction).round(),
            reduction,
        )
    });
    let mud = correction(r.mud, a.balance(MUD), amount, r.reach);
    let presence = correction(r.presence, a.balance(PRESENCE), amount, r.reach);
    let air = correction(r.air, a.balance(AIR), amount, r.reach);
    let resonance = a
        .resonance()
        .map(|(hz, over)| (hz, -((over - 2.0).min(5.0) * amount)))
        .filter(|(_, db)| *db <= -1.0);
    Plan {
        low_cut,
        gate,
        deess,
        comp,
        mud: (MUD_HZ, mud),
        resonance,
        presence,
        air,
        // A voice whose top the EQ has to pull down needs no more of it.
        exciter: if air >= 0.0 {
            (r.exciter * amount).round()
        } else {
            0.0
        },
        space: (space >= 0.01).then(|| {
            let send = LEAD_SEND * space;
            (send, if r.delay.0 > 0.0 { send } else { 0.0 })
        }),
    }
}

/// Settings for the voice effects from what was heard: the source
/// cleaned at the input (low cut, gate, de-esser, compressor), the
/// colour set on the output EQ. `amount` (0-1) is how far towards the
/// style the EQ, the de-esser and the compressor go; the low cut and
/// the gate are cleaning and do not depend on it.
#[must_use]
pub fn propose(a: &Analysis, style: Style, amount: Amounts) -> Proposal {
    let r = style.rules();
    let p = plan(a, style, amount);
    let half = |v: f32| f64::from((v * 2.0).round() / 2.0 + 0.0);
    let mut c: Vec<(ParamId, f64)> = vec![
        (ParamId::FxOn, 1.0),
        (ParamId::ToneOn, 1.0),
        (ParamId::LowCut, f64::from(p.low_cut)),
        (ParamId::DynamicsOn, 1.0),
        (
            ParamId::Gate,
            f64::from(p.gate.unwrap_or(crate::fx::GATE_OFF_DB)),
        ),
    ];
    match p.deess {
        Some((amt, hz)) => {
            c.push((ParamId::DeEss, f64::from(amt)));
            c.push((ParamId::DeEssFreq, f64::from(hz.clamp(3000.0, 12_000.0))));
        }
        None => c.push((ParamId::DeEss, 0.0)),
    }
    match p.comp {
        Some((threshold, ratio, makeup, _)) => c.extend([
            (ParamId::CompThreshold, f64::from(threshold)),
            (ParamId::CompRatio, f64::from(ratio)),
            (ParamId::CompAttack, f64::from(r.attack_ms)),
            (ParamId::CompRelease, f64::from(r.release_ms)),
            (ParamId::CompMakeup, f64::from(makeup)),
        ]),
        None => c.extend([(ParamId::CompRatio, 1.0), (ParamId::CompMakeup, 0.0)]),
    }
    let [mud, res, pres] = EQ_BELL_PARAMS;
    c.extend([
        (ParamId::EqOn, 1.0),
        (ParamId::EqLowCut, f64::from(r.low_cut_hz)),
        (ParamId::EqLowShelfGain, 0.0),
        (mud[0], f64::from(p.mud.0.round())),
        (mud[1], half(p.mud.1)),
        (mud[2], 1.0),
    ]);
    match p.resonance {
        Some((hz, db)) => c.extend([
            (res[0], f64::from(hz.round())),
            (res[1], half(db)),
            (res[2], 3.0),
        ]),
        None => c.push((res[1], 0.0)),
    }
    c.extend([
        (pres[0], 3500.0),
        (pres[1], half(p.presence)),
        (pres[2], 0.8),
        (ParamId::EqHighShelfFreq, 9000.0),
        (ParamId::EqHighShelfGain, half(p.air)),
    ]);
    c.push((ParamId::ExciterAmount, f64::from(p.exciter)));
    if let Some((reverb, delay)) = p.space {
        // The style sets the returns; Space doses each layer's sends.
        c.extend([
            (ParamId::ReverbOn, 1.0),
            (ParamId::ReverbLevel, f64::from(r.reverb.0)),
            (ParamId::ReverbDecay, f64::from(r.reverb.1)),
            (ParamId::ReverbPredelay, f64::from(r.reverb.2)),
            (ParamId::ReverbLowCut, 180.0),
        ]);
        for (id, share) in REVERB_SEND_PARAMS.iter().zip(REVERB_SHARE) {
            c.push((*id, f64::from((reverb * share).round())));
        }
        for (id, share) in DELAY_SEND_PARAMS.iter().zip(DELAY_SHARE) {
            c.push((*id, f64::from((delay * share).round())));
        }
        if delay > 0.0 {
            let division = Division::ALL
                .iter()
                .position(|d| *d == r.delay.1)
                .unwrap_or(0);
            #[allow(clippy::cast_precision_loss)]
            c.extend([
                (ParamId::DelayOn, 1.0),
                (ParamId::DelayLevel, f64::from(r.delay.0)),
                (ParamId::DelaySync, 1.0),
                (ParamId::DelayDivision, division as f64),
                (ParamId::DelayFeedback, f64::from(r.delay.2)),
                (ParamId::DelayLowCut, 200.0),
                // The echoes step back while the voice sings.
                (ParamId::DelayDuck, 40.0),
            ]);
        }
    }
    for (id, v) in &mut c {
        *v = id.spec().kind.sanitize(*v);
    }
    Proposal { changes: c }
}

/// Name of the note nearest to `hz` ("A2").
fn note_name(hz: f32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    #[allow(clippy::cast_possible_truncation)]
    let n = (69.0 + 12.0 * (hz.max(1.0) / 440.0).log2()).round() as i32;
    #[allow(clippy::cast_sign_loss)]
    let name = NAMES[n.rem_euclid(12) as usize];
    format!("{name}{}", n.div_euclid(12) - 1)
}

fn hz_words(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

/// What was heard and what is proposed, in a few short lines.
#[must_use]
pub fn describe(a: &Analysis, style: Style, amount: Amounts) -> Vec<String> {
    let r = style.rules();
    let p = plan(a, style, amount);
    let mut out = vec![format!(
        "{}, {} to {}. Low cut at {:.0} Hz.",
        a.voice_type(),
        note_name(a.low_hz),
        note_name(a.high_hz),
        p.low_cut
    )];
    // Tone: each zone against the style, in words.
    let gap = |target: f32, zone: (f32, f32)| a.balance(zone) - target;
    let mut tone: Vec<String> = Vec::new();
    let mud = gap(r.mud, MUD);
    if mud > 1.5 {
        tone.push("muddy".to_owned());
    } else if mud < -2.5 {
        tone.push("thin".to_owned());
    }
    if let Some((hz, _)) = p.resonance {
        tone.push(format!("hard at {}", hz_words(hz)));
    }
    let pres = gap(r.presence, PRESENCE);
    if pres < -1.5 {
        tone.push("lacks presence".to_owned());
    } else if pres > 2.0 {
        tone.push("forward".to_owned());
    }
    let air = gap(r.air, AIR);
    if air < -2.0 {
        tone.push("little air".to_owned());
    } else if air > 2.5 {
        tone.push("bright".to_owned());
    }
    out.push(if tone.is_empty() {
        format!("Tone close to the {} target.", style.name())
    } else {
        let mut t = tone.join(", ");
        if let Some(first) = t.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{t}.")
    });
    out.push(match (a.sibilance, p.deess) {
        (Some((_, hz)), Some((amt, _))) => {
            format!("Sibilants at {}: de-esser at {amt:.0} %.", hz_words(hz))
        }
        (Some(_), None) => "Sibilants under control.".to_owned(),
        (None, _) => "No sibilants heard.".to_owned(),
    });
    let spread = a.loud_db - a.soft_db;
    out.push(match p.comp {
        Some((_, ratio, _, red)) => {
            format!("{spread:.0} dB soft to loud: {ratio:.0}:1, about {red:.0} dB of reduction.",)
        }
        None => format!("{spread:.0} dB soft to loud: no compression."),
    });
    out.push(match (a.noise_db, p.gate) {
        (Some(n), Some(g)) => format!("Noise at {n:.0} dB between phrases: gate at {g:.0} dB."),
        (Some(n), None) => format!("Quiet between phrases ({n:.0} dB): no gate."),
        (None, _) => "No pause heard: gate left off.".to_owned(),
    });
    out.push(match p.space {
        Some((send, delay)) if delay > 0.0 => format!(
            "Reverb {:.1} s, {} delay: lead sends at {send:.0} %.",
            r.reverb.1,
            r.delay.1.name()
        ),
        Some((send, _)) => format!(
            "Reverb {:.1} s, no delay: lead send at {send:.0} %.",
            r.reverb.1
        ),
        None => "Reverb and delay left as they are.".to_owned(),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sung vowel at `f0` with a falling spectrum, `tilt` dB per
    /// octave, then noise bursts as sibilants and a quiet tail.
    fn sing(sr: f32, f0: f32, tilt: f32, secs: f32, sibilants: f32, noise: f32) -> Listener {
        let mut l = Listener::new(f64::from(sr));
        let mut seed = 22_222_u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            #[allow(clippy::cast_precision_loss)]
            let v = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            v
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (secs * sr) as usize;
        let mut block = Vec::with_capacity(512);
        let mut hp = 0.0_f32;
        for start in (0..n).step_by(512) {
            block.clear();
            // Phrases of 1.5 s: sung, a sibilant, a pause.
            #[allow(clippy::cast_precision_loss)]
            let t0 = (start as f32 / sr) % 1.5;
            let (voiced, sib) = (t0 < 1.0, (1.0..1.15).contains(&t0));
            for i in start..start + 512 {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f32 / sr;
                let mut x = noise * rnd();
                if voiced {
                    let swell = 0.6 + 0.4 * (t * 2.1).sin();
                    for h in 1..=60 {
                        #[allow(clippy::cast_precision_loss)]
                        let f = f0 * h as f32;
                        if f > 0.45 * sr {
                            break;
                        }
                        let a = 10.0_f32.powf(tilt * (f / f0).log2() / 20.0);
                        x += 0.2 * swell * a * (2.0 * std::f32::consts::PI * f * t).sin();
                    }
                } else if sib {
                    // White noise through a crude high-pass.
                    let w = rnd();
                    hp = 0.9 * hp + w;
                    x += sibilants * (w - 0.1 * hp);
                }
                block.push(x);
            }
            l.feed(&block, &block, if voiced { f0 } else { 0.0 });
        }
        l
    }

    #[test]
    fn hears_range_dynamics_sibilants_and_noise() {
        let a = sing(48_000.0, 146.8, -9.0, 12.0, 0.2, 0.001)
            .finish()
            .expect("enough singing");
        assert!((a.median_hz - 146.8).abs() < 5.0, "{}", a.median_hz);
        assert_eq!(a.voice_type(), "Baritone");
        assert!(a.loud_db > a.soft_db + 1.0);
        let (rel, hz) = a.sibilance.expect("sibilants");
        assert!(
            rel > -15.0 && (4000.0..12_000.0).contains(&hz),
            "{rel} {hz}"
        );
        let noise = a.noise_db.expect("pauses");
        assert!((-72.0..-55.0).contains(&noise), "{noise}");
        // The spectrum falls as it was made to.
        assert!(a.balance(PRESENCE) < -10.0 && a.balance(AIR) < a.balance(PRESENCE));
    }

    #[test]
    fn too_little_singing_gives_no_diagnosis() {
        assert!(
            sing(48_000.0, 220.0, -9.0, 1.5, 0.0, 0.0)
                .finish()
                .is_none()
        );
    }

    #[test]
    fn proposal_brightens_a_dull_voice_and_darkens_a_bright_one() {
        let dull = sing(48_000.0, 220.0, -12.0, 9.0, 0.05, 0.0)
            .finish()
            .expect("analysis");
        let p = propose(&dull, Style::Pop, Amounts::all(1.0));
        assert!(p.get(ParamId::EqHighShelfGain).expect("air") > 2.0);
        assert!(p.get(EQ_BELL_PARAMS[2][1]).expect("presence") > 1.0);
        let bright = sing(48_000.0, 220.0, -3.0, 9.0, 0.05, 0.0)
            .finish()
            .expect("analysis");
        let q = propose(&bright, Style::Pop, Amounts::all(1.0));
        assert!(q.get(ParamId::EqHighShelfGain).expect("air") < -2.0);
        // The low cut sits under the lowest note; values stay in range.
        assert!(p.get(ParamId::LowCut).expect("low cut") < 220.0);
        for (id, v) in &p.changes {
            assert!((id.spec().kind.sanitize(*v) - v).abs() < 1e-9, "{id:?}");
        }
    }

    #[test]
    fn amount_scales_and_zero_leaves_the_colour_alone() {
        let a = sing(48_000.0, 220.0, -12.0, 9.0, 0.3, 0.002)
            .finish()
            .expect("analysis");
        let none = propose(&a, Style::Trap, Amounts::all(0.0));
        assert!(none.get(ParamId::EqHighShelfGain).expect("air").abs() < 1e-9);
        assert!((none.get(ParamId::CompRatio).expect("ratio") - 1.0).abs() < 1e-9);
        assert!(none.get(ParamId::DeEss).expect("de-ess").abs() < 1e-9);
        let half = propose(&a, Style::Trap, Amounts::all(0.5));
        let full = propose(&a, Style::Trap, Amounts::all(1.0));
        let air = |p: &Proposal| p.get(ParamId::EqHighShelfGain).expect("air");
        assert!(air(&half) > 0.0 && air(&full) >= air(&half));
        // The space follows its own amount, and stays put at zero.
        assert!(none.get(ParamId::ReverbLevel).is_none());
        let wet = |p: &Proposal| p.get(REVERB_SEND_PARAMS[0]).expect("lead reverb send");
        assert!(wet(&full) > wet(&half) && half.get(ParamId::DelayLevel).expect("delay") > 0.0);
        // The harmonies sit further back than the lead, in reverb.
        assert!(full.get(REVERB_SEND_PARAMS[2]).expect("harmony send") > wet(&full));
        assert!(full.get(DELAY_SEND_PARAMS[2]).expect("harmony echo") < wet(&full));
        let dry = propose(
            &a,
            Style::Trap,
            Amounts {
                space: 0.0,
                ..Amounts::all(1.0)
            },
        );
        assert!(dry.get(ParamId::ReverbLevel).is_none() && air(&dry) > 0.0);
        // Stronger styles compress more.
        let thr = |s| {
            propose(&a, s, Amounts::all(1.0))
                .get(ParamId::CompThreshold)
                .expect("threshold")
        };
        assert!(thr(Style::Trap) < thr(Style::Natural));
        // Words for every style, and nothing set outside the list.
        for s in Style::ALL {
            assert!(
                propose(&a, s, Amounts::all(1.0))
                    .changes
                    .iter()
                    .all(|(id, _)| PARAMS.contains(id))
            );
            let lines = describe(&a, s, Amounts::all(1.0));
            assert_eq!(lines.len(), 6);
            assert!(
                lines.iter().all(|l| !l.is_empty() && l.len() < 56),
                "{lines:?}"
            );
        }
    }
}
