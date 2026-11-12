//! Autotune (roadmap step 4): which note to correct to, and how.
//!
//! **Notes.** The allowed notes are a 12-bit mask of pitch classes
//! (bit 0 = C): the scale in its key ([`Scale`], [`scale_mask`]), minus
//! the notes switched off, or the notes held on the MIDI input
//! ([`Source`]).
//!
//! **Correction.** [`Retuner`] runs inside the shifter, once per grain,
//! on the grain's own period (the distance between two pulses of the
//! waveform), not on the detector's reading. The detector is about
//! 25 ms late; correcting a moving voice from it would put the vibrato
//! back out of phase. On the grain, everything is in time:
//!
//! ```text
//! dev     = sung - target                     (semitones)
//! centre  = low-pass of dev at ~3 Hz          (the note without its vibrato)
//! vibrato = dev - centre
//! lp      = low-pass of (dev - keep · vibrato), time constant = retune speed
//! out     = sung - amount · lp  (+ transposition)
//! ```
//!
//! - Retune 0 ms: `lp = dev`, the grains are laid exactly one target
//!   period apart. Jitter and vibrato are gone: the hard-tune effect,
//!   with clean jumps from note to note.
//! - Slower: `out = target + high-pass(dev)`, the classic retune speed:
//!   intonation drifts are pulled in, faster motion (vibrato, scoops)
//!   passes through.
//! - *Vibrato* keeps the vibrato even at fast speeds, because it is
//!   left out of what is corrected.
//! - *Humanize* slows the retune down as a note is held, so sustained
//!   notes breathe while fast passages stay tight.
//!
//! **Note choice** uses a slightly smoothed sung pitch with hysteresis:
//! the target changes only once the voice is clearly nearer another
//! allowed note, so a vibrato or a wobble around a midpoint never makes
//! the output flip between two notes.
//!
//! Real-time: no allocation, a few multiplications per grain.

use crate::midi::NoteStack;

/// Scales, as 12-bit masks over pitch classes, bit 0 = the key's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Scale {
    #[default]
    Chromatic,
    Major,
    NaturalMinor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Mixolydian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

impl Scale {
    pub const COUNT: u32 = 10;
    pub const ALL: [Self; 10] = [
        Self::Chromatic,
        Self::Major,
        Self::NaturalMinor,
        Self::HarmonicMinor,
        Self::MelodicMinor,
        Self::Dorian,
        Self::Mixolydian,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::Blues,
    ];

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
            Self::Chromatic => "Chromatic",
            Self::Major => "Major",
            Self::NaturalMinor => "Minor",
            Self::HarmonicMinor => "Harmonic minor",
            Self::MelodicMinor => "Melodic minor",
            Self::Dorian => "Dorian",
            Self::Mixolydian => "Mixolydian",
            Self::MajorPentatonic => "Major pentatonic",
            Self::MinorPentatonic => "Minor pentatonic",
            Self::Blues => "Blues",
        }
    }

    #[must_use]
    pub const fn mask(self) -> u16 {
        match self {
            Self::Chromatic => 0b1111_1111_1111,
            Self::Major => 0b1010_1011_0101,
            Self::NaturalMinor => 0b0101_1010_1101,
            Self::HarmonicMinor => 0b1001_1010_1101,
            Self::MelodicMinor => 0b1010_1010_1101,
            Self::Dorian => 0b0110_1010_1101,
            Self::Mixolydian => 0b0110_1011_0101,
            Self::MajorPentatonic => 0b0010_1001_0101,
            Self::MinorPentatonic => 0b0100_1010_1001,
            Self::Blues => 0b0100_1110_1001,
        }
    }
}

/// Where the target notes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Source {
    /// The scale in its key, minus the notes switched off.
    #[default]
    Scale,
    /// The pitch classes held on the MIDI input, in any octave; the
    /// scale while no note is held.
    Midi,
    /// The exact note held on the MIDI input (last pressed), whatever
    /// is sung: play the melody, sing or speak anything. The scale
    /// while no note is held.
    MidiExact,
}

impl Source {
    pub const COUNT: u32 = 3;
    pub const ALL: [Self; 3] = [Self::Scale, Self::Midi, Self::MidiExact];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scale => "Scale",
            Self::Midi => "MIDI",
            Self::MidiExact => "MIDI Exact",
        }
    }
}

/// Pitch classes of `scale` in `key` (0 = C .. 11 = B), bit 0 = C.
#[must_use]
pub const fn scale_mask(key: u8, scale: Scale) -> u16 {
    let m = scale.mask() as u32;
    let k = (key % 12) as u32;
    (((m << k) | (m >> (12 - k))) & 0xFFF) as u16
}

/// Pitch classes held on the MIDI input, bit 0 = C.
#[must_use]
pub fn held_mask(held: &NoteStack) -> u16 {
    held.held().iter().fold(0, |m, n| m | (1 << (n.note % 12)))
}

/// Nearest note to the fractional MIDI note `note` whose pitch class is
/// in `mask` (bit 0 = C). Ties go down. `None` for an empty mask.
#[must_use]
pub fn nearest_in_mask(note: f32, mask: u16) -> Option<f32> {
    if mask & 0xFFF == 0 || !note.is_finite() {
        return None;
    }
    let centre = note.round();
    let mut best: Option<(f32, f32)> = None;
    for off in -6..=6 {
        #[allow(clippy::cast_precision_loss)]
        let cand = centre + off as f32;
        #[allow(clippy::cast_possible_truncation)]
        let pc = (cand as i32).rem_euclid(12);
        if mask & (1 << pc) == 0 {
            continue;
        }
        let d = (cand - note).abs();
        if best.is_none_or(|(_, bd)| d < bd - 1e-6) {
            best = Some((cand, d));
        }
    }
    best.map(|b| b.0)
}

/// Nearest note of `scale` in `key` to `note`. Ties go down.
#[must_use]
pub fn nearest_in_scale(note: f32, key: u8, scale: Scale) -> f32 {
    nearest_in_mask(note, scale_mask(key, scale)).unwrap_or(note)
}

/// Nearest held MIDI note to `note`, octave-free. `None` if no note is
/// held.
#[must_use]
pub fn nearest_held(note: f32, held: &NoteStack) -> Option<f32> {
    nearest_in_mask(note, held_mask(held))
}

/// How far past the midpoint between two notes the voice must go
/// before the target changes, in semitones: wide in the Natural style,
/// so a note held a little low stays on its note; tighter in Tune, so
/// runs land on every note.
const HYSTERESIS_NATURAL_ST: f64 = 0.35;
const HYSTERESIS_TUNE_ST: f64 = 0.2;
/// Natural style: the correction eases in over the first part of each
/// note, so scoops into the note and very short notes keep their own
/// pitch (they are heard as expression, not as errors).
const NATURAL_EASE_S: f64 = 0.08;
/// Natural style: share of the vibrato always kept.
pub const NATURAL_MIN_VIBRATO: f64 = 0.5;
/// Smoothing of the sung pitch used to choose the note.
const CHOICE_S: f64 = 0.012;
/// Cutoff of the vibrato centre (vibrato is 4 to 8 Hz).
const CENTRE_HZ: f64 = 3.0;
/// Widest motion counted as vibrato, in semitones.
const VIBRATO_MAX_ST: f64 = 2.0;
/// Vibrato detection: the motion around the centre must swing past
/// ±[`VIBRATO_MIN_ST`] and alternate at a vibrato's pace (half a cycle
/// of 2.5 to 10 Hz) to count. A scoop or a drift goes one way only and
/// is corrected.
const VIBRATO_MIN_ST: f64 = 0.06;
const VIBRATO_HALF_MIN_S: f64 = 0.05;
const VIBRATO_HALF_MAX_S: f64 = 0.2;
/// How fast the vibrato gate opens and closes.
const VIBRATO_GATE_S: f64 = 0.04;
/// Humanize: extra retune time on a note held long, reached after
/// [`HUMANIZE_RAMP_S`].
const HUMANIZE_S: f64 = 0.35;
const HUMANIZE_RAMP_S: f64 = 0.3;
/// A gap in the voice longer than this starts the next note afresh.
const RESTART_S: f64 = 0.06;
/// So does a jump of the sung pitch wider than this (a new note after
/// a consonant, or a glitch of the marks): its state would only drag
/// the correction the wrong way.
const RESTART_JUMP_ST: f64 = 3.0;
/// Onset of a note: the note is chosen again on every grain, without
/// hysteresis or glide, while the voice settles.
const ONSET_S: f64 = 0.02;
/// Grain periods in the median (odd): at a consonant, a few marks in a
/// row can land on noise.
const MEDIAN: usize = 5;

/// Autotune settings, set by the engine from the parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuneSettings {
    /// 0 (off) to 1 (full correction).
    pub amount: f64,
    /// Retune time constant in seconds (0 = hard tune).
    pub retune_s: f64,
    /// 0 to 1.
    pub humanize: f64,
    /// Share of the vibrato kept, 0 to 1.
    pub vibrato: f64,
    /// Allowed pitch classes (bit 0 = C); 0 disables the correction.
    pub mask: u16,
    /// Exact target note (MIDI Exact with a note held).
    pub exact: Option<f64>,
    /// Natural style (see the consts above); otherwise Tune.
    pub natural: bool,
    /// Tune style: time the target takes to move from one note to the
    /// next, on top of the retune (0 = a jump).
    pub transition_s: f64,
}

impl Default for TuneSettings {
    fn default() -> Self {
        Self {
            amount: 0.0,
            retune_s: 0.05,
            humanize: 0.0,
            vibrato: 1.0,
            mask: 0xFFF,
            exact: None,
            natural: false,
            transition_s: 0.0,
        }
    }
}

/// What the correction is doing, for the editor.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TuneReading {
    /// Note sung, on the grain (fractional MIDI note).
    pub sung: f32,
    /// Target note.
    pub target: f32,
    /// Note sent out, before transposition.
    pub out: f32,
}

/// Per-grain pitch correction. See the module docs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Retuner {
    pub settings: TuneSettings,
    /// Transposition added on top of the correction, in semitones.
    pub transpose: f64,
    /// Transpose first, then correct: the output lands on the scale
    /// (Keep In Key). Otherwise the transposition comes after.
    pub transpose_first: bool,
    sample_rate: f64,
    active: bool,
    target: f64,
    choice: f64,
    centre: [f64; 2],
    lp: f64,
    held_s: f64,
    /// Target in effect (gliding during a transition), where the glide
    /// started, and how far into it we are.
    glided: f64,
    from: f64,
    trans_s: f64,
    /// Vibrato detector: side of the last swing (+1, -1, 0), time since
    /// it, swings in a row at a vibrato's pace, gate (0 to 1).
    swing: f64,
    swing_s: f64,
    swings: u8,
    gate: f64,
    /// Time since the note started (onset phase).
    age_s: f64,
    gap_s: f64,
    /// Last grain periods, for a median: misplaced pulse marks must not
    /// move the note choice.
    periods: [f64; MEDIAN],
    reading: Option<TuneReading>,
}

impl Retuner {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.reading = None;
        self.gap_s = 0.0;
    }

    /// Latest reading, `None` while unvoiced or with the correction off.
    #[must_use]
    pub fn reading(&self) -> Option<TuneReading> {
        self.reading
    }

    fn enabled(&self) -> bool {
        self.settings.amount > 0.0
            && (self.settings.mask & 0xFFF != 0 || self.settings.exact.is_some())
    }

    fn choose(&self, from: f64) -> f64 {
        if let Some(n) = self.settings.exact {
            return n;
        }
        #[allow(clippy::cast_possible_truncation)]
        let near = nearest_in_mask(from as f32, self.settings.mask);
        near.map_or(from, f64::from)
    }

    /// Pitch ratio for a voiced grain of `period` samples, `dt` seconds
    /// after the previous grain.
    pub fn voiced(&mut self, period: f64, dt: f64) -> f64 {
        let transpose = self.transpose;
        if !self.enabled() || period.is_nan() || period <= 0.0 {
            self.active = false;
            self.reading = None;
            return st_to_ratio(transpose);
        }
        let s = &self.settings;
        // Keep In Key: the note is chosen for the transposed voice.
        let offset = if self.transpose_first { transpose } else { 0.0 };
        let sung = crate::pitch::hz_to_midi_f64(self.sample_rate / period) + offset;
        let mut restart = !self.active || self.gap_s > RESTART_S;
        self.gap_s = 0.0;
        let dt = dt.clamp(0.0, 0.05);
        if restart {
            self.periods = [period; MEDIAN];
        } else {
            self.periods.copy_within(1.., 0);
            self.periods[MEDIAN - 1] = period;
        }
        let mut sorted = self.periods;
        sorted.sort_unstable_by(f64::total_cmp);
        let robust = crate::pitch::hz_to_midi_f64(self.sample_rate / sorted[MEDIAN / 2]) + offset;
        if !restart && (robust - self.choice).abs() > RESTART_JUMP_ST {
            restart = true;
            self.periods = [period; MEDIAN];
        }
        let robust = if restart { sung } else { robust };
        if restart {
            self.active = true;
            self.held_s = 0.0;
            self.age_s = 0.0;
        }
        self.age_s += dt;
        if self.age_s < ONSET_S {
            self.choice = robust;
            self.target = self.choose(robust);
            let dev = robust - self.target;
            self.centre = [dev; 2];
            self.swing = 0.0;
            self.swings = 0;
            self.gate = 0.0;
            // A MIDI Exact note starts on the played note; a sung one
            // starts as sung and glides in at the retune speed.
            self.lp = if s.exact.is_some() { dev } else { 0.0 };
            self.glided = self.target;
            self.trans_s = f64::INFINITY;
        } else {
            self.choice += coef(dt, CHOICE_S) * (robust - self.choice);
            let cand = self.choose(self.choice);
            let switch = if s.exact.is_some() {
                (cand - self.target).abs() > 1e-6
            } else {
                (cand - self.target).abs() > 1e-6
                    && (self.choice - self.target).abs() - (self.choice - cand).abs()
                        > 2.0
                            * if s.natural {
                                HYSTERESIS_NATURAL_ST
                            } else {
                                HYSTERESIS_TUNE_ST
                            }
            };
            if switch {
                self.from = self.glided;
                self.trans_s = 0.0;
                self.target = cand;
                self.held_s = 0.0;
            }
        }
        self.held_s += dt;
        // The target in effect: in Tune it moves to a new note over the
        // transition time; in Natural it jumps and the retune glides.
        self.trans_s += dt;
        let glided = if s.natural || s.transition_s <= 0.0 || self.trans_s >= s.transition_s {
            self.target
        } else {
            let x = self.trans_s / s.transition_s;
            self.from + (self.target - self.from) * x * x * (3.0 - 2.0 * x)
        };
        // The vibrato centre follows the target; the correction does
        // not, so it glides at the retune speed.
        let moved = glided - self.glided;
        self.centre[0] -= moved;
        self.centre[1] -= moved;
        self.glided = glided;
        let dev = robust - glided;
        // Two one-pole stages: a gentle, critically damped centre.
        let a = coef(dt, 1.0 / (std::f64::consts::TAU * CENTRE_HZ));
        self.centre[0] += a * (dev - self.centre[0]);
        self.centre[1] += a * (self.centre[0] - self.centre[1]);
        for c in &mut self.centre {
            *c = c.clamp(dev - VIBRATO_MAX_ST, dev + VIBRATO_MAX_ST);
        }
        let vibrato = (dev - self.centre[1]).clamp(-VIBRATO_MAX_ST, VIBRATO_MAX_ST);
        self.swing_s += dt;
        if vibrato.abs() > VIBRATO_MIN_ST && vibrato.signum() != self.swing {
            let paced = (VIBRATO_HALF_MIN_S..=VIBRATO_HALF_MAX_S).contains(&self.swing_s);
            self.swings = if paced && self.swing != 0.0 {
                self.swings.saturating_add(1).min(4)
            } else {
                1
            };
            self.swing = vibrato.signum();
            self.swing_s = 0.0;
        }
        if self.swing_s > VIBRATO_HALF_MAX_S * 1.5 {
            self.swings = 0;
        }
        let open = if self.swings >= 2 { 1.0 } else { 0.0 };
        self.gate += coef(dt, VIBRATO_GATE_S) * (open - self.gate);
        let wanted = dev - s.vibrato * self.gate * vibrato;
        let human = s.humanize * HUMANIZE_S * (self.held_s / HUMANIZE_RAMP_S).min(1.0);
        self.lp += coef(dt, s.retune_s + human) * (wanted - self.lp);
        // Hard tune works on the raw grain, so the grains land exactly
        // one target period apart (no jitter left); otherwise the grain's
        // own jitter is the voice's and passes through.
        let amount = if s.natural && s.exact.is_none() {
            let x = (self.age_s / NATURAL_EASE_S).min(1.0);
            s.amount * x * x * (3.0 - 2.0 * x)
        } else {
            s.amount
        };
        let corr = if s.retune_s + human <= 0.0 && s.vibrato <= 0.0 {
            amount * (glided - sung)
        } else {
            -amount * self.lp
        };
        #[allow(clippy::cast_possible_truncation)]
        let reading = TuneReading {
            sung: robust as f32,
            target: self.target as f32,
            out: (robust + corr) as f32,
        };
        self.reading = Some(reading);
        st_to_ratio(corr + transpose)
    }

    /// An unvoiced grain, `dt` seconds after the previous one.
    pub fn unvoiced(&mut self, dt: f64) {
        self.gap_s += dt.max(0.0);
        if self.gap_s > RESTART_S {
            self.reading = None;
        }
    }
}

/// One-pole coefficient for a step of `dt` and a time constant `tau`
/// (1 when `tau` is 0: no smoothing).
fn coef(dt: f64, tau: f64) -> f64 {
    if tau <= 1e-9 {
        1.0
    } else {
        1.0 - (-dt / tau).exp()
    }
}

fn st_to_ratio(st: f64) -> f64 {
    (st / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_scale() {
        // C major: C# 61.4 -> D 62 is nearer than C 60? 61.4 is 1.4 from
        // 60 and 0.6 from 62.
        assert_eq!(nearest_in_scale(61.4, 0, Scale::Major), 62.0);
        assert_eq!(nearest_in_scale(60.2, 0, Scale::Major), 60.0);
        // A minor (key 9): G# is out, G is in.
        assert_eq!(nearest_in_scale(67.9, 9, Scale::NaturalMinor), 67.0);
        assert_eq!(nearest_in_scale(67.9, 9, Scale::HarmonicMinor), 68.0);
        assert_eq!(nearest_in_scale(64.4, 0, Scale::Chromatic), 64.0);
        // E blues: D E G A A# B.
        assert_eq!(scale_mask(4, Scale::Blues), 0b1110_1001_0100);
    }

    #[test]
    fn scale_masks_rotate_with_the_key() {
        assert_eq!(scale_mask(0, Scale::Major), Scale::Major.mask());
        // G major: G A B C D E F#.
        let g = scale_mask(7, Scale::Major);
        for pc in [7, 9, 11, 0, 2, 4, 6] {
            assert!(g & (1 << pc) != 0, "{pc}");
        }
        assert_eq!(g.count_ones(), 7);
        for s in Scale::ALL {
            assert_eq!(Scale::from_index(f64::from(s as u8)), s);
            assert!(s.mask() & 1 != 0, "{} lacks its root", s.name());
        }
    }

    #[test]
    fn follows_held_notes_in_any_octave() {
        let mut s = NoteStack::default();
        assert_eq!(nearest_held(60.0, &s), None);
        s.note_on(0, 43, 100); // G2
        assert_eq!(nearest_held(66.4, &s), Some(67.0)); // sung near G4
        s.note_on(0, 64, 100); // E4
        assert_eq!(nearest_held(65.0, &s), Some(64.0));
    }

    const SR: f64 = 48_000.0;

    fn period_of(note: f64) -> f64 {
        SR / (440.0 * ((note - 69.0) / 12.0).exp2())
    }

    /// Run `notes` (one sung note per grain) and return the output
    /// notes.
    fn run(r: &mut Retuner, notes: &[f64]) -> Vec<f64> {
        notes
            .iter()
            .map(|&n| {
                let p = period_of(n);
                let ratio = r.voiced(p, p / SR);
                n + 12.0 * ratio.log2()
            })
            .collect()
    }

    fn retuner(s: TuneSettings) -> Retuner {
        let mut r = Retuner::default();
        r.prepare(SR);
        r.settings = s;
        r
    }

    #[test]
    fn hard_tune_lands_on_the_note_and_flattens_vibrato() {
        let mut r = retuner(TuneSettings {
            amount: 1.0,
            retune_s: 0.0,
            vibrato: 0.0,
            ..TuneSettings::default()
        });
        // A3 (57) sung 30 ct sharp with a 6 Hz, ±40 ct vibrato; grains
        // of ~4.5 ms.
        let notes: Vec<f64> = (0..400)
            .map(|i| 57.3 + 0.4 * (std::f64::consts::TAU * 6.0 * f64::from(i) * 0.0045).sin())
            .collect();
        let out = run(&mut r, &notes);
        for o in &out[10..] {
            assert!((o - 57.0).abs() < 1e-6, "{o}");
        }
    }

    #[test]
    fn slow_retune_keeps_the_vibrato_and_fixes_the_drift() {
        let mut r = retuner(TuneSettings {
            amount: 1.0,
            retune_s: 0.08,
            vibrato: 1.0,
            ..TuneSettings::default()
        });
        let notes: Vec<f64> = (0..600)
            .map(|i| 57.3 + 0.4 * (std::f64::consts::TAU * 6.0 * f64::from(i) * 0.0045).sin())
            .collect();
        let out = run(&mut r, &notes);
        let tail = &out[300..];
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        let (lo, hi) = tail
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        assert!((mean - 57.0).abs() < 0.05, "centre {mean}");
        assert!(hi - lo > 0.6, "vibrato depth {}", hi - lo);
    }

    #[test]
    fn note_choice_has_hysteresis() {
        let mut r = retuner(TuneSettings {
            amount: 1.0,
            retune_s: 0.0,
            vibrato: 0.0,
            ..TuneSettings::default()
        });
        // Wobbling around the C/C# midpoint stays on C...
        let notes: Vec<f64> = (0..200)
            .map(|i| 60.2 + 0.25 * (f64::from(i) * 0.3).sin())
            .collect();
        let out = run(&mut r, &notes);
        assert!(out.iter().all(|o| (o - 60.0).abs() < 1e-6));
        // ...and moving clearly to D lands on D.
        let out = run(&mut r, &[62.0; 40]);
        assert!((out[39] - 62.0).abs() < 1e-6);
    }

    #[test]
    fn midi_exact_plays_the_held_note() {
        let mut r = retuner(TuneSettings {
            amount: 1.0,
            retune_s: 0.02,
            exact: Some(67.0),
            ..TuneSettings::default()
        });
        let out = run(&mut r, &[55.4; 100]);
        assert!(
            out.iter().all(|o| (o - 67.0).abs() < 0.01),
            "{:?}",
            &out[..3]
        );
    }

    #[test]
    fn keep_in_key_lands_the_transposed_voice_on_the_scale() {
        // C major, hard tune, a quarter-tone transposition (Throat
        // Pitch): after the correction it is off key, before it is not.
        let settings = TuneSettings {
            amount: 1.0,
            retune_s: 0.0,
            vibrato: 0.0,
            mask: scale_mask(0, Scale::Major),
            ..TuneSettings::default()
        };
        let mut r = retuner(settings);
        r.transpose = 0.5;
        let out = run(&mut r, &[60.1; 100]);
        assert!((out[99] - 60.5).abs() < 0.01, "{}", out[99]);
        let mut r = retuner(settings);
        r.transpose = 0.5;
        r.transpose_first = true;
        let out = run(&mut r, &[60.1; 100]);
        assert!((out[99] - 60.0).abs() < 0.01, "{}", out[99]);
    }

    #[test]
    fn off_or_empty_does_nothing_but_transpose() {
        let mut r = retuner(TuneSettings::default());
        r.transpose = 7.0;
        let out = run(&mut r, &[57.3; 10]);
        assert!(out.iter().all(|o| (o - 64.3).abs() < 1e-9));
        assert!(r.reading().is_none());
        r.settings.amount = 1.0;
        r.settings.mask = 0;
        let out = run(&mut r, &[57.3; 10]);
        assert!(out.iter().all(|o| (o - 64.3).abs() < 1e-9));
    }

    #[test]
    fn transition_glides_between_notes() {
        let mut r = retuner(TuneSettings {
            amount: 1.0,
            retune_s: 0.0,
            vibrato: 0.0,
            transition_s: 0.1,
            ..TuneSettings::default()
        });
        run(&mut r, &[60.0; 60]);
        // Jump to D: the output takes ~100 ms to get there, not a step.
        let out = run(&mut r, &[62.0; 60]);
        let mid = out.iter().position(|o| *o > 61.0).unwrap();
        #[allow(clippy::cast_precision_loss)]
        let t = mid as f64 * period_of(62.0) / SR;
        assert!((0.03..0.09).contains(&t), "{t}");
        assert!((out[59] - 62.0).abs() < 1e-6);
    }

    #[test]
    fn natural_eases_in_and_keeps_scoops() {
        let s = TuneSettings {
            amount: 1.0,
            retune_s: 0.03,
            vibrato: 0.5,
            natural: true,
            ..TuneSettings::default()
        };
        let mut r = retuner(s);
        // A note starting 40 ct flat: its first 40 ms stay near as sung.
        let out = run(&mut r, &[59.6; 120]);
        assert!((out[4] - 59.6).abs() < 0.1, "{}", out[4]);
        assert!((out[119] - 60.0).abs() < 0.03, "{}", out[119]);
    }
}
