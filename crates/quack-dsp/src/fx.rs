//! Built-in voice effects, with no latency added (no look-ahead).
//!
//! - [`Dynamics`], at the input, before everything else: gate, de-esser
//!   and compressor. Every layer (lead, harmonies, vocoder modulator)
//!   then gets a clean and even voice, and the pitch detector and the
//!   vocoder's envelopes a steadier level.
//! - [`Space`], on sends: each layer has its own send to a shared bus
//!   with a tempo-synced delay and a reverb. *Duck* lowers that bus
//!   while the voice sings and lets the tails come back in the gaps.

#![allow(clippy::needless_range_loop)]

use std::f32::consts::PI;

/// Topology-preserving state-variable filter (one channel).
/// `lp + k·bp + hp` gives the input back exactly.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    g: f32,
    k: f32,
    a1: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    fn set(&mut self, sample_rate: f32, hz: f32, q: f32) {
        let hz = hz.clamp(10.0, 0.45 * sample_rate);
        self.g = (PI * hz / sample_rate).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
    }

    fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Returns (low, band, high).
    #[inline]
    fn tick(&mut self, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2;
        let v1 = self.a1 * (self.ic1 + self.g * v3);
        let v2 = self.ic2 + self.g * v1;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let hp = x - self.k * v1 - v2;
        (v2, v1, hp)
    }
}

/// One-pole coefficient for a time constant in milliseconds.
fn coef(sample_rate: f32, ms: f32) -> f32 {
    if ms <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (sample_rate * ms / 1000.0)).exp()
    }
}

/// Peak envelope with separate attack and release.
#[derive(Debug, Clone, Copy, Default)]
struct Follower {
    att: f32,
    rel: f32,
    env: f32,
}

impl Follower {
    fn set(&mut self, sample_rate: f32, attack_ms: f32, release_ms: f32) {
        self.att = coef(sample_rate, attack_ms);
        self.rel = coef(sample_rate, release_ms);
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let a = x.abs();
        let c = if a > self.env { self.att } else { self.rel };
        self.env += (a - self.env) * c;
        self.env
    }
}

#[inline]
fn to_db(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}

#[inline]
fn from_db(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Settings of the input dynamics (plain values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicsSettings {
    /// Gate threshold in dB; at or below [`GATE_OFF_DB`] the gate is off.
    pub gate_db: f32,
    /// De-esser amount, 0-1 (0 = off), and its frequency in Hz.
    pub deess: f32,
    pub deess_hz: f32,
    /// Compressor: threshold (dB), ratio (1 = off), attack and release
    /// (ms), make-up gain (dB).
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
}

/// Gate threshold that means "off".
pub const GATE_OFF_DB: f32 = -80.0;

impl Default for DynamicsSettings {
    fn default() -> Self {
        Self {
            gate_db: GATE_OFF_DB,
            deess: 0.0,
            deess_hz: 6500.0,
            threshold_db: -18.0,
            ratio: 1.0,
            attack_ms: 5.0,
            release_ms: 120.0,
            makeup_db: 0.0,
        }
    }
}

/// Gate, de-esser and compressor, stereo-linked. Real-time safe.
#[derive(Debug, Clone, Default)]
pub struct Dynamics {
    sample_rate: f32,
    settings: DynamicsSettings,
    // Gate.
    gate_env: Follower,
    gate_gain: f32,
    gate_open: bool,
    gate_att: f32,
    gate_rel: f32,
    // De-esser: detector on the mono sum, then the top of each channel.
    ds_detect: Svf,
    ds_split: [Svf; 2],
    ds_hp_env: Follower,
    ds_full_env: Follower,
    ds_gain: f32,
    ds_att: f32,
    ds_rel: f32,
    // Compressor: level (dB) smoothed into gain reduction.
    comp_env: Follower,
    comp_gr: f32,
    comp_att: f32,
    comp_rel: f32,
    makeup: f32,
    // Meters (dB of reduction, ≤ 0).
    meter_comp: f32,
    meter_deess: f32,
}

impl Dynamics {
    pub fn prepare(&mut self, sample_rate: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        self.gate_env.set(sr, 5.0, 5.0);
        self.gate_att = coef(sr, 1.0);
        self.gate_rel = coef(sr, 80.0);
        self.ds_hp_env.set(sr, 0.3, 30.0);
        self.ds_full_env.set(sr, 0.3, 30.0);
        self.ds_att = coef(sr, 0.5);
        self.ds_rel = coef(sr, 40.0);
        self.comp_env.set(sr, 0.2, 20.0);
        let s = self.settings;
        self.set(s);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.gate_env.env = 0.0;
        self.gate_gain = if self.gate_on() { 0.0 } else { 1.0 };
        self.gate_open = false;
        self.ds_detect.reset();
        for f in &mut self.ds_split {
            f.reset();
        }
        self.ds_hp_env.env = 0.0;
        self.ds_full_env.env = 0.0;
        self.ds_gain = 1.0;
        self.comp_env.env = 0.0;
        self.comp_gr = 0.0;
        self.meter_comp = 0.0;
        self.meter_deess = 0.0;
    }

    pub fn set(&mut self, s: DynamicsSettings) {
        self.settings = s;
        let sr = self.sample_rate.max(1.0);
        self.ds_detect.set(sr, s.deess_hz, 0.707);
        for f in &mut self.ds_split {
            f.set(sr, s.deess_hz * 0.6, 0.707);
        }
        self.comp_att = coef(sr, s.attack_ms.max(0.05));
        self.comp_rel = coef(sr, s.release_ms.max(1.0));
        self.makeup = from_db(s.makeup_db);
    }

    #[must_use]
    pub fn settings(&self) -> DynamicsSettings {
        self.settings
    }

    fn gate_on(&self) -> bool {
        self.settings.gate_db > GATE_OFF_DB + 0.01
    }

    /// Gain reduction of the compressor and the de-esser, in dB (≤ 0).
    #[must_use]
    pub fn reduction_db(&self) -> (f32, f32) {
        (self.meter_comp, self.meter_deess)
    }

    /// One stereo sample.
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        let s = self.settings;
        let (mut l, mut r) = (l, r);
        let mono = 0.5 * (l + r);

        // Gate, with 6 dB of hysteresis.
        if self.gate_on() {
            // Mean square over ~5 ms: noise peaks do not open it.
            let env = 0.5 * to_db(self.gate_env.tick(mono * mono));
            if env > s.gate_db {
                self.gate_open = true;
            } else if env < s.gate_db - 6.0 {
                self.gate_open = false;
            }
            let (goal, c) = if self.gate_open {
                (1.0, self.gate_att)
            } else {
                (0.0, self.gate_rel)
            };
            self.gate_gain += (goal - self.gate_gain) * c;
            l *= self.gate_gain;
            r *= self.gate_gain;
        } else {
            self.gate_gain = 1.0;
        }

        // De-esser: when the top is close to the whole voice in level,
        // it is a sibilant; its top is lowered, the rest left alone.
        if s.deess > 0.0 {
            let m = 0.5 * (l + r);
            let (_, _, hp) = self.ds_detect.tick(m);
            let hp_db = to_db(self.ds_hp_env.tick(hp));
            let full_db = to_db(self.ds_full_env.tick(m));
            let excess = (hp_db - full_db + 4.0 + 8.0 * s.deess).max(0.0);
            let gr_db = -(excess * 1.5 * s.deess).min(24.0 * s.deess);
            let goal = from_db(gr_db);
            let c = if goal < self.ds_gain {
                self.ds_att
            } else {
                self.ds_rel
            };
            self.ds_gain += (goal - self.ds_gain) * c;
            let cut = 1.0 - self.ds_gain;
            // The top band is what the low-pass leaves (complementary, so
            // the voice comes back whole when nothing is cut).
            let (ll, _, _) = self.ds_split[0].tick(l);
            let (lr, _, _) = self.ds_split[1].tick(r);
            l -= (l - ll) * cut;
            r -= (r - lr) * cut;
            self.meter_deess = to_db(self.ds_gain);
        } else {
            self.ds_gain = 1.0;
            self.meter_deess = 0.0;
        }

        // Compressor: feed-forward, 6 dB soft knee, smoothed in dB.
        if s.ratio > 1.001 {
            let lvl = to_db(self.comp_env.tick(0.5 * (l + r)));
            let over = lvl - s.threshold_db;
            let knee = 6.0;
            let slope = 1.0 - 1.0 / s.ratio;
            let goal = if over <= -knee / 2.0 {
                0.0
            } else if over >= knee / 2.0 {
                -over * slope
            } else {
                let o = over + knee / 2.0;
                -slope * o * o / (2.0 * knee)
            };
            let c = if goal < self.comp_gr {
                self.comp_att
            } else {
                self.comp_rel
            };
            self.comp_gr += (goal - self.comp_gr) * c;
            let g = from_db(self.comp_gr) * self.makeup;
            l *= g;
            r *= g;
            self.meter_comp = self.comp_gr;
        } else {
            self.comp_gr = 0.0;
            self.meter_comp = 0.0;
            l *= self.makeup;
            r *= self.makeup;
        }
        (l, r)
    }
}

/// Delay times, as note values on the host's tempo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Division {
    Sixteenth,
    EighthTriplet,
    Eighth,
    QuarterTriplet,
    #[default]
    DottedEighth,
    Quarter,
    HalfTriplet,
    DottedQuarter,
    Half,
    Bar,
}

impl Division {
    pub const COUNT: u32 = 10;
    pub const ALL: [Self; 10] = [
        Self::Sixteenth,
        Self::EighthTriplet,
        Self::Eighth,
        Self::QuarterTriplet,
        Self::DottedEighth,
        Self::Quarter,
        Self::HalfTriplet,
        Self::DottedQuarter,
        Self::Half,
        Self::Bar,
    ];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = i.round().clamp(0.0, f64::from(Self::COUNT - 1)) as usize;
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sixteenth => "1/16",
            Self::EighthTriplet => "1/8T",
            Self::Eighth => "1/8",
            Self::QuarterTriplet => "1/4T",
            Self::DottedEighth => "1/8D",
            Self::Quarter => "1/4",
            Self::HalfTriplet => "1/2T",
            Self::DottedQuarter => "1/4D",
            Self::Half => "1/2",
            Self::Bar => "1 Bar",
        }
    }

    /// Length in beats (quarter notes), `bar` beats for a bar.
    #[must_use]
    pub fn beats(self, bar: f64) -> f64 {
        match self {
            Self::Sixteenth => 0.25,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::Eighth => 0.5,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::DottedEighth => 0.75,
            Self::Quarter => 1.0,
            Self::HalfTriplet => 4.0 / 3.0,
            Self::DottedQuarter => 1.5,
            Self::Half => 2.0,
            Self::Bar => bar,
        }
    }
}

/// Settings of the space bus (plain values, levels 0-1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpaceSettings {
    pub sync: bool,
    pub division: Division,
    pub delay_ms: f32,
    pub feedback: f32,
    pub ping_pong: bool,
    /// Darkening of the repeats, 0-1.
    pub delay_tone: f32,
    pub delay_level: f32,
    /// Reverb decay (RT60, s), damping 0-1, pre-delay (ms), level.
    pub decay_s: f32,
    pub damping: f32,
    pub predelay_ms: f32,
    pub reverb_level: f32,
    /// How much the singing voice lowers the bus, 0-1.
    pub duck: f32,
    /// Tape-like wobble of the delay time, 0-1.
    pub delay_mod: f32,
    /// Right side later than the left (not in ping-pong), 0-1.
    pub delay_width: f32,
    /// Low cut of the repeats, Hz.
    pub delay_low_cut_hz: f32,
    /// Room size 0-1, line modulation 0-1, low cut of the reverb input
    /// (Hz), stereo width 0-1.
    pub reverb_size: f32,
    pub reverb_mod: f32,
    pub reverb_low_cut_hz: f32,
    pub reverb_width: f32,
    /// How much of the delay feeds the reverb, 0-1.
    pub delay_to_reverb: f32,
}

impl Default for SpaceSettings {
    fn default() -> Self {
        Self {
            sync: true,
            division: Division::DottedEighth,
            delay_ms: 375.0,
            feedback: 0.35,
            ping_pong: false,
            delay_tone: 0.3,
            delay_level: 0.0,
            decay_s: 2.0,
            damping: 0.5,
            predelay_ms: 20.0,
            reverb_level: 0.6,
            duck: 0.0,
            delay_mod: 0.0,
            delay_width: 0.0,
            delay_low_cut_hz: 150.0,
            reverb_size: 0.5,
            reverb_mod: 0.2,
            reverb_low_cut_hz: 100.0,
            reverb_width: 1.0,
            delay_to_reverb: 0.0,
        }
    }
}

/// Longest delay: a 4/4 bar at 60 BPM.
const MAX_DELAY_S: f64 = 4.0;
const MAX_PREDELAY_S: f64 = 0.25;
const FDN: usize = 8;
/// Reverb line lengths (ms), mutually prime-ish.
const FDN_MS: [f32; FDN] = [29.7, 37.1, 41.1, 43.7, 53.0, 59.9, 67.3, 73.1];
const DIFFUSER_MS: [f32; 4] = [4.7, 3.6, 12.7, 9.3];
/// Reverb line length scale from the size (0-1): 0.6 to 2.2.
fn size_scale(size: f32) -> f32 {
    0.6 + 1.6 * size.clamp(0.0, 1.0)
}
const SIZE_MAX: f32 = 2.2;
/// Deepest line modulation, ms.
const REVERB_MOD_MS: f32 = 0.6;
/// Deepest delay wobble (wow, flutter), ms.
const WOW_MS: f32 = 4.0;
const FLUTTER_MS: f32 = 0.35;
/// Widest right-side offset of the delay, as a fraction of its time.
const DELAY_WIDTH: f32 = 0.12;

/// A fixed-length circular buffer.
#[derive(Debug, Clone, Default)]
struct Ring {
    buf: Vec<f32>,
    pos: usize,
}

impl Ring {
    fn prepare(&mut self, len: usize) {
        self.buf = vec![0.0; len.max(2)];
        self.pos = 0;
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        self.buf[self.pos] = x;
    }

    /// The sample pushed `d` pushes ago (0 = the last one).
    #[inline]
    fn read(&self, d: usize) -> f32 {
        let n = self.buf.len();
        let d = d.min(n - 1);
        self.buf[(self.pos + n - d) % n]
    }

    /// Fractional read, before the next push: the input of `d` samples
    /// ago (`d` ≥ 1), linear interpolation.
    #[inline]
    fn read_frac(&self, d: f32) -> f32 {
        let d = d.max(1.0) - 1.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = d as usize;
        #[allow(clippy::cast_precision_loss)]
        let f = d - i as f32;
        let a = self.read(i);
        let b = self.read(i + 1);
        a + (b - a) * f
    }
}

/// Schroeder all-pass for the reverb's input diffusion.
#[derive(Debug, Clone, Default)]
struct AllPass {
    ring: Ring,
    len: usize,
}

impl AllPass {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        const G: f32 = 0.6;
        let d = self.ring.read(self.len - 1);
        let v = x + G * d;
        self.ring.push(v);
        d - G * v
    }
}

/// Tempo-synced stereo delay and an 8-line feedback-delay-network
/// reverb on one bus, with ducking. Real-time safe.
#[derive(Debug, Clone, Default)]
pub struct Space {
    sample_rate: f32,
    settings: SpaceSettings,
    tempo: f64,
    bar_beats: f64,
    // Delay.
    delay: [Ring; 2],
    delay_len: f32,
    delay_goal: f32,
    delay_glide: f32,
    tone: [f32; 2],
    tone_coef: f32,
    low_cut: [f32; 2],
    low_cut_coef: f32,
    wow: f32,
    flutter: f32,
    wow_inc: f32,
    flutter_inc: f32,
    // Reverb.
    predelay: Ring,
    predelay_len: usize,
    diffuser: [AllPass; 4],
    lines: [Ring; FDN],
    lens: [usize; FDN],
    gains: [f32; FDN],
    damp: [f32; FDN],
    damp_coef: f32,
    base_lens: [f32; FDN],
    rv_low: f32,
    rv_low_coef: f32,
    lfo: f32,
    lfo_inc: f32,
    // Ducking.
    duck_env: Follower,
}

impl Space {
    pub fn prepare(&mut self, sample_rate: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        if self.tempo <= 0.0 {
            self.tempo = 120.0;
            self.bar_beats = 4.0;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max = (sample_rate * MAX_DELAY_S) as usize + 4;
        for d in &mut self.delay {
            d.prepare(max);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        self.predelay
            .prepare((sample_rate * MAX_PREDELAY_S) as usize + 2);
        for (a, ms) in self.diffuser.iter_mut().zip(DIFFUSER_MS) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let len = (sr * ms / 1000.0) as usize + 1;
            a.ring.prepare(len + 1);
            a.len = len;
        }
        let margin = sr * REVERB_MOD_MS / 1000.0 + 4.0;
        for (i, ms) in FDN_MS.iter().enumerate() {
            self.base_lens[i] = sr * ms / 1000.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let max = (self.base_lens[i] * SIZE_MAX + margin) as usize + 2;
            self.lines[i].prepare(max);
        }
        self.wow_inc = 2.0 * PI * 0.55 / sr;
        self.flutter_inc = 2.0 * PI * 5.3 / sr;
        self.lfo_inc = 2.0 * PI * 0.7 / sr;
        self.delay_glide = coef(sr, 60.0);
        self.duck_env.set(sr, 10.0, 300.0);
        let s = self.settings;
        self.set(s);
        self.delay_len = self.delay_goal;
        self.reset();
    }

    pub fn reset(&mut self) {
        for d in &mut self.delay {
            d.reset();
        }
        self.predelay.reset();
        for a in &mut self.diffuser {
            a.ring.reset();
        }
        for l in &mut self.lines {
            l.reset();
        }
        self.tone = [0.0; 2];
        self.low_cut = [0.0; 2];
        self.damp = [0.0; FDN];
        self.rv_low = 0.0;
        self.duck_env.env = 0.0;
        self.delay_len = self.delay_goal;
    }

    pub fn set(&mut self, s: SpaceSettings) {
        self.settings = s;
        let sr = self.sample_rate.max(1.0);
        // Tone: the repeats' low-pass, 12 kHz down to 1.5 kHz.
        let hz = 12_000.0 * (1.5_f32 / 12.0).powf(s.delay_tone.clamp(0.0, 1.0));
        self.tone_coef = coef(sr, 1000.0 / (2.0 * PI * hz));
        // Damping: 16 kHz down to 2 kHz in the reverb lines.
        let hz = 16_000.0 * (2.0_f32 / 16.0).powf(s.damping.clamp(0.0, 1.0));
        self.damp_coef = coef(sr, 1000.0 / (2.0 * PI * hz));
        self.low_cut_coef = coef(sr, 1000.0 / (2.0 * PI * s.delay_low_cut_hz.max(10.0)));
        self.rv_low_coef = coef(sr, 1000.0 / (2.0 * PI * s.reverb_low_cut_hz.max(10.0)));
        let scale = size_scale(s.reverb_size);
        for i in 0..FDN {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                self.lens[i] = (self.base_lens[i] * scale) as usize + 1;
            }
        }
        let t60 = s.decay_s.max(0.1);
        for i in 0..FDN {
            #[allow(clippy::cast_precision_loss)]
            let len_s = self.lens[i] as f32 / sr;
            self.gains[i] = 10.0_f32.powf(-3.0 * len_s / t60);
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            self.predelay_len = ((s.predelay_ms / 1000.0 * sr) as usize).max(1);
        }
        self.update_delay_goal();
    }

    #[must_use]
    pub fn settings(&self) -> SpaceSettings {
        self.settings
    }

    /// Host tempo and bar length in beats (the synced delay follows).
    pub fn set_transport(&mut self, tempo: f64, bar_beats: f64) {
        let mut changed = false;
        if tempo.is_finite() && tempo > 0.0 {
            let t = tempo.clamp(20.0, 400.0);
            changed |= (t - self.tempo).abs() > 1e-6;
            self.tempo = t;
        }
        if bar_beats.is_finite() && bar_beats > 0.0 {
            let b = bar_beats.clamp(1.0, 16.0);
            changed |= (b - self.bar_beats).abs() > 1e-6;
            self.bar_beats = b;
        }
        if changed {
            self.update_delay_goal();
        }
    }

    /// Delay time in seconds, from the tempo or the free time.
    #[must_use]
    pub fn delay_seconds(&self) -> f64 {
        let s = self.settings;
        let t = if s.sync {
            s.division.beats(self.bar_beats) * 60.0 / self.tempo.max(1.0)
        } else {
            f64::from(s.delay_ms) / 1000.0
        };
        t.clamp(0.001, MAX_DELAY_S)
    }

    fn update_delay_goal(&mut self) {
        #[allow(clippy::cast_possible_truncation)]
        let d = (self.delay_seconds() * f64::from(self.sample_rate)) as f32;
        self.delay_goal = d.max(1.0);
    }

    /// One stereo sample: the bus input (sum of the sends) and the key
    /// (the voice, for the ducking). Returns delay plus reverb.
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32, key: f32) -> (f32, f32) {
        self.tick_sends((l, r), (l, r), key)
    }

    /// One stereo sample with the delay's and the reverb's own inputs.
    #[inline]
    pub fn tick_sends(&mut self, delay: (f32, f32), reverb: (f32, f32), key: f32) -> (f32, f32) {
        let s = self.settings;
        let (l, r) = delay;
        let mut out = (0.0, 0.0);
        let mut fed = 0.0;

        if s.delay_level > 0.0 {
            // The time glides to a new value (tape-like), no click.
            self.delay_len += (self.delay_goal - self.delay_len) * self.delay_glide;
            let mut len = self.delay_len;
            if s.delay_mod > 0.0 {
                self.wow = (self.wow + self.wow_inc) % (2.0 * PI);
                self.flutter = (self.flutter + self.flutter_inc) % (2.0 * PI);
                let ms = WOW_MS * (1.0 + self.wow.sin()) + FLUTTER_MS * self.flutter.sin();
                len += s.delay_mod * ms * self.sample_rate / 1000.0;
            }
            let len_r = if s.ping_pong {
                len
            } else {
                len * (1.0 + DELAY_WIDTH * s.delay_width)
            };
            let dl = self.delay[0].read_frac(len);
            let dr = self.delay[1].read_frac(len_r);
            let fb = s.feedback.clamp(0.0, 0.95);
            let (il, ir) = if s.ping_pong {
                // The input enters left; the repeats cross sides.
                (0.5 * (l + r) + dr * fb, dl * fb)
            } else {
                (l + dl * fb, r + dr * fb)
            };
            let mut push = [il, ir];
            for c in 0..2 {
                self.tone[c] += (push[c] - self.tone[c]) * self.tone_coef;
                self.low_cut[c] += (self.tone[c] - self.low_cut[c]) * self.low_cut_coef;
                // Soft limit, so a high feedback never runs away.
                push[c] = soft_clip(self.tone[c] - self.low_cut[c]);
            }
            self.delay[0].push(push[0]);
            self.delay[1].push(push[1]);
            out.0 += dl * s.delay_level;
            out.1 += dr * s.delay_level;
            fed = 0.5 * (dl + dr) * s.delay_level * s.delay_to_reverb;
        }

        if s.reverb_level > 0.0 {
            self.predelay.push(0.5 * (reverb.0 + reverb.1) + fed);
            let x = self.predelay.read(self.predelay_len);
            // Low cut on the way in: no boom in the tail.
            self.rv_low += (x - self.rv_low) * self.rv_low_coef;
            let mut x = x - self.rv_low;
            for a in &mut self.diffuser {
                x = a.tick(x);
            }
            let mut taps = [0.0_f32; FDN];
            let mut sum = 0.0;
            // Slow line modulation breaks the metallic ringing.
            let depth = s.reverb_mod * REVERB_MOD_MS * self.sample_rate / 1000.0;
            let (sn, cs) = if depth > 0.0 {
                self.lfo = (self.lfo + self.lfo_inc) % (2.0 * PI);
                (self.lfo.sin() * depth, self.lfo.cos() * depth)
            } else {
                (0.0, 0.0)
            };
            for i in 0..FDN {
                #[allow(clippy::cast_precision_loss)]
                let base = self.lens[i] as f32;
                let m = match i % 4 {
                    0 => sn,
                    1 => cs,
                    2 => -sn,
                    _ => -cs,
                };
                let y = if depth > 0.0 {
                    self.lines[i].read_frac(base + m + depth)
                } else {
                    self.lines[i].read(self.lens[i] - 1)
                };
                self.damp[i] += (y - self.damp[i]) * self.damp_coef;
                taps[i] = self.damp[i] * self.gains[i];
                sum += taps[i];
            }
            // Householder mix: lossless, every line feeds every other.
            #[allow(clippy::cast_precision_loss)]
            let h = 2.0 / FDN as f32 * sum;
            let mut wl = 0.0;
            let mut wr = 0.0;
            for i in 0..FDN {
                self.lines[i].push(taps[i] - h + x);
                if i % 2 == 0 {
                    wl += taps[i];
                } else {
                    wr += taps[i];
                }
            }
            let g = 0.5 * s.reverb_level;
            let (mid, side) = (0.5 * (wl + wr), 0.5 * (wl - wr) * s.reverb_width);
            out.0 += (mid + side) * g;
            out.1 += (mid - side) * g;
        }

        if s.duck > 0.0 {
            let env = self.duck_env.tick(key);
            let g = 1.0 / (1.0 + s.duck * 12.0 * env);
            out = (out.0 * g, out.1 * g);
        }
        out
    }
}

/// Settings of the tone stage (plain values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneSettings {
    /// Low cut, Hz (at [`LOW_CUT_OFF_HZ`] or below: off).
    pub low_cut_hz: f32,
    /// Presence (bell at 3.5 kHz) and air (shelf from 10 kHz), dB.
    pub presence_db: f32,
    pub air_db: f32,
}

/// Low cut setting that means "off".
pub const LOW_CUT_OFF_HZ: f32 = 20.0;

impl Default for ToneSettings {
    fn default() -> Self {
        Self {
            low_cut_hz: LOW_CUT_OFF_HZ,
            presence_db: 0.0,
            air_db: 0.0,
        }
    }
}

/// Input tone: low cut (before the dynamics), then presence and air
/// (after them). No latency. Real-time safe.
#[derive(Debug, Clone, Default)]
pub struct Tone {
    sample_rate: f32,
    settings: ToneSettings,
    low: [Svf; 2],
    presence: [Svf; 2],
    air: [Svf; 2],
    presence_gain: f32,
    air_gain: f32,
}

impl Tone {
    pub fn prepare(&mut self, sample_rate: f64) {
        #[allow(clippy::cast_possible_truncation)]
        {
            self.sample_rate = sample_rate as f32;
        }
        let s = self.settings;
        self.set(s);
        self.reset();
    }

    pub fn reset(&mut self) {
        for f in self
            .low
            .iter_mut()
            .chain(self.presence.iter_mut())
            .chain(self.air.iter_mut())
        {
            f.reset();
        }
    }

    pub fn set(&mut self, s: ToneSettings) {
        self.settings = s;
        let sr = self.sample_rate.max(1.0);
        for f in &mut self.low {
            f.set(sr, s.low_cut_hz, 0.707);
        }
        for f in &mut self.presence {
            f.set(sr, 3500.0, 0.8);
        }
        for f in &mut self.air {
            f.set(sr, 10_000.0, 0.6);
        }
        self.presence_gain = from_db(s.presence_db) - 1.0;
        self.air_gain = from_db(s.air_db) - 1.0;
    }

    /// The low cut, before the dynamics.
    #[inline]
    pub fn pre(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.settings.low_cut_hz <= LOW_CUT_OFF_HZ + 0.01 {
            return (l, r);
        }
        (self.low[0].tick(l).2, self.low[1].tick(r).2)
    }

    /// Presence and air, after the dynamics.
    #[inline]
    pub fn post(&mut self, l: f32, r: f32) -> (f32, f32) {
        let mut io = [l, r];
        for (c, x) in io.iter_mut().enumerate() {
            if self.presence_gain != 0.0 {
                let (_, bp, _) = self.presence[c].tick(*x);
                *x += self.presence_gain * self.presence[c].k * bp;
            }
            if self.air_gain != 0.0 {
                let (_, _, hp) = self.air[c].tick(*x);
                *x += self.air_gain * hp;
            }
        }
        (io[0], io[1])
    }
}

/// Settings of the chorus (plain values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChorusSettings {
    pub rate_hz: f32,
    /// Depth and mix, 0-1.
    pub depth: f32,
    pub mix: f32,
}

impl Default for ChorusSettings {
    fn default() -> Self {
        Self {
            rate_hz: 0.8,
            depth: 0.4,
            mix: 0.0,
        }
    }
}

const CHORUS_BASE_MS: f32 = 11.0;
const CHORUS_DEPTH_MS: f32 = 6.0;

/// Stereo chorus, two voices per side. The dry sound is not delayed:
/// no latency. Real-time safe.
#[derive(Debug, Clone, Default)]
pub struct Chorus {
    sample_rate: f32,
    settings: ChorusSettings,
    ring: [Ring; 2],
    phase: f32,
    inc: f32,
}

impl Chorus {
    pub fn prepare(&mut self, sample_rate: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let len = (sr * (CHORUS_BASE_MS + CHORUS_DEPTH_MS + 2.0) / 1000.0) as usize + 4;
        for r in &mut self.ring {
            r.prepare(len);
        }
        let s = self.settings;
        self.set(s);
    }

    pub fn reset(&mut self) {
        for r in &mut self.ring {
            r.reset();
        }
    }

    pub fn set(&mut self, s: ChorusSettings) {
        self.settings = s;
        self.inc = 2.0 * PI * s.rate_hz.clamp(0.05, 10.0) / self.sample_rate.max(1.0);
    }

    /// The chorus on the whole of `(l, r)`, at its mix.
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        let m = self.settings.mix;
        self.tick_send(l, r, l * m, r * m)
    }

    /// The layers' sum `(l, r)` with the chorus on the part of it each
    /// layer sends (`(sl, sr)`, its amount times the layer): what is
    /// sent is partly replaced by its chorused copy.
    #[inline]
    pub fn tick_send(&mut self, l: f32, r: f32, sl: f32, sr: f32) -> (f32, f32) {
        let s = self.settings;
        if s.mix <= 0.0 {
            return (l, r);
        }
        self.phase = (self.phase + self.inc) % (2.0 * PI);
        let ms = self.sample_rate / 1000.0;
        let base = CHORUS_BASE_MS * ms;
        let depth = 0.5 * CHORUS_DEPTH_MS * ms * s.depth;
        let (sn, cs) = (self.phase.sin(), self.phase.cos());
        // Voices in quadrature, mirrored between the sides.
        let taps = [[sn, -cs], [cs, -sn]];
        let mut out = [l, r];
        for (c, (x, send)) in [(l, sl), (r, sr)].into_iter().enumerate() {
            let wet = 0.5
                * (self.ring[c].read_frac(base + depth * (1.0 + taps[c][0]))
                    + self.ring[c].read_frac(base * 1.3 + depth * (1.0 + taps[c][1])));
            self.ring[c].push(send);
            out[c] = x - 0.4 * send + 0.8 * wet;
        }
        (out[0], out[1])
    }
}

/// Distortion styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DriveStyle {
    /// Warm and asymmetric, like a valve.
    #[default]
    Tube,
    /// Soft, round compression of the peaks.
    Tape,
    /// Hard clipping.
    Hard,
    /// Square-edged, buzzing.
    Fuzz,
    /// Fewer bits and a lower rate.
    LoFi,
}

impl DriveStyle {
    pub const COUNT: u32 = 5;
    pub const ALL: [Self; 5] = [Self::Tube, Self::Tape, Self::Hard, Self::Fuzz, Self::LoFi];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = i.round().clamp(0.0, f64::from(Self::COUNT - 1)) as usize;
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tube => "Tube",
            Self::Tape => "Tape",
            Self::Hard => "Hard",
            Self::Fuzz => "Fuzz",
            Self::LoFi => "Lo-Fi",
        }
    }
}

/// Settings of the distortion (plain values).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DriveSettings {
    pub style: DriveStyle,
    /// Drive 0-1 (0 to +36 dB into the shaper), tone 0-1 (dark to bright).
    pub drive: f32,
    pub tone: f32,
}

/// Distortion of one layer, mixed in by the layer's amount. No
/// latency. Real-time safe.
#[derive(Debug, Clone, Default)]
pub struct Distortion {
    sample_rate: f32,
    settings: DriveSettings,
    gain: f32,
    makeup: f32,
    tone: [f32; 2],
    tone_coef: f32,
    dc: [f32; 2],
    dc_x: [f32; 2],
    dc_coef: f32,
    held: [f32; 2],
    count: f32,
}

impl Distortion {
    pub fn prepare(&mut self, sample_rate: f64) {
        #[allow(clippy::cast_possible_truncation)]
        {
            self.sample_rate = sample_rate as f32;
        }
        self.dc_coef = 1.0 - 2.0 * PI * 20.0 / self.sample_rate.max(1.0);
        let s = self.settings;
        self.set(s);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.tone = [0.0; 2];
        self.dc = [0.0; 2];
        self.dc_x = [0.0; 2];
        self.held = [0.0; 2];
        self.count = 0.0;
    }

    pub fn set(&mut self, s: DriveSettings) {
        self.settings = s;
        let d = s.drive.clamp(0.0, 1.0);
        self.gain = from_db(36.0 * d);
        // Quiet parts stay near their level, loud ones are squashed.
        self.makeup = self.gain.powf(-0.7);
        let hz = 2000.0 * 9.0_f32.powf(s.tone.clamp(0.0, 1.0));
        self.tone_coef = coef(self.sample_rate.max(1.0), 1000.0 / (2.0 * PI * hz));
    }

    #[inline]
    fn shape(&self, x: f32) -> f32 {
        let g = x * self.gain;
        match self.settings.style {
            DriveStyle::Tube => {
                // Softer on one side: even harmonics.
                if g >= 0.0 {
                    g.tanh()
                } else {
                    (0.7 * g).tanh() / 0.7
                }
            }
            DriveStyle::Tape => g / (1.0 + g.abs()),
            DriveStyle::Hard => g.clamp(-1.0, 1.0),
            DriveStyle::Fuzz => (3.0 * g).tanh().clamp(-0.8, 0.8) / 0.8,
            DriveStyle::LoFi => {
                let bits = 16.0 - 12.0 * self.settings.drive;
                let q = 2.0_f32.powf(bits - 1.0);
                ((x * q).round() / q).clamp(-1.0, 1.0) / self.makeup.max(1e-3)
            }
        }
    }

    /// One stereo sample, `amount` (0-1) of it distorted.
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32, amount: f32) -> (f32, f32) {
        if amount <= 0.0 {
            return (l, r);
        }
        let lofi = self.settings.style == DriveStyle::LoFi;
        // Lo-fi: the rate falls with the drive (hold 1 to 16 samples).
        let step = if lofi {
            1.0 / (1.0 + 15.0 * self.settings.drive)
        } else {
            1.0
        };
        self.count += step;
        let take = self.count >= 1.0;
        if take {
            self.count -= 1.0;
        }
        let mut out = [l, r];
        for (c, x) in [l, r].into_iter().enumerate() {
            let y = if take || !lofi {
                let y = self.shape(x) * self.makeup;
                self.held[c] = y;
                y
            } else {
                self.held[c]
            };
            // Tone, then a DC blocker (the tube is asymmetric).
            self.tone[c] += (y - self.tone[c]) * self.tone_coef;
            let t = self.tone[c];
            let d = t - self.dc_x[c] + self.dc_coef * self.dc[c];
            self.dc_x[c] = t;
            self.dc[c] = d;
            out[c] = x + (d - x) * amount;
        }
        (out[0], out[1])
    }
}

/// Settings of the output dynamics (plain values, 0-1 amounts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputSettings {
    pub deess: f32,
    /// One-knob compressor: threshold, ratio and make-up together.
    pub comp: f32,
    /// Limiter ceiling, dB.
    pub ceiling_db: f32,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            deess: 0.0,
            comp: 0.0,
            ceiling_db: 0.0,
        }
    }
}

/// Output dynamics: de-esser and compressor on the whole mix, then a
/// soft limiter. No look-ahead, no latency. Real-time safe.
#[derive(Debug, Clone, Default)]
pub struct Output {
    dynamics: Dynamics,
    ceiling: f32,
    active: bool,
}

impl Output {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.dynamics.prepare(sample_rate);
    }

    pub fn reset(&mut self) {
        self.dynamics.reset();
    }

    pub fn set(&mut self, s: OutputSettings, on: bool) {
        let c = s.comp.clamp(0.0, 1.0);
        self.dynamics.set(DynamicsSettings {
            gate_db: GATE_OFF_DB,
            deess: s.deess,
            deess_hz: 6500.0,
            threshold_db: -6.0 - 24.0 * c,
            ratio: if c > 0.0 { 1.5 + 4.5 * c } else { 1.0 },
            attack_ms: 3.0,
            release_ms: 120.0,
            makeup_db: 9.0 * c,
        });
        self.ceiling = from_db(s.ceiling_db.min(0.0));
        self.active = on;
    }

    /// Gain reduction of the compressor and de-esser, in dB (≤ 0).
    #[must_use]
    pub fn reduction_db(&self) -> (f32, f32) {
        self.dynamics.reduction_db()
    }

    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.active {
            return (l, r);
        }
        let (l, r) = self.dynamics.tick(l, r);
        (limit(l, self.ceiling), limit(r, self.ceiling))
    }
}

/// Soft limit to `ceiling`: untouched below 80 % of it.
#[inline]
fn limit(x: f32, ceiling: f32) -> f32 {
    let knee = 0.8 * ceiling;
    let a = x.abs();
    if a <= knee {
        x
    } else {
        let room = ceiling - knee;
        x.signum() * (knee + room * ((a - knee) / room).tanh())
    }
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    if x.abs() < 1.0 {
        x
    } else {
        x.signum() * (1.0 + (x.abs() - 1.0).tanh() * 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: f32, hz: f32, n: usize, amp: f32) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss)]
        (0..n)
            .map(|i| amp * (2.0 * PI * hz * i as f32 / sr).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn neutral_dynamics_are_transparent() {
        let mut d = Dynamics::default();
        d.prepare(48_000.0);
        for x in sine(48_000.0, 220.0, 4800, 0.5) {
            let (l, r) = d.tick(x, x);
            assert!((l - x).abs() < 1e-6 && (r - x).abs() < 1e-6);
        }
    }

    #[test]
    fn compressor_reduces_loud_and_not_quiet() {
        let mut d = Dynamics::default();
        d.prepare(48_000.0);
        d.set(DynamicsSettings {
            threshold_db: -20.0,
            ratio: 4.0,
            ..DynamicsSettings::default()
        });
        let loud = sine(48_000.0, 220.0, 48_000, 0.9);
        let out: Vec<f32> = loud.iter().map(|&x| d.tick(x, x).0).collect();
        let gain = rms(&out[24_000..]) / rms(&loud[24_000..]);
        assert!(gain < 0.4, "loud gain {gain}");
        d.reset();
        let quiet = sine(48_000.0, 220.0, 48_000, 0.01);
        let out: Vec<f32> = quiet.iter().map(|&x| d.tick(x, x).0).collect();
        let gain = rms(&out[24_000..]) / rms(&quiet[24_000..]);
        assert!((gain - 1.0).abs() < 0.02, "quiet gain {gain}");
    }

    #[test]
    fn deesser_cuts_highs_not_lows() {
        let mut d = Dynamics::default();
        d.prepare(48_000.0);
        d.set(DynamicsSettings {
            deess: 1.0,
            ..DynamicsSettings::default()
        });
        let hiss = sine(48_000.0, 8000.0, 24_000, 0.3);
        let out: Vec<f32> = hiss.iter().map(|&x| d.tick(x, x).0).collect();
        assert!(rms(&out[12_000..]) < 0.5 * rms(&hiss[12_000..]));
        d.reset();
        let low = sine(48_000.0, 200.0, 24_000, 0.3);
        let out: Vec<f32> = low.iter().map(|&x| d.tick(x, x).0).collect();
        let g = rms(&out[12_000..]) / rms(&low[12_000..]);
        assert!((g - 1.0).abs() < 0.05, "low gain {g}");
    }

    #[test]
    fn gate_closes_on_silence_and_opens_on_voice() {
        let mut d = Dynamics::default();
        d.prepare(48_000.0);
        d.set(DynamicsSettings {
            gate_db: -40.0,
            ..DynamicsSettings::default()
        });
        let noise = sine(48_000.0, 300.0, 24_000, 0.002);
        let out: Vec<f32> = noise.iter().map(|&x| d.tick(x, x).0).collect();
        assert!(rms(&out[12_000..]) < 1e-4);
        let voice = sine(48_000.0, 300.0, 24_000, 0.3);
        let out: Vec<f32> = voice.iter().map(|&x| d.tick(x, x).0).collect();
        assert!(rms(&out[12_000..]) > 0.95 * rms(&voice[12_000..]));
    }

    #[test]
    fn synced_delay_repeats_on_the_beat() {
        let sr = 48_000.0;
        let mut s = Space::default();
        s.prepare(sr);
        s.set(SpaceSettings {
            division: Division::Quarter,
            delay_level: 1.0,
            reverb_level: 0.0,
            feedback: 0.0,
            delay_tone: 0.0,
            ..SpaceSettings::default()
        });
        s.set_transport(120.0, 4.0);
        s.reset();
        let n = 30_000;
        let mut peak = (0_usize, 0.0_f32);
        for i in 0..n {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, _) = s.tick(x, x, 0.0);
            if l.abs() > peak.1 {
                peak = (i, l.abs());
            }
        }
        // 1/4 at 120 BPM = 0.5 s = 24000 samples.
        assert!(peak.0.abs_diff(24_000) <= 2, "{peak:?}");
    }

    #[test]
    fn reverb_decays_and_stays_finite() {
        let mut s = Space::default();
        s.prepare(48_000.0);
        s.set(SpaceSettings {
            reverb_level: 1.0,
            decay_s: 1.0,
            delay_level: 1.0,
            feedback: 0.95,
            ..SpaceSettings::default()
        });
        let mut early = 0.0_f32;
        let mut late = 0.0_f32;
        for i in 0..240_000 {
            let x = if i < 4800 { 0.8 } else { 0.0 };
            let (l, r) = s.tick(x, x, 0.0);
            assert!(l.is_finite() && r.is_finite());
            if (4800..14_400).contains(&i) {
                early = early.max(l.abs());
            }
            if i > 230_000 {
                late = late.max(l.abs());
            }
        }
        assert!(early > 0.01);
        assert!(late < early);
    }

    #[test]
    fn duck_lowers_the_bus_while_singing() {
        let mut s = Space::default();
        s.prepare(48_000.0);
        let set = SpaceSettings {
            reverb_level: 1.0,
            delay_level: 0.0,
            ..SpaceSettings::default()
        };
        let run = |s: &mut Space, duck: f32| {
            s.set(SpaceSettings { duck, ..set });
            s.reset();
            let x = sine(48_000.0, 220.0, 24_000, 0.3);
            let out: Vec<f32> = x.iter().map(|&v| s.tick(v, v, v).0).collect();
            rms(&out[12_000..])
        };
        let open = run(&mut s, 0.0);
        let ducked = run(&mut s, 1.0);
        assert!(ducked < 0.5 * open, "{ducked} vs {open}");
    }

    #[test]
    fn neutral_tone_and_chorus_are_transparent() {
        let mut t = Tone::default();
        t.prepare(48_000.0);
        let mut c = Chorus::default();
        c.prepare(48_000.0);
        for x in sine(48_000.0, 330.0, 4800, 0.5) {
            let (a, b) = t.pre(x, x);
            let (a, b) = t.post(a, b);
            let (a, b) = c.tick(a, b);
            assert!((a - x).abs() < 1e-6 && (b - x).abs() < 1e-6);
        }
    }

    #[test]
    fn tone_cuts_lows_and_lifts_air() {
        let mut t = Tone::default();
        t.prepare(48_000.0);
        t.set(ToneSettings {
            low_cut_hz: 200.0,
            air_db: 6.0,
            ..ToneSettings::default()
        });
        let run = |t: &mut Tone, hz: f32| {
            t.reset();
            let x = sine(48_000.0, hz, 24_000, 0.1);
            let y: Vec<f32> = x
                .iter()
                .map(|&v| {
                    let (a, b) = t.pre(v, v);
                    t.post(a, b).0
                })
                .collect();
            rms(&y[12_000..]) / rms(&x[12_000..])
        };
        assert!(run(&mut t, 50.0) < 0.1);
        let mid = run(&mut t, 1000.0);
        assert!((mid - 1.0).abs() < 0.1, "mid {mid}");
        assert!(run(&mut t, 15_000.0) > 1.6);
    }

    #[test]
    fn chorus_moves_and_keeps_level() {
        let mut c = Chorus::default();
        c.prepare(48_000.0);
        c.set(ChorusSettings {
            mix: 1.0,
            ..ChorusSettings::default()
        });
        let x = sine(48_000.0, 440.0, 48_000, 0.3);
        let y: Vec<f32> = x.iter().map(|&v| c.tick(v, v).0).collect();
        let g = rms(&y[24_000..]) / rms(&x[24_000..]);
        assert!(g > 0.5 && g < 1.3, "{g}");
        assert!(y.iter().zip(&x).any(|(a, b)| (a - b).abs() > 0.01));
    }

    #[test]
    fn modulated_reverb_of_every_size_stays_finite() {
        let mut s = Space::default();
        s.prepare(48_000.0);
        for size in [0.0, 0.5, 1.0] {
            s.set(SpaceSettings {
                reverb_level: 1.0,
                reverb_size: size,
                reverb_mod: 1.0,
                decay_s: 8.0,
                delay_level: 1.0,
                delay_mod: 1.0,
                delay_width: 1.0,
                delay_to_reverb: 1.0,
                feedback: 0.9,
                ..SpaceSettings::default()
            });
            s.reset();
            let mut peak = 0.0_f32;
            for i in 0..96_000 {
                let x = if i < 4800 { 0.8 } else { 0.0 };
                let (l, r) = s.tick(x, x, 0.0);
                assert!(l.is_finite() && r.is_finite());
                peak = peak.max(l.abs());
            }
            assert!(peak > 0.01 && peak < 8.0, "size {size}: {peak}");
        }
    }

    #[test]
    fn every_drive_style_is_bounded_and_dry_at_zero() {
        let x = sine(48_000.0, 220.0, 9600, 0.5);
        for style in DriveStyle::ALL {
            let mut d = Distortion::default();
            d.prepare(48_000.0);
            d.set(DriveSettings {
                style,
                drive: 0.8,
                tone: 0.5,
            });
            for &v in &x {
                let (a, _) = d.tick(v, v, 0.0);
                assert!((a - v).abs() < 1e-7);
            }
            let y: Vec<f32> = x.iter().map(|&v| d.tick(v, v, 1.0).0).collect();
            assert!(
                y.iter().all(|v| v.is_finite() && v.abs() < 2.0),
                "{style:?}"
            );
            assert!(rms(&y[4800..]) > 0.01, "{style:?} silent");
            assert!(
                y.iter().zip(&x).any(|(a, b)| (a - b).abs() > 0.02),
                "{style:?} clean"
            );
        }
    }

    #[test]
    fn output_limiter_holds_the_ceiling() {
        let mut o = Output::default();
        o.prepare(48_000.0);
        o.set(
            OutputSettings {
                ceiling_db: -6.0,
                ..OutputSettings::default()
            },
            true,
        );
        let c = from_db(-6.0);
        for v in sine(48_000.0, 220.0, 4800, 2.0) {
            assert!(o.tick(v, v).0.abs() <= c + 1e-6);
        }
        assert!((o.tick(0.1, 0.1).0 - 0.1).abs() < 1e-6);
    }
}
