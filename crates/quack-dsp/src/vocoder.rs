//! Vocoder and talkbox (roadmap step 7), Synth mode.
//!
//! The voice (the *modulator*) shapes a *carrier*: an internal synth
//! played by the MIDI input, so no external synth is needed live, or
//! the side-chain input.
//!
//! - **Vocoder**: a bank of 8 to [`MAX_BANDS`] band-pass filters
//!   (two biquads each) on the voice and the same bank on the carrier.
//!   Each voice band's envelope sets the level of the carrier band,
//!   whose own envelope is divided out first, so any carrier (bright
//!   saw, dark side-chain pad) gets the voice's spectrum. *Band Shift*
//!   moves the carrier bank against the voice bank: higher or lower
//!   "mouth".
//! - **Talkbox**: linear prediction (LPC) of the voice every couple of
//!   milliseconds, the all-pole filter applied to the carrier (a
//!   lattice, stable while its coefficients glide between frames). The
//!   carrier goes "through the mouth": vowels are much clearer than on
//!   a vocoder, closer to a real talkbox.
//!
//! In both, *Sibilance* adds the high part of the voice back in
//! unvoiced parts (s, t, ch), which a pitched carrier cannot say;
//! *Band Shift* moves the "mouth" (the talkbox reads its analysis frame
//! faster or slower); *Hold* freezes the timbre on the carrier, played
//! by MIDI; *Width* spreads the result in stereo by phase only, so the
//! mono sum is untouched; *Lo-Fi* lowers the rate and the bits (a
//! talking toy at the top).
//!
//! The carrier synth: up to [`CARRIER_VOICES`] voices, one per held MIDI
//! note (saw, square, supersaw, or noise for a whisper), *Detune*
//! between the oscillators, *Carrier Noise* mixed in, *Carrier Release*.
//! With *Carrier Follow* and no note held, the carrier sings the tuned
//! note of the singer (autotune output plus Pitch), alone or as a chord
//! (*Follow Chord*), so the vocoder works without a keyboard. MIDI
//! notes glide (*Carrier Glide*) and follow the pitch bend.
//!
//! Latency: none added. The filters are causal; the output is delayed
//! to line up with the rest of the engine, by the engine's latency minus
//! the analysis lag (the talkbox's frame is centred in the past), so the
//! budget the PSOLA already pays for absorbs the analysis.
//!
//! Real-time: buffers are allocated in [`Vocoder::prepare`], fixed
//! arrays elsewhere, nothing allocated in [`Vocoder::tick`].

use crate::latency::DelayLine;
use crate::midi::NoteStack;

/// Most vocoder bands (fixed, allocated in `prepare`).
pub const MAX_BANDS: usize = 32;
/// Band counts offered by the Bands parameter.
pub const BAND_CHOICES: [usize; 4] = [8, 16, 24, 32];
/// Carrier synth polyphony.
pub const CARRIER_VOICES: usize = 8;
/// Oscillators per carrier voice (the supersaw uses them all).
const OSCS: usize = 5;
/// Lowest and highest band centres (Hz).
const BAND_LO: f64 = 110.0;
const BAND_HI: f64 = 11_000.0;
/// Carrier band envelope (whitening), seconds.
const CARRIER_ENV_S: f64 = 0.008;
/// Floor of the carrier envelopes: under it the carrier counts as
/// silent (no noise floor blown up to the voice's level).
const CARRIER_FLOOR: f64 = 1e-3;
/// Talkbox analysis window and frames per window.
const LPC_WINDOW_S: f64 = 0.02;
const LPC_HOPS: usize = 8;
/// Most LPC order (the order grows with the sample rate).
const MAX_ORDER: usize = 48;
/// Pre-emphasis of the voice before the analysis (flattens the glottal
/// tilt; the carrier brings its own).
const PRE_EMPHASIS: f64 = 0.95;
/// Bandwidth expansion of the LPC poles (Hz), against whistling peaks.
const LAG_WINDOW_HZ: f64 = 50.0;
/// Sibilance: high-pass corner and the voicing fade.
const SIBILANCE_HZ: f64 = 3_500.0;
const VOICING_S: f64 = 0.01;
/// Carrier voice attack, and the Follow voice's glide.
const ATTACK_S: f64 = 0.004;
const FOLLOW_GLIDE_S: f64 = 0.02;
/// Follow voice: a leap wider than this must last [`LEAP_HOLD_S`].
const LEAP_ST: f64 = 7.0;
const LEAP_HOLD_S: f64 = 0.04;
/// Width: first-order allpass coefficients (a phase smear, no level
/// change, so left plus right is the mono voice exactly).
const WIDE_STAGES: usize = 4;
const WIDE_COEFS: [f64; WIDE_STAGES] = [0.62, -0.35, 0.8, -0.55];
/// Soft limit of the vocoded voice (a low, whitened carrier is spiky).
const SOFT_KNEE: f64 = 0.5;
/// Level calibration: a voice at some level comes out about as loud.
const VOCODER_MAKEUP: f64 = 2.25;
const TALKBOX_MAKEUP: f64 = 5.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VocoderMode {
    #[default]
    Off,
    Vocoder,
    Talkbox,
}

impl VocoderMode {
    /// The choices of the parameter (`Off` is the layer switched off,
    /// the `vocoder_on` parameter).
    pub const COUNT: u32 = 2;
    pub const ALL: [Self; 2] = [Self::Vocoder, Self::Talkbox];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        Self::ALL[index(i, Self::ALL.len())]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Vocoder => "Vocoder",
            Self::Talkbox => "Talkbox",
        }
    }
}

/// What the voice shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Carrier {
    #[default]
    Saw,
    Square,
    Supersaw,
    /// White noise: a whisper (no pitch, MIDI only opens it).
    Noise,
    /// The side-chain input (another track, an external synth).
    Sidechain,
}

impl Carrier {
    pub const COUNT: u32 = 5;
    pub const ALL: [Self; 5] = [
        Self::Saw,
        Self::Square,
        Self::Supersaw,
        Self::Noise,
        Self::Sidechain,
    ];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        Self::ALL[index(i, Self::ALL.len())]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Saw => "Saw",
            Self::Square => "Square",
            Self::Supersaw => "Supersaw",
            Self::Noise => "Noise",
            Self::Sidechain => "Sidechain",
        }
    }
}

/// Notes the Follow carrier sings around the tuned note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowChord {
    #[default]
    Single,
    /// The note and the octave below.
    Octave,
    /// The note and the fifth above.
    Fifth,
    Major,
    Minor,
    /// Third and fifth taken in the autotune's scale.
    Scale,
}

impl FollowChord {
    pub const COUNT: u32 = 6;
    pub const ALL: [Self; 6] = [
        Self::Single,
        Self::Octave,
        Self::Fifth,
        Self::Major,
        Self::Minor,
        Self::Scale,
    ];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        Self::ALL[index(i, Self::ALL.len())]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Octave => "Octave",
            Self::Fifth => "Fifth",
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Scale => "Scale",
        }
    }

    /// Offsets in semitones from `note`, and how many there are.
    /// `scale`: allowed pitch classes (bit 0 = C), for [`Self::Scale`].
    #[must_use]
    pub fn offsets(self, note: f64, scale: u16) -> ([f64; FOLLOW_SLOTS], usize) {
        match self {
            Self::Single => ([0.0, 0.0, 0.0], 1),
            Self::Octave => ([0.0, -12.0, 0.0], 2),
            Self::Fifth => ([0.0, 7.0, 0.0], 2),
            Self::Major => ([0.0, 4.0, 7.0], 3),
            Self::Minor => ([0.0, 3.0, 7.0], 3),
            Self::Scale => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let pc = (note.round().rem_euclid(12.0)) as u32;
                let has = |st: u32| scale & (1 << ((pc + st) % 12)) != 0;
                let third = if has(4) {
                    4.0
                } else if has(3) {
                    3.0
                } else {
                    4.0
                };
                let fifth = if has(7) {
                    7.0
                } else if has(6) {
                    6.0
                } else if has(8) {
                    8.0
                } else {
                    7.0
                };
                ([0.0, third, fifth], 3)
            }
        }
    }
}

/// Most Follow voices (the chord's notes).
pub const FOLLOW_SLOTS: usize = 3;

fn index(i: f64, len: usize) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let i = i.max(0.0).round() as usize;
    i.min(len - 1)
}

/// Band count from the Bands parameter's index.
#[must_use]
pub fn bands_from_index(i: f64) -> usize {
    BAND_CHOICES[index(i, BAND_CHOICES.len())]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VocoderSettings {
    pub mode: VocoderMode,
    pub carrier: Carrier,
    pub bands: usize,
    /// Vocoder band envelope release (attack is a quarter of it).
    pub response_s: f64,
    /// Carrier bank against the voice bank, semitones.
    pub shift_st: f64,
    /// 0..1.
    pub sibilance: f64,
    /// Carrier octave, -2..=2.
    pub octave: i32,
    pub detune_cents: f64,
    /// Noise mixed into the carrier, 0..1.
    pub noise: f64,
    pub release_s: f64,
    /// Without MIDI, the carrier sings the tuned note.
    pub follow: bool,
    /// Notes the Follow carrier sings.
    pub chord: FollowChord,
    /// Scale of the [`FollowChord::Scale`] chord (bit 0 = C).
    pub scale: u16,
    /// Portamento of the MIDI carrier, seconds (0 = none).
    pub glide_s: f64,
    /// Freeze the timbre: the voice's last spectrum stays on the
    /// carrier (played by MIDI) until released.
    pub hold: bool,
    /// Pitch bend range, semitones.
    pub bend_range: f64,
    /// Stereo width of the vocoded voice, 0..1 (mono-safe).
    pub width: f64,
    /// Lo-fi: sample rate and bit depth come down together, 0..1
    /// (talking-toy voice at the top).
    pub lofi: f64,
}

impl Default for VocoderSettings {
    fn default() -> Self {
        Self {
            mode: VocoderMode::Off,
            carrier: Carrier::Saw,
            bands: 24,
            response_s: 0.012,
            shift_st: 0.0,
            sibilance: 0.5,
            octave: 0,
            detune_cents: 12.0,
            noise: 0.05,
            release_s: 0.08,
            follow: true,
            chord: FollowChord::Single,
            scale: 0xFFF,
            glide_s: 0.0,
            hold: false,
            bend_range: 2.0,
            width: 0.5,
            lofi: 0.0,
        }
    }
}

/// Transposed direct form II biquad.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Band-pass, 0 dB at the centre.
    fn band_pass(&mut self, sr: f64, hz: f64, q: f64) {
        let w = std::f64::consts::TAU * hz.clamp(10.0, 0.49 * sr) / sr;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b1 = 0.0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * w.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn high_pass(&mut self, sr: f64, hz: f64) {
        let w = std::f64::consts::TAU * hz.clamp(10.0, 0.49 * sr) / sr;
        let alpha = w.sin() / std::f64::consts::SQRT_2;
        let (c, a0) = (w.cos(), 1.0 + alpha);
        self.b0 = (1.0 + c) / 2.0 / a0;
        self.b1 = -(1.0 + c) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// One band: two band-passes in series, and its envelope.
#[derive(Debug, Clone, Copy, Default)]
struct Band {
    f: [Biquad; 2],
    env: f64,
}

impl Band {
    fn set(&mut self, sr: f64, hz: f64, q: f64) {
        for f in &mut self.f {
            f.band_pass(sr, hz, q);
        }
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.f[0].tick(x);
        self.f[1].tick(y)
    }

    fn reset(&mut self) {
        self.f.iter_mut().for_each(Biquad::reset);
        self.env = 0.0;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    /// MIDI note (fractional for the Follow voice), `None` = free.
    note: Option<f64>,
    held: bool,
    /// A Follow voice (not a MIDI note), and its place in the chord.
    follow: bool,
    slot: usize,
    /// Current and target frequency, Hz.
    hz: f64,
    target_hz: f64,
    gain: f64,
    peak: f64,
    phase: [f64; OSCS],
    /// Press order, to steal the oldest.
    age: u64,
}

/// `x % 1.0` for `x` in `0..2` (a phase plus at most one cycle),
/// exactly, without the `fmod` call.
#[inline]
fn wrap(x: f64) -> f64 {
    if x >= 1.0 { x - 1.0 } else { x }
}

/// PolyBLEP correction around a discontinuity at phase 0.
#[inline]
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// Oscillator spread of the supersaw, in units of Detune.
const SPREAD: [f64; OSCS] = [0.0, 1.0, -1.0, 0.45, -0.45];

#[derive(Debug, Clone)]
pub struct Vocoder {
    pub settings: VocoderSettings,
    sample_rate: f64,
    applied: Option<VocoderSettings>,
    // Vocoder.
    voice_bands: [Band; MAX_BANDS],
    carrier_bands: [Band; MAX_BANDS],
    band_att: f64,
    band_rel: f64,
    carrier_env: f64,
    carrier_att: f64,
    carrier_rel: f64,
    // Talkbox.
    window: Vec<f64>,
    ring: Vec<f64>,
    frame: Vec<f64>,
    ring_pos: usize,
    hop: usize,
    hop_count: usize,
    order: usize,
    k: [f64; MAX_ORDER + 1],
    k_step: [f64; MAX_ORDER + 1],
    lattice: [f64; MAX_ORDER + 1],
    lpc_gain: f64,
    lpc_gain_step: f64,
    window_power: f64,
    prev_in: f64,
    carrier_rms: f64,
    rms_coef: f64,
    // Sibilance.
    sib_hp: [Biquad; 2],
    unvoiced: f64,
    voicing_coef: f64,
    // Carrier synth.
    voices: [Voice; CARRIER_VOICES],
    clock: u64,
    noise_state: u32,
    attack_coef: f64,
    release_coef: f64,
    glide_coef: f64,
    notes: NoteStack,
    /// The tuned note the Follow voices are built on.
    follow_note: Option<f64>,
    /// Pitch bend, -1..1, and the MIDI portamento.
    bend: f64,
    /// Detune (cents) the oscillator ratios below were computed for:
    /// `exp2` once per change, not per oscillator and sample.
    detune_key: f64,
    detune_ratio: [f64; OSCS],
    midi_glide_coef: f64,
    last_hz: f64,
    leap_count: usize,
    leap_hold: usize,
    /// Allpass chain for the width (side = allpassed - direct).
    wide: [f64; WIDE_STAGES],
    /// Lo-fi sample and hold: phase and held value.
    crush_phase: f64,
    crush_held: f64,
    // Output alignment.
    out_delay: DelayLine,
    latency: usize,
    level: f64,
}

impl Default for Vocoder {
    fn default() -> Self {
        let mut v = Self {
            settings: VocoderSettings::default(),
            sample_rate: 48_000.0,
            applied: None,
            voice_bands: [Band::default(); MAX_BANDS],
            carrier_bands: [Band::default(); MAX_BANDS],
            band_att: 0.0,
            band_rel: 0.0,
            carrier_env: 0.0,
            carrier_att: 0.0,
            carrier_rel: 0.0,
            window: Vec::new(),
            ring: Vec::new(),
            frame: Vec::new(),
            ring_pos: 0,
            hop: 1,
            hop_count: 0,
            order: 0,
            k: [0.0; MAX_ORDER + 1],
            k_step: [0.0; MAX_ORDER + 1],
            lattice: [0.0; MAX_ORDER + 1],
            lpc_gain: 0.0,
            lpc_gain_step: 0.0,
            window_power: 1.0,
            prev_in: 0.0,
            carrier_rms: 0.0,
            rms_coef: 0.0,
            sib_hp: [Biquad::default(); 2],
            unvoiced: 0.0,
            voicing_coef: 0.0,
            voices: [Voice::default(); CARRIER_VOICES],
            clock: 0,
            noise_state: 0x1234_5678,
            attack_coef: 0.0,
            release_coef: 0.0,
            glide_coef: 0.0,
            notes: NoteStack::default(),
            follow_note: None,
            bend: 0.0,
            detune_key: f64::NAN,
            detune_ratio: [1.0; OSCS],
            midi_glide_coef: 1.0,
            last_hz: 0.0,
            leap_count: 0,
            leap_hold: 0,
            wide: [0.0; WIDE_STAGES],
            crush_phase: 0.0,
            crush_held: 0.0,
            out_delay: DelayLine::default(),
            latency: 0,
            level: 0.0,
        };
        v.prepare(48_000.0, 4096);
        v
    }
}

fn coef(sr: f64, s: f64) -> f64 {
    1.0 - (-1.0 / (s.max(1e-5) * sr)).exp()
}

impl Vocoder {
    /// Allocates the analysis buffers. Not for the audio thread.
    pub fn prepare(&mut self, sample_rate: f64, max_latency: usize) {
        self.sample_rate = sample_rate;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let w = ((LPC_WINDOW_S * sample_rate) as usize).max(64);
        self.hop = (w / LPC_HOPS).max(1);
        // Order: about one pole pair per kHz of bandwidth, plus a few.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let order = (sample_rate / 2000.0).round() as usize + 4;
        self.order = order.clamp(8, MAX_ORDER);
        #[allow(clippy::cast_precision_loss)]
        let n = w as f64;
        self.window = (0..w)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let x = (i as f64 + 0.5) / n;
                0.5 - 0.5 * (std::f64::consts::TAU * x).cos()
            })
            .collect();
        self.window_power = self.window.iter().map(|w| w * w).sum::<f64>().max(1e-9);
        // Room for a frame read twice as slowly (Band Shift down to
        // an octave up).
        self.ring = vec![0.0; 2 * w + 2];
        self.frame = vec![0.0; w];
        self.out_delay.prepare(max_latency);
        self.rms_coef = coef(sample_rate, 0.01);
        self.carrier_att = coef(sample_rate, 0.0005);
        self.carrier_rel = coef(sample_rate, CARRIER_ENV_S);
        self.voicing_coef = coef(sample_rate, VOICING_S);
        self.attack_coef = coef(sample_rate, ATTACK_S);
        self.glide_coef = coef(sample_rate, FOLLOW_GLIDE_S);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            self.leap_hold = (LEAP_HOLD_S * sample_rate) as usize;
        }
        for f in &mut self.sib_hp {
            f.high_pass(sample_rate, SIBILANCE_HZ);
        }
        self.applied = None;
        self.apply_settings();
        self.set_latency(self.latency.min(max_latency));
        self.reset();
    }

    /// Line the output up with an engine latency of `latency` samples.
    pub fn set_latency(&mut self, latency: usize) {
        self.latency = latency;
        self.out_delay
            .set_delay(latency.saturating_sub(self.analysis_lag()));
    }

    /// How late the analysis follows the voice (samples): half the
    /// talkbox window, or the vocoder bands' attack.
    #[must_use]
    pub fn analysis_lag(&self) -> usize {
        match self.settings.mode {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            VocoderMode::Talkbox => (self.window.len() as f64 * self.talk_shift() / 2.0) as usize,
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            _ => (0.25 * self.settings.response_s * self.sample_rate) as usize,
        }
    }

    pub fn reset(&mut self) {
        for b in self
            .voice_bands
            .iter_mut()
            .chain(self.carrier_bands.iter_mut())
        {
            b.reset();
        }
        self.carrier_env = 0.0;
        self.ring.iter_mut().for_each(|x| *x = 0.0);
        self.ring_pos = 0;
        self.hop_count = 0;
        self.k = [0.0; MAX_ORDER + 1];
        self.k_step = [0.0; MAX_ORDER + 1];
        self.lattice = [0.0; MAX_ORDER + 1];
        self.lpc_gain = 0.0;
        self.lpc_gain_step = 0.0;
        self.prev_in = 0.0;
        self.carrier_rms = 0.0;
        self.sib_hp.iter_mut().for_each(Biquad::reset);
        self.unvoiced = 0.0;
        self.voices = [Voice::default(); CARRIER_VOICES];
        self.notes.clear();
        self.follow_note = None;
        self.last_hz = 0.0;
        self.wide = [0.0; WIDE_STAGES];
        self.crush_phase = 0.0;
        self.crush_held = 0.0;
        self.out_delay.reset();
        self.level = 0.0;
    }

    /// Recompute what depends on the settings (filters, envelopes).
    /// Real-time safe, cheap when nothing changed.
    pub fn apply_settings(&mut self) {
        let s = self.settings;
        let sr = self.sample_rate;
        let changed = |a: Option<VocoderSettings>| a.is_none_or(|a| a != s);
        if !changed(self.applied) {
            return;
        }
        let old = self.applied;
        self.applied = Some(s);
        let bands = s.bands.clamp(2, MAX_BANDS);
        let rebuild = old.is_none_or(|o| o.bands != s.bands || o.shift_st != s.shift_st);
        if rebuild {
            let hi = BAND_HI.min(0.42 * sr);
            #[allow(clippy::cast_precision_loss)]
            let span = (hi / BAND_LO).log2() / (bands - 1) as f64;
            // Bandwidth of one band = the spacing; two stages narrow it,
            // so each is a little wider.
            let q = 2f64.powf(span / 2.0) / (2f64.powf(span) - 1.0) * 0.75;
            let shift = (s.shift_st / 12.0).exp2();
            for i in 0..bands {
                #[allow(clippy::cast_precision_loss)]
                let hz = BAND_LO * (span * i as f64).exp2();
                self.voice_bands[i].set(sr, hz, q);
                self.carrier_bands[i].set(sr, hz * shift, q);
            }
            if old.is_some_and(|o| o.bands != s.bands) {
                for b in self
                    .voice_bands
                    .iter_mut()
                    .chain(self.carrier_bands.iter_mut())
                {
                    b.reset();
                }
            }
        }
        self.band_rel = coef(sr, s.response_s);
        self.band_att = coef(sr, 0.25 * s.response_s);
        self.release_coef = coef(sr, s.release_s / 4.0);
        self.midi_glide_coef = if s.glide_s > 1e-4 {
            coef(sr, s.glide_s / 3.0)
        } else {
            1.0
        };
        if old.is_none_or(|o| {
            o.mode != s.mode || o.response_s != s.response_s || o.shift_st != s.shift_st
        }) {
            self.set_latency(self.latency);
        }
    }

    /// Held MIDI notes: one carrier voice each (newest steal the oldest).
    pub fn set_notes(&mut self, notes: &NoteStack) {
        self.notes = *notes;
        // Release voices whose note was let go.
        for v in &mut self.voices {
            if v.follow {
                continue;
            }
            if let Some(n) = v.note {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let still = notes.held().iter().any(|h| f64::from(h.note) == n);
                if !still {
                    v.held = false;
                }
            }
        }
        for h in notes.held() {
            let n = f64::from(h.note);
            if self
                .voices
                .iter()
                .any(|v| v.held && !v.follow && v.note == Some(n))
            {
                continue;
            }
            self.clock += 1;
            let i = self.free_voice();
            let hz = note_hz(n + 12.0 * f64::from(self.settings.octave));
            // Portamento: from the last note played.
            let from = if self.settings.glide_s > 1e-4 && self.last_hz > 0.0 {
                self.last_hz
            } else {
                hz
            };
            self.last_hz = hz;
            let v = &mut self.voices[i];
            let restart = v.note.is_none() || v.gain < 1e-3;
            *v = Voice {
                note: Some(n),
                held: true,
                follow: false,
                slot: 0,
                hz: from,
                target_hz: hz,
                gain: if restart { 0.0 } else { v.gain },
                peak: 0.55 + 0.45 * f64::from(h.velocity) / 127.0,
                phase: v.phase,
                age: self.clock,
            };
        }
        // The Follow voice gives way to the keyboard.
        if !notes.held().is_empty() {
            for v in &mut self.voices {
                if v.follow {
                    v.held = false;
                }
            }
        }
    }

    /// Lo-fi: hold the signal at a lower rate (down to 4 kHz) and round
    /// it to fewer bits (down to 4).
    fn crush(&mut self, x: f64) -> f64 {
        let a = self.settings.lofi;
        if a <= 0.0 {
            return x;
        }
        let rate = (self.sample_rate * (1.0 - a) + 4_000.0 * a).min(self.sample_rate);
        self.crush_phase += rate / self.sample_rate;
        if self.crush_phase >= 1.0 {
            self.crush_phase -= 1.0;
            let bits = 16.0 - 12.0 * a;
            let steps = bits.exp2() / 2.0;
            self.crush_held = (x * steps).round() / steps;
        }
        self.crush_held
    }

    /// Spread the vocoded voice in stereo: left and right differ in
    /// phase only, their sum is the mono voice.
    pub fn widen(&mut self, y: f32) -> (f32, f32) {
        let x = f64::from(y);
        let mut a = x;
        for (z, c) in self.wide.iter_mut().zip(WIDE_COEFS) {
            let out = c * a + *z;
            *z = a - c * out;
            a = out;
        }
        #[allow(clippy::cast_possible_truncation)]
        let side = (0.5 * self.settings.width * (a - x)) as f32;
        (y + side, y - side)
    }

    /// Talkbox frame reading rate from the Band Shift (an octave either
    /// way).
    fn talk_shift(&self) -> f64 {
        (self.settings.shift_st / 12.0).exp2().clamp(0.5, 2.0)
    }

    /// MIDI pitch bend, -1 to 1 (bends the carrier by the bend range).
    pub fn set_bend(&mut self, bend: f64) {
        self.bend = if bend.is_finite() {
            bend.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }

    fn free_voice(&self) -> usize {
        let score = |v: &Voice| -> (u8, u64) {
            if v.note.is_none() {
                (0, 0)
            } else if !v.held {
                (1, v.age)
            } else {
                (2, v.age)
            }
        };
        (0..CARRIER_VOICES)
            .min_by_key(|&i| score(&self.voices[i]))
            .unwrap_or(0)
    }

    /// Notes the carrier plays now (bit n = note n), for the editor.
    #[must_use]
    pub fn sounding(&self) -> u128 {
        self.voices.iter().fold(0, |m, v| match v.note {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(n) if v.gain > 1e-3 => m | (1u128 << (n.round().clamp(0.0, 127.0) as u32)),
            _ => m,
        })
    }

    #[inline]
    fn noise(&mut self) -> f64 {
        let mut x = self.noise_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise_state = x;
        f64::from(x) / f64::from(u32::MAX) * 2.0 - 1.0
    }

    /// One sample of the carrier synth, and its level (sum of the voice
    /// envelopes, up to 1).
    fn synth(&mut self, follow: Option<f64>) -> (f64, f64) {
        let s = self.settings;
        // The Follow voices: the tuned sung note (and its chord), while
        // nothing is held.
        if s.follow && self.notes.held().is_empty() {
            match follow {
                Some(n) => {
                    // A leap (an octave error of the detector, most
                    // often) must last before the carrier follows it.
                    let leap = self.follow_note.is_some_and(|m| (n - m).abs() > LEAP_ST);
                    self.leap_count = if leap { self.leap_count + 1 } else { 0 };
                    if !leap || self.leap_count > self.leap_hold {
                        self.leap_count = 0;
                        self.follow_note = Some(n);
                    }
                }
                None => self.follow_note = None,
            }
            let base = self.follow_note;
            let (offsets, count) = s.chord.offsets(base.unwrap_or(60.0), s.scale);
            for (slot, off) in offsets.iter().enumerate() {
                let at = self
                    .voices
                    .iter()
                    .position(|v| v.follow && v.held && v.slot == slot && v.note.is_some());
                match (base, slot < count, at) {
                    (Some(b), true, Some(i)) => {
                        let v = &mut self.voices[i];
                        v.note = Some(b + off);
                        v.target_hz = note_hz(b + off + 12.0 * f64::from(s.octave));
                    }
                    (Some(b), true, None) => {
                        self.clock += 1;
                        let i = self.free_voice();
                        let hz = note_hz(b + off + 12.0 * f64::from(s.octave));
                        self.voices[i] = Voice {
                            note: Some(b + off),
                            held: true,
                            follow: true,
                            slot,
                            hz,
                            target_hz: hz,
                            gain: 0.0,
                            peak: if slot == 0 { 0.8 } else { 0.6 },
                            phase: self.voices[i].phase,
                            age: self.clock,
                        };
                    }
                    (_, _, Some(i)) => self.voices[i].held = false,
                    _ => {}
                }
            }
        } else {
            self.follow_note = None;
            for v in &mut self.voices {
                if v.follow {
                    v.held = false;
                }
            }
        }
        let sr = self.sample_rate;
        let detune = s.detune_cents / 1200.0;
        let oscs = match s.carrier {
            Carrier::Supersaw => OSCS,
            _ if s.detune_cents > 0.05 => 2,
            _ => 1,
        };
        #[allow(clippy::cast_precision_loss)]
        let norm = 1.0 / (oscs as f64).sqrt();
        let (att, rel) = (self.attack_coef, self.release_coef);
        let bend = (self.bend * s.bend_range / 12.0).exp2();
        if self.detune_key != s.detune_cents {
            self.detune_key = s.detune_cents;
            for (r, spread) in self.detune_ratio.iter_mut().zip(SPREAD) {
                *r = (spread * detune).exp2();
            }
        }
        let ratios = self.detune_ratio;
        let mut out = 0.0;
        let mut level = 0.0;
        for v in &mut self.voices {
            if v.note.is_none() {
                continue;
            }
            let goal = if v.held { v.peak } else { 0.0 };
            v.gain += (goal - v.gain) * if v.held { att } else { rel };
            if !v.held && v.gain < 1e-4 {
                v.note = None;
                v.follow = false;
                continue;
            }
            let glide = if v.follow {
                self.glide_coef
            } else {
                self.midi_glide_coef
            };
            v.hz += (v.target_hz - v.hz) * glide;
            level += v.gain;
            if s.carrier == Carrier::Noise || s.carrier == Carrier::Sidechain {
                continue;
            }
            let mut x = 0.0;
            let hz_bent = v.hz * bend;
            for (o, ratio) in ratios.iter().enumerate().take(oscs) {
                let hz = hz_bent * ratio;
                let dt = (hz / sr).min(0.5);
                let t = v.phase[o];
                x += match s.carrier {
                    Carrier::Square => {
                        let sq = if t < 0.5 { 1.0 } else { -1.0 };
                        sq + blep(t, dt) - blep(wrap(t + 0.5), dt)
                    }
                    _ => 2.0 * t - 1.0 - blep(t, dt),
                };
                v.phase[o] = wrap(t + dt);
            }
            out += x * norm * v.gain;
        }
        let level = level.min(1.0);
        let n = self.noise();
        let noise = match s.carrier {
            Carrier::Noise => 1.0,
            _ => s.noise,
        };
        (out + n * noise * level, level)
    }

    /// One sample. `input`: the voice now; `dry`: the voice delayed by
    /// the engine latency (for the sibilance, which must line up with
    /// the dry and lead); `sidechain`: the side-chain input now;
    /// `follow`: the tuned sung note (MIDI note number) while voiced.
    /// Returns the vocoded voice, lined up with the engine's output.
    pub fn tick(&mut self, input: f32, dry: f32, sidechain: f32, follow: Option<f64>) -> f32 {
        let s = self.settings;
        let goal = if s.mode == VocoderMode::Off { 0.0 } else { 1.0 };
        if goal == 0.0 && self.level < 1e-5 {
            if self.level != 0.0 {
                self.reset();
            }
            return 0.0;
        }
        self.level += (goal - self.level) * self.attack_coef;
        let x = f64::from(input);
        let (mut carrier, amp) = self.synth(follow);
        if s.carrier == Carrier::Sidechain {
            carrier = f64::from(sidechain);
        }
        let wet = match s.mode {
            VocoderMode::Talkbox => self.talkbox(x, carrier, amp),
            _ => self.vocode(x, carrier, amp),
        };
        // Sibilance: the voice's top, where the voice is unvoiced.
        let voiced = follow.is_some();
        let u = if voiced { 0.15 } else { 1.0 };
        self.unvoiced += (u - self.unvoiced) * self.voicing_coef;
        let d = f64::from(dry);
        let h0 = self.sib_hp[0].tick(d);
        let hp = self.sib_hp[1].tick(h0);
        let sib = hp * s.sibilance * self.unvoiced;
        let wet = self.crush(wet);
        let y = self.out_delay.tick(soft_limit(wet * self.level) as f32);
        #[allow(clippy::cast_possible_truncation)]
        let out = y + (sib * self.level) as f32;
        out
    }

    fn vocode(&mut self, x: f64, carrier: f64, amp: f64) -> f64 {
        let n = self.settings.bands.clamp(2, MAX_BANDS);
        let (att, rel) = (self.band_att, self.band_rel);
        let hold = self.settings.hold;
        let (c_att, c_coef) = (self.carrier_att, self.carrier_rel);
        let mut y = 0.0;
        for (vb, cb) in self.voice_bands[..n]
            .iter_mut()
            .zip(self.carrier_bands[..n].iter_mut())
        {
            let m = vb.tick(x).abs();
            if !hold {
                vb.env += (m - vb.env) * if m > vb.env { att } else { rel };
            }
            let c = cb.tick(carrier);
            // Fast attack: a carrier onset never outruns its envelope
            // (no burst through the division).
            let a = c.abs();
            cb.env += (a - cb.env) * if a > cb.env { c_att } else { c_coef };
            y += c * vb.env / cb.env.max(CARRIER_FLOOR);
        }
        // Keyboard envelopes and a carrier at rest stay audible as such.
        let gate = if self.settings.carrier == Carrier::Sidechain {
            let a = carrier.abs();
            self.carrier_env +=
                (a - self.carrier_env) * if a > self.carrier_env { c_att } else { c_coef };
            (self.carrier_env / CARRIER_FLOOR).min(1.0)
        } else {
            amp
        };
        y * gate * VOCODER_MAKEUP
    }

    fn talkbox(&mut self, x: f64, carrier: f64, amp: f64) -> f64 {
        // Analysis: pre-emphasised voice into the ring, a frame per hop.
        let pre = x - PRE_EMPHASIS * self.prev_in;
        self.prev_in = x;
        let w = self.ring.len();
        self.ring[self.ring_pos] = pre;
        self.ring_pos = (self.ring_pos + 1) % w;
        self.hop_count += 1;
        if self.hop_count >= self.hop {
            self.hop_count = 0;
            if self.settings.hold {
                // Frozen: the last frame stays.
                self.k_step = [0.0; MAX_ORDER + 1];
                self.lpc_gain_step = 0.0;
            } else {
                self.analyse();
            }
        }
        let p = self.order;
        for i in 1..=p {
            self.k[i] += self.k_step[i];
        }
        self.lpc_gain += self.lpc_gain_step;
        // Carrier at unit level, times the voice's residual level.
        let c2 = carrier * carrier;
        let k = if c2 > self.carrier_rms {
            self.carrier_att
        } else {
            self.rms_coef
        };
        self.carrier_rms += (c2 - self.carrier_rms) * k;
        let rms = self.carrier_rms.sqrt();
        let gate = if self.settings.carrier == Carrier::Sidechain {
            (rms / CARRIER_FLOOR).min(1.0)
        } else {
            amp
        };
        let e = carrier / rms.max(CARRIER_FLOOR) * self.lpc_gain.max(0.0) * gate;
        // All-pole lattice.
        let mut f = e;
        for i in (1..=p).rev() {
            f -= self.k[i] * self.lattice[i - 1];
            self.lattice[i] = self.k[i] * f + self.lattice[i - 1];
        }
        self.lattice[0] = f;
        if !f.is_finite() {
            self.lattice = [0.0; MAX_ORDER + 1];
            return 0.0;
        }
        f * TALKBOX_MAKEUP
    }

    /// LPC of the last window (Levinson-Durbin): reflection coefficients
    /// and residual level, reached over the next hop.
    fn analyse(&mut self) {
        // The frame is read at the Band Shift's rate: read faster, the
        // voice's formants come out higher (and slower, lower).
        let n = self.ring.len();
        let w = self.window.len();
        let q = self.talk_shift();
        #[allow(clippy::cast_precision_loss)]
        let newest = (self.ring_pos + n - 1) as f64;
        for i in 0..w {
            #[allow(clippy::cast_precision_loss)]
            let back = (w - 1 - i) as f64 * q;
            let t = newest - back;
            let t = if t < 0.0 { t + n as f64 } else { t };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let a = t.floor() as usize % n;
            let f = t - t.floor();
            let x = self.ring[a] + (self.ring[(a + 1) % n] - self.ring[a]) * f;
            self.frame[i] = x * self.window[i];
        }
        let p = self.order;
        let mut r = [0.0; MAX_ORDER + 1];
        for (lag, r) in r.iter_mut().enumerate().take(p + 1) {
            *r = self.frame[lag..]
                .iter()
                .zip(&self.frame[..w - lag])
                .map(|(a, b)| a * b)
                .sum();
        }
        let mut k = [0.0; MAX_ORDER + 1];
        let mut gain = 0.0;
        if r[0] > 1e-10 {
            r[0] *= 1.0 + 1e-5;
            #[allow(clippy::cast_precision_loss)]
            for (lag, r) in r.iter_mut().enumerate().skip(1).take(p) {
                let a = std::f64::consts::TAU * LAG_WINDOW_HZ * lag as f64 / self.sample_rate;
                *r *= (-0.5 * a * a).exp();
            }
            let mut a = [0.0; MAX_ORDER + 1];
            let mut prev = [0.0; MAX_ORDER + 1];
            let mut err = r[0];
            for i in 1..=p {
                let mut acc = r[i];
                for j in 1..i {
                    acc += a[j] * r[i - j];
                }
                let ki = (-acc / err).clamp(-0.999, 0.999);
                prev[..i].copy_from_slice(&a[..i]);
                for j in 1..i {
                    a[j] = prev[j] + ki * prev[i - j];
                }
                a[i] = ki;
                k[i] = ki;
                err *= 1.0 - ki * ki;
                if err <= 0.0 {
                    break;
                }
            }
            gain = (err.max(0.0) / self.window_power).sqrt();
        }
        #[allow(clippy::cast_precision_loss)]
        let steps = self.hop as f64;
        for ((step, target), now) in self.k_step[1..=p]
            .iter_mut()
            .zip(&k[1..=p])
            .zip(&self.k[1..=p])
        {
            *step = (target - now) / steps;
        }
        self.lpc_gain_step = (gain - self.lpc_gain) / steps;
    }
}

/// Unity below the knee, then a smooth bend that never passes
/// twice the knee.
fn soft_limit(x: f64) -> f64 {
    let a = x.abs();
    if a <= SOFT_KNEE {
        return x;
    }
    let over = a - SOFT_KNEE;
    (SOFT_KNEE + over / (1.0 + over / SOFT_KNEE)).copysign(x)
}

fn note_hz(note: f64) -> f64 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// A buzzy "vowel": 150 Hz pulse train through two resonances.
    fn vowel(n: usize, f1: f64, f2: f64) -> Vec<f32> {
        let mut r1 = Biquad::default();
        let mut r2 = Biquad::default();
        r1.band_pass(SR, f1, 6.0);
        r2.band_pass(SR, f2, 8.0);
        let period = (SR / 150.0) as usize;
        (0..n)
            .map(|i| {
                let p = if i % period == 0 { 1.0 } else { 0.0 };
                (0.6 * r1.tick(p) + 0.4 * r2.tick(p)) as f32
            })
            .collect()
    }

    fn run(v: &mut Vocoder, x: &[f32], follow: Option<f64>) -> Vec<f32> {
        x.iter().map(|&s| v.tick(s, s, 0.0, follow)).collect()
    }

    fn played(mode: VocoderMode) -> Vocoder {
        let mut v = Vocoder::default();
        v.prepare(SR, 2048);
        v.set_latency(480);
        v.settings.mode = mode;
        v.settings.sibilance = 0.0;
        v.apply_settings();
        let mut notes = NoteStack::default();
        notes.note_on(0, 48, 127);
        v.set_notes(&notes);
        v
    }

    #[test]
    fn off_is_silent_and_free() {
        let mut v = Vocoder::default();
        let x = vowel(4800, 700.0, 1200.0);
        assert!(run(&mut v, &x, Some(60.0)).iter().all(|y| *y == 0.0));
    }

    #[test]
    fn both_modes_follow_the_voice_level() {
        for mode in [VocoderMode::Vocoder, VocoderMode::Talkbox] {
            let mut v = played(mode);
            let x = vowel(48_000, 700.0, 1200.0);
            let y = run(&mut v, &x, Some(60.0));
            let (rx, ry) = (rms(&x[24_000..]), rms(&y[24_000..]));
            let db = 20.0 * (ry / rx).log10();
            eprintln!("LEVEL {mode:?}: {db:.1} dB");
            assert!(db.abs() < 9.0, "{mode:?}: {db:.1} dB");
            assert!(y.iter().all(|s| s.is_finite()));
            // Silence in, silence out (no blown-up noise floor).
            let y = run(&mut v, &vec![0.0; 24_000], Some(60.0));
            assert!(rms(&y[12_000..]) < 1e-4, "{mode:?}");
        }
    }

    #[test]
    fn carrier_needs_a_note_or_follow() {
        let mut v = played(VocoderMode::Vocoder);
        v.set_notes(&NoteStack::default());
        v.settings.follow = false;
        v.apply_settings();
        let x = vowel(24_000, 700.0, 1200.0);
        let y = run(&mut v, &x, Some(60.0));
        assert!(rms(&y[12_000..]) < 1e-4);
        v.settings.follow = true;
        v.apply_settings();
        let y = run(&mut v, &x, Some(60.0));
        assert!(rms(&y[12_000..]) > 1e-3);
        assert_ne!(v.sounding() & (1 << 60), 0);
    }

    #[test]
    fn talkbox_keeps_the_vowel() {
        // The output's spectrum peaks near the vowel's formant, not
        // elsewhere: energy around F1 beats energy an octave above.
        let mut v = played(VocoderMode::Talkbox);
        let x = vowel(48_000, 800.0, 800.0);
        let y = run(&mut v, &x, Some(48.0));
        let band = |hz: f64| {
            let mut b = Band::default();
            b.set(SR, hz, 4.0);
            let o: Vec<f32> = y[24_000..]
                .iter()
                .map(|s| b.tick(f64::from(*s)) as f32)
                .collect();
            rms(&o)
        };
        assert!(
            band(800.0) > 2.0 * band(1600.0),
            "{} {}",
            band(800.0),
            band(1600.0)
        );
    }

    #[test]
    fn talkbox_shift_moves_the_formant() {
        let mut v = played(VocoderMode::Talkbox);
        v.settings.shift_st = 12.0;
        v.apply_settings();
        let x = vowel(48_000, 800.0, 800.0);
        let y = run(&mut v, &x, Some(48.0));
        let band = |hz: f64| {
            let mut b = Band::default();
            b.set(SR, hz, 4.0);
            let o: Vec<f32> = y[24_000..]
                .iter()
                .map(|s| b.tick(f64::from(*s)) as f32)
                .collect();
            rms(&o)
        };
        assert!(
            band(1600.0) > 2.0 * band(800.0),
            "{} {}",
            band(1600.0),
            band(800.0)
        );
    }

    #[test]
    fn lofi_steps_the_signal() {
        let mut v = played(VocoderMode::Vocoder);
        v.settings.lofi = 1.0;
        v.apply_settings();
        let x = vowel(24_000, 700.0, 1200.0);
        let y = run(&mut v, &x, Some(60.0));
        // Held for whole steps: many repeated samples in a row.
        let held = y[12_000..].windows(2).filter(|w| w[0] == w[1]).count();
        assert!(held > 6_000, "{held}");
    }

    #[test]
    fn width_is_mono_safe() {
        let mut v = Vocoder::default();
        v.settings.width = 1.0;
        let x = vowel(4800, 700.0, 1200.0);
        let mut side = 0.0f32;
        for s in x {
            let (l, r) = v.widen(s);
            assert!((l + r - 2.0 * s).abs() < 1e-6);
            side = side.max((l - r).abs());
        }
        assert!(side > 1e-3);
    }

    #[test]
    fn polyphony_is_bounded_and_releases() {
        let mut v = played(VocoderMode::Vocoder);
        let mut notes = NoteStack::default();
        for n in 40..52 {
            notes.note_on(0, n, 100);
        }
        v.set_notes(&notes);
        assert_eq!(v.voices.iter().filter(|v| v.held).count(), CARRIER_VOICES);
        v.set_notes(&NoteStack::default());
        v.settings.follow = false;
        v.apply_settings();
        let x = vowel(48_000, 700.0, 1200.0);
        run(&mut v, &x, None);
        assert_eq!(v.sounding(), 0);
    }

    #[test]
    fn follow_chords_and_bend() {
        let mut v = played(VocoderMode::Vocoder);
        v.set_notes(&NoteStack::default());
        v.settings.chord = FollowChord::Major;
        v.apply_settings();
        let x = vowel(4800, 700.0, 1200.0);
        run(&mut v, &x, Some(60.0));
        assert_eq!(v.sounding(), (1 << 60) | (1 << 64) | (1 << 67));
        // A minor scale: the third comes out minor.
        v.settings.chord = FollowChord::Scale;
        v.settings.scale = crate::autotune::scale_mask(9, crate::autotune::Scale::NaturalMinor);
        v.apply_settings();
        run(&mut v, &x, Some(57.0));
        let held: Vec<f64> = v
            .voices
            .iter()
            .filter(|v| v.held)
            .filter_map(|v| v.note)
            .collect();
        assert!(held.contains(&60.0) && held.contains(&64.0), "{held:?}");
        // Bend moves the oscillators, not the notes.
        v.set_bend(1.0);
        run(&mut v, &x, Some(57.0));
        assert!(v.voices.iter().any(|v| v.held && v.note == Some(57.0)));
    }

    #[test]
    fn hold_keeps_the_timbre() {
        for mode in [VocoderMode::Vocoder, VocoderMode::Talkbox] {
            let mut v = played(mode);
            let x = vowel(24_000, 700.0, 1200.0);
            run(&mut v, &x, Some(60.0));
            v.settings.hold = true;
            v.apply_settings();
            // The singer stops: the held note still speaks the vowel.
            let y = run(&mut v, &vec![0.0; 24_000], None);
            assert!(rms(&y[12_000..]) > 1e-3, "{mode:?}");
        }
    }

    #[test]
    fn output_is_lined_up_with_the_latency() {
        let mut v = played(VocoderMode::Vocoder);
        assert_eq!(v.out_delay.delay(), 480 - v.analysis_lag());
        v.settings.mode = VocoderMode::Talkbox;
        v.apply_settings();
        assert_eq!(v.out_delay.delay(), 480 - 480);
    }
}
