//! Harmony voices (roadmap step 6).
//!
//! Up to [`MAX_VOICES`] extra voices built from the sung one, on the
//! same analysis grains as the lead (see [`crate::shift::Psola`]): each
//! voice has its own synthesis marks, at its own pitch, with the grain
//! shapes (the formants) of the singer. A harmony therefore sounds like
//! the same singer singing another note, not like a resampled copy, and
//! costs no latency: it reads the same input as the lead.
//!
//! Where the notes come from ([`HarmonyMode`]):
//!
//! - **MIDI**: every held note is a voice, at that pitch (polyphonic,
//!   up to 8). The singer's vibrato and inflections are carried over
//!   (*Expression*), so the stack moves with the voice.
//! - **Scale**: up to four voices at fixed intervals counted in the
//!   notes of the autotune's scale (a third above, a sixth below...),
//!   from the note the lead is on: the intervals follow the key (major
//!   or minor thirds where they belong). Chromatic counts in the major
//!   scale of the key.
//! - **Chord**: the same intervals, each moved to the nearest note of
//!   the chord held on the MIDI input (any octave), never onto the
//!   lead's own note; without a chord, the Scale behaviour.
//!
//! What makes the stack sound like several singers rather than clones
//! of one: each voice lags the lead by a few slowly wandering
//! milliseconds and drifts by a few cents (*Humanize*), has its own
//! formant offset (*Character*: other throats), its own place in the
//! stereo field (*Spread*), and glides between notes (*Glide*).
//!
//! Real-time: fixed arrays, no allocation; the engine calls
//! [`Harmonizer::tick`] once per sample and the shifter asks
//! [`Harmonizer::voice_note`] once per voice grain.

use crate::midi::NoteStack;

/// Most harmony voices at once.
pub const MAX_VOICES: usize = 8;
/// Interval voices in the Scale and Chord modes.
pub const INTERVAL_VOICES: usize = 4;
/// Interval range, in scale steps either way (±9 = a tenth).
pub const MAX_STEPS: i32 = 9;
/// Interval choices: `-MAX_STEPS..=MAX_STEPS`, 0 = off.
pub const INTERVAL_COUNT: u32 = 2 * MAX_STEPS as u32 + 1;

/// Voice fade in and out.
const ATTACK_S: f64 = 0.015;
const RELEASE_S: f64 = 0.08;
/// Hysteresis of the lead's reference note, past the midpoint (st).
const LEAD_HYSTERESIS_ST: f64 = 0.3;
/// Time constant of the slow lead pitch (MIDI mode expression: what is
/// faster than this, vibrato and scoops, is copied).
const LEAD_SLOW_S: f64 = 0.15;
/// Most expression copied, either way (semitones).
const MAX_EXPRESSION_ST: f64 = 1.0;
/// Humanize at 100 %: lag range (ms) and drift (cents) of the voices.
const LAG_MIN_MS: f64 = 4.0;
const LAG_RANGE_MS: f64 = 14.0;
const DRIFT_CENTS: f64 = 12.0;
/// How often the humanize wanderers pick a new goal, and how fast they
/// get there.
const WANDER_S: f64 = 0.6;
const WANDER_TAU_S: f64 = 0.35;
/// Character at 100 %: formant offset of each voice (semitones), so
/// each sounds like another throat.
const CHARACTER_ST: [f64; MAX_VOICES] = [1.6, -1.8, 0.9, -1.1, 2.2, -2.4, 0.5, -0.6];
/// Place of each voice in the stereo field at full spread.
const PAN: [f32; MAX_VOICES] = [-1.0, 1.0, -0.5, 0.5, -0.75, 0.75, -0.25, 0.25];
/// Level of a MIDI voice at velocity 0 (127 = full).
const MIN_VELOCITY_GAIN: f32 = 0.3;
/// A pause this long ends a phrase.
const PHRASE_GAP_S: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum HarmonyMode {
    #[default]
    Off,
    Midi,
    Scale,
    Chord,
}

impl HarmonyMode {
    pub const ALL: [Self; 4] = [Self::Off, Self::Midi, Self::Scale, Self::Chord];

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
            Self::Midi => "MIDI",
            Self::Scale => "Scale",
            Self::Chord => "Chord",
        }
    }
}

/// Interval in scale steps from a parameter's plain value (index).
#[must_use]
pub fn interval_steps(index: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    let i = index.round() as i32;
    i.clamp(0, 2 * MAX_STEPS) - MAX_STEPS
}

/// Name of an interval of `steps` scale steps: "3rd up", "6th down",
/// "Octave up", "Off".
#[must_use]
pub fn interval_name(steps: i32) -> String {
    if steps == 0 {
        return "Off".to_owned();
    }
    let dir = if steps > 0 { "up" } else { "down" };
    let n = steps.unsigned_abs() + 1;
    let name = match n {
        8 => "Octave".to_owned(),
        2 => "2nd".to_owned(),
        3 => "3rd".to_owned(),
        _ => format!("{n}th"),
    };
    format!("{name} {dir}")
}

/// Short form for the knobs: "+3rd", "-6th", "+8ve".
#[must_use]
pub fn interval_short(steps: i32) -> String {
    if steps == 0 {
        return "Off".to_owned();
    }
    let sign = if steps > 0 { '+' } else { '-' };
    let n = steps.unsigned_abs() + 1;
    let name = match n {
        8 => "8ve".to_owned(),
        2 => "2nd".to_owned(),
        3 => "3rd".to_owned(),
        _ => format!("{n}th"),
    };
    format!("{sign}{name}")
}

/// The note `steps` notes of `mask` (pitch classes, bit 0 = C) away from
/// `from`, which is taken as a note of the scale.
#[must_use]
pub fn scale_step(from: i32, steps: i32, mask: u16) -> i32 {
    let mask = if mask & 0xFFF == 0 { 0xFFF } else { mask };
    let in_mask = |n: i32| mask & (1 << n.rem_euclid(12)) != 0;
    let mut n = from;
    let dir = steps.signum();
    for _ in 0..steps.abs() {
        n += dir;
        while !in_mask(n) {
            n += dir;
        }
    }
    n
}

/// Nearest note of `chord` (pitch classes) to `note` on the side `dir`
/// of `lead` (+1 above, -1 below; preferred on a tie), never on `avoid`.
#[must_use]
pub fn nearest_chord_note(
    note: i32,
    chord: u16,
    dir: i32,
    lead: i32,
    avoid: &[i32],
) -> Option<i32> {
    if chord & 0xFFF == 0 {
        return None;
    }
    let pref = if dir < 0 { -1 } else { 1 };
    for d in 0..=12 {
        for cand in [note + d * pref, note - d * pref] {
            let side = (cand - lead) * pref > 0;
            if side && chord & (1 << cand.rem_euclid(12)) != 0 && !avoid.contains(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// Settings from the parameters (the engine fills them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonySettings {
    pub mode: HarmonyMode,
    /// Scale steps of the interval voices (0 = off).
    pub intervals: [i32; INTERVAL_VOICES],
    /// Pitch classes the intervals count in (the engine passes the
    /// major scale of the key when the autotune is chromatic: thirds
    /// must be thirds).
    pub scale: u16,
    /// Pitch classes of the chord held on the MIDI input.
    pub chord: u16,
    /// Share of the lead's inflections the voices copy, 0 to 1.
    pub expression: f64,
    /// Glide between notes, seconds.
    pub glide_s: f64,
    /// Lag and drift, 0 to 1.
    pub humanize: f64,
    /// Formant offsets, 0 to 1.
    pub character: f64,
    /// Stereo spread, 0 to 1.
    pub spread: f32,
}

impl Default for HarmonySettings {
    fn default() -> Self {
        Self {
            mode: HarmonyMode::Off,
            intervals: [0; INTERVAL_VOICES],
            scale: 0xFFF,
            chord: 0,
            expression: 1.0,
            glide_s: 0.03,
            humanize: 0.3,
            character: 0.25,
            spread: 0.6,
        }
    }
}

/// What the shifter needs from a voice for this sample.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VoiceMix {
    /// The voice places grains (it may still be fading).
    pub active: bool,
    /// Output gains, left and right.
    pub gain: [f32; 2],
    /// Lag behind the lead, in samples.
    pub lag: f64,
    /// Formant ratio on top of the global one.
    pub formant: f64,
}

/// A slowly wandering value in 0..1 (humanize).
#[derive(Debug, Clone, Copy, Default)]
struct Wander {
    value: f64,
    goal: f64,
    left: f64,
}

impl Wander {
    fn tick(&mut self, rng: &mut u32, dt: f64) -> f64 {
        self.left -= dt;
        if self.left <= 0.0 {
            self.goal = next_unit(rng);
            self.left = WANDER_S * (0.5 + next_unit(rng));
        }
        self.value += (self.goal - self.value) * (dt / WANDER_TAU_S).min(1.0);
        self.value
    }
}

fn next_unit(rng: &mut u32) -> f64 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 17;
    *rng ^= *rng << 5;
    f64::from(*rng) / f64::from(u32::MAX)
}

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// Note asked for (MIDI note or interval note), while gated.
    note: Option<f64>,
    /// Note sung now (glides to `note`).
    current: f64,
    /// Note on (MIDI) or interval on.
    gate: bool,
    velocity: f32,
    /// Fade, 0 to 1.
    env: f32,
    /// Pan position now (moves with the spread).
    pan: f32,
    lag: Wander,
    drift: Wander,
    /// Press order (MIDI): the oldest released slot is reused first.
    stamp: u64,
}

/// The harmony voices. See the module docs.
#[derive(Debug, Clone)]
pub struct Harmonizer {
    pub settings: HarmonySettings,
    sample_rate: f64,
    slots: [Slot; MAX_VOICES],
    attack: f32,
    release: f32,
    pan_coef: f32,
    rng: u32,
    stamp: u64,
    /// Lead: note sent out (fractional), its reference note (hysteresis)
    /// and its slow pitch, `None` until a voiced grain.
    lead: Option<f64>,
    lead_ref: f64,
    lead_slow: f64,
    mix: [VoiceMix; MAX_VOICES],
    /// Per-sample humanize update divider.
    wander_tick: u32,
    /// Unvoiced time since the last voiced grain.
    gap: f64,
    /// Mode the voices were set up for.
    applied: HarmonyMode,
}

impl Default for Harmonizer {
    fn default() -> Self {
        let mut h = Self {
            settings: HarmonySettings::default(),
            sample_rate: 48_000.0,
            slots: [Slot::default(); MAX_VOICES],
            attack: 0.0,
            release: 0.0,
            pan_coef: 0.0,
            rng: 0x9E37_79B9,
            stamp: 0,
            lead: None,
            lead_ref: 60.0,
            lead_slow: 60.0,
            mix: [VoiceMix::default(); MAX_VOICES],
            wander_tick: 0,
            gap: 0.0,
            applied: HarmonyMode::Off,
        };
        h.prepare(48_000.0);
        h
    }
}

/// Samples between two humanize updates.
const WANDER_EVERY: u32 = 32;

impl Harmonizer {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        #[allow(clippy::cast_possible_truncation)]
        {
            self.attack = (1.0 / (sample_rate * ATTACK_S)) as f32;
            self.release = (1.0 / (sample_rate * RELEASE_S)) as f32;
            self.pan_coef = (1.0 - (-1.0 / (sample_rate * 0.05)).exp()) as f32;
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            *s = Slot {
                pan: PAN[i],
                ..Slot::default()
            };
        }
        self.lead = None;
        self.mix = [VoiceMix::default(); MAX_VOICES];
        // Spread the wanderers from the start.
        for i in 0..MAX_VOICES {
            let a = next_unit(&mut self.rng);
            let b = next_unit(&mut self.rng);
            self.slots[i].lag = Wander {
                value: a,
                goal: a,
                left: 0.0,
            };
            self.slots[i].drift = Wander {
                value: b,
                goal: b,
                left: 0.0,
            };
        }
    }

    /// Whether any voice can sound.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.settings.mode != HarmonyMode::Off
    }

    /// Follow the MIDI input (MIDI mode): every held note gets a voice,
    /// released ones fade out. Call after each note on or off.
    pub fn set_notes(&mut self, notes: &NoteStack) {
        let midi = self.settings.mode == HarmonyMode::Midi;
        // Release the voices whose note is gone.
        for s in &mut self.slots {
            if !s.gate {
                continue;
            }
            let held = midi
                && s.note.is_some_and(|n| {
                    notes
                        .held()
                        .iter()
                        .any(|h| (f64::from(h.note) - n).abs() < 0.5)
                });
            if !held && midi {
                s.gate = false;
            }
        }
        if !midi {
            return;
        }
        for h in notes.held() {
            let n = f64::from(h.note);
            let vel = MIN_VELOCITY_GAIN + (1.0 - MIN_VELOCITY_GAIN) * f32::from(h.velocity) / 127.0;
            if let Some(s) = self
                .slots
                .iter_mut()
                .find(|s| s.gate && s.note.is_some_and(|x| (x - n).abs() < 0.5))
            {
                s.velocity = vel;
                continue;
            }
            // A free slot: silent first, then the one released longest ago.
            let pick = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.gate)
                .min_by(|a, b| {
                    let ka = (a.1.env > 0.0, a.1.stamp);
                    let kb = (b.1.env > 0.0, b.1.stamp);
                    ka.cmp(&kb)
                })
                .map(|(i, _)| i);
            let Some(i) = pick else { continue };
            self.stamp += 1;
            let glide = self.settings.glide_s > 0.0;
            let s = &mut self.slots[i];
            if s.env <= 0.0 || !glide {
                s.current = n;
            }
            s.note = Some(n);
            s.gate = true;
            s.velocity = vel;
            s.stamp = self.stamp;
        }
    }

    /// New settings: interval voices on or off. On a change of mode,
    /// every voice fades out first (the engine then hands the held notes
    /// back with [`Harmonizer::set_notes`]).
    pub fn apply_settings(&mut self) {
        let mode = self.settings.mode;
        if mode != self.applied {
            self.applied = mode;
            for s in &mut self.slots {
                s.gate = false;
            }
        }
        if matches!(mode, HarmonyMode::Scale | HarmonyMode::Chord) {
            for (i, s) in self.slots.iter_mut().enumerate() {
                s.gate = i < INTERVAL_VOICES && self.settings.intervals[i] != 0;
                s.velocity = 1.0;
            }
        }
    }

    /// The lead's note for a voiced grain (fractional MIDI note, after
    /// correction and transposition), `dt` seconds after the last one.
    pub fn lead(&mut self, note: f64, dt: f64) {
        if !note.is_finite() {
            return;
        }
        match self.lead {
            None => {
                self.lead_slow = note;
                self.lead_ref = self.reference(note, None);
            }
            Some(_) => {
                self.lead_slow += (note - self.lead_slow) * (dt / LEAD_SLOW_S).min(1.0);
                self.lead_ref = self.reference(note, Some(self.lead_ref));
            }
        }
        self.lead = Some(note);
        self.gap = 0.0;
    }

    /// The voice is unvoiced for `dt` seconds: after a pause, the next
    /// phrase starts fresh from its own note.
    pub fn unvoiced(&mut self, dt: f64) {
        self.gap += dt.max(0.0);
        if self.gap > PHRASE_GAP_S {
            self.lead = None;
        }
    }

    /// The lead's reference note: nearest note of the scale, kept until
    /// the lead is clearly nearer another one.
    fn reference(&self, note: f64, prev: Option<f64>) -> f64 {
        let mask = self.scale_mask();
        #[allow(clippy::cast_possible_truncation)]
        let near = |x: f64| {
            let c = x.round() as i32;
            (-6..=6)
                .map(|d| c + d)
                .filter(|n| mask & (1 << n.rem_euclid(12)) != 0)
                .min_by(|a, b| {
                    (f64::from(*a) - x)
                        .abs()
                        .total_cmp(&(f64::from(*b) - x).abs())
                })
                .unwrap_or(c)
        };
        let cand = f64::from(near(note));
        match prev {
            Some(p) if p != cand => {
                // Switch only past the midpoint plus the hysteresis.
                let mid = 0.5 * (p + cand);
                let past = if cand > p { note - mid } else { mid - note };
                if past > LEAD_HYSTERESIS_ST || (note - p).abs() > 1.5 {
                    cand
                } else {
                    p
                }
            }
            _ => cand,
        }
    }

    /// Pitch classes the intervals count in.
    fn scale_mask(&self) -> u16 {
        self.settings.scale & 0xFFF
    }

    /// Notes of the interval voices for the lead's reference note.
    #[allow(clippy::cast_possible_truncation)]
    fn interval_notes(&self) -> [Option<i32>; INTERVAL_VOICES] {
        let st = &self.settings;
        let lead = self.lead_ref.round() as i32;
        let mut out = [None; INTERVAL_VOICES];
        let mut taken = [lead; INTERVAL_VOICES + 1];
        let mut n_taken = 1;
        for (i, &k) in st.intervals.iter().enumerate() {
            if k == 0 {
                continue;
            }
            let mut n = scale_step(lead, k, self.scale_mask());
            if st.mode == HarmonyMode::Chord
                && st.chord & 0xFFF != 0
                && let Some(c) =
                    nearest_chord_note(n, st.chord, k.signum(), lead, &taken[..n_taken])
            {
                n = c;
            }
            out[i] = Some(n);
            if n_taken < taken.len() {
                taken[n_taken] = n;
                n_taken += 1;
            }
        }
        out
    }

    /// Note a voice sings for its next grain (fractional MIDI note), `dt`
    /// seconds after its previous grain. `None`: the voice is silent.
    pub fn voice_note(&mut self, v: usize, dt: f64) -> Option<f64> {
        let lead = self.lead?;
        let st = self.settings;
        let target = match st.mode {
            HarmonyMode::Off => return None,
            HarmonyMode::Midi => self.slots[v].note?,
            HarmonyMode::Scale | HarmonyMode::Chord => {
                if v >= INTERVAL_VOICES {
                    return None;
                }
                let n = self.interval_notes()[v]?;
                self.slots[v].note = Some(f64::from(n));
                f64::from(n)
            }
        };
        // The lead's inflections: around its reference note in the
        // scale modes (a lead sung a little flat keeps its harmonies
        // parallel), its fast moves only over MIDI (the notes played
        // are absolute).
        let dev = match st.mode {
            HarmonyMode::Midi => lead - self.lead_slow,
            _ => lead - self.lead_ref,
        }
        .clamp(-MAX_EXPRESSION_ST, MAX_EXPRESSION_ST);
        let s = &mut self.slots[v];
        if s.env <= 0.0 {
            s.current = target;
        } else if st.glide_s > 0.0 {
            s.current += (target - s.current) * (1.0 - (-dt / st.glide_s).exp());
        } else {
            s.current = target;
        }
        let drift = st.humanize * DRIFT_CENTS / 100.0 * (2.0 * s.drift.value - 1.0);
        Some(s.current + st.expression * dev + drift)
    }

    /// One sample: fades, pans, humanize. Returns what the shifter needs.
    pub fn tick(&mut self) -> &[VoiceMix; MAX_VOICES] {
        let st = self.settings;
        self.wander_tick += 1;
        let wander = self.wander_tick >= WANDER_EVERY;
        if wander {
            self.wander_tick = 0;
        }
        #[allow(clippy::cast_precision_loss)]
        let dt = f64::from(WANDER_EVERY) / self.sample_rate;
        let mut power = 0.0f32;
        for (i, s) in self.slots.iter_mut().enumerate() {
            let on = s.gate && st.mode != HarmonyMode::Off;
            s.env = if on {
                (s.env + self.attack).min(1.0)
            } else {
                (s.env - self.release).max(0.0)
            };
            let pan = PAN[i] * st.spread;
            s.pan += (pan - s.pan) * self.pan_coef;
            if wander && (on || s.env > 0.0) {
                s.lag.tick(&mut self.rng, dt);
                s.drift.tick(&mut self.rng, dt);
            }
            let g = s.env * s.velocity;
            power += g * g;
        }
        // Several voices stack up, but not linearly: four voices are
        // about twice as loud as one.
        let norm = power.max(1.0).powf(-0.25);
        for (i, s) in self.slots.iter().enumerate() {
            let active = s.env > 0.0;
            let g = s.env * s.velocity * norm;
            // Equal power, 1 in the centre.
            let a = (f64::from(s.pan) + 1.0) * std::f64::consts::FRAC_PI_4;
            #[allow(clippy::cast_possible_truncation)]
            let (gl, gr) = (
                (a.cos() * std::f64::consts::SQRT_2) as f32,
                (a.sin() * std::f64::consts::SQRT_2) as f32,
            );
            let lag_ms = st.humanize * (LAG_MIN_MS + LAG_RANGE_MS * s.lag.value);
            self.mix[i] = VoiceMix {
                active,
                gain: [g * gl, g * gr],
                lag: lag_ms * 0.001 * self.sample_rate,
                formant: (st.character * CHARACTER_ST[i] / 12.0).exp2(),
            };
        }
        &self.mix
    }

    /// Notes the voices sing now (rounded), bit n = note n, for the
    /// editor. Empty while the lead is unvoiced.
    #[must_use]
    pub fn sounding(&self) -> u128 {
        if self.lead.is_none() || self.settings.mode == HarmonyMode::Off {
            return 0;
        }
        self.slots
            .iter()
            .filter(|s| s.env > 0.5 && s.gate)
            .filter_map(|s| s.note)
            .filter(|n| (0.0..128.0).contains(n))
            .fold(0u128, |m, n| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let n = n.round() as u32;
                m | (1u128 << n)
            })
    }

    /// The lead stopped for good (silence, reset): next phrase starts
    /// from its own note.
    pub fn lead_lost(&mut self) {
        self.lead = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_MAJOR: u16 = 0b1010_1011_0101;

    #[test]
    fn intervals_count_scale_notes() {
        // C major: a third above C is E, above E is G, a sixth below C is E.
        assert_eq!(scale_step(60, 2, C_MAJOR), 64);
        assert_eq!(scale_step(64, 2, C_MAJOR), 67);
        assert_eq!(scale_step(60, -5, C_MAJOR), 52);
        assert_eq!(scale_step(62, 7, C_MAJOR), 74);
        // Chromatic mask: semitones.
        assert_eq!(scale_step(60, 2, 0xFFF), 62);
        assert_eq!(interval_name(2), "3rd up");
        assert_eq!(interval_name(-5), "6th down");
        assert_eq!(interval_short(7), "+8ve");
        assert_eq!(interval_steps(f64::from(MAX_STEPS) + 2.0), 2);
    }

    #[test]
    fn chord_notes_avoid_the_lead() {
        // C major chord, lead on E: a third above E (G) is in the chord.
        let chord = (1 << 0) | (1 << 4) | (1 << 7);
        assert_eq!(nearest_chord_note(67, chord, 1, 64, &[64]), Some(67));
        // A second above E (F#) goes to G, the chord note above.
        assert_eq!(nearest_chord_note(66, chord, 1, 64, &[64]), Some(67));
        // Below the lead, never on it.
        assert_eq!(nearest_chord_note(64, chord, -1, 64, &[64]), Some(60));
    }

    fn harm(mode: HarmonyMode) -> Harmonizer {
        let mut h = Harmonizer::default();
        h.settings.mode = mode;
        h.settings.scale = C_MAJOR;
        h.settings.humanize = 0.0;
        h.settings.glide_s = 0.0;
        h
    }

    #[test]
    fn scale_voices_follow_the_key() {
        let mut h = harm(HarmonyMode::Scale);
        h.settings.intervals = [2, -3, 0, 0];
        h.apply_settings();
        h.lead(64.2, 0.005);
        for _ in 0..2000 {
            h.tick();
        }
        // Lead on E, 20 cents sharp: G and B below, both 20 cents sharp
        // (parallel), a minor third above E.
        let a = h.voice_note(0, 0.005).unwrap();
        let b = h.voice_note(1, 0.005).unwrap();
        assert!((a - 67.2).abs() < 1e-9, "{a}");
        assert!((b - 59.2).abs() < 1e-9, "{b}");
        assert!(h.voice_note(2, 0.005).is_none());
        // Expression off: on the notes.
        h.settings.expression = 0.0;
        assert!((h.voice_note(0, 0.005).unwrap() - 67.0).abs() < 1e-9);
    }

    #[test]
    fn midi_voices_play_the_held_notes() {
        let mut h = harm(HarmonyMode::Midi);
        let mut notes = NoteStack::default();
        notes.note_on(0, 48, 100);
        notes.note_on(0, 55, 100);
        h.set_notes(&notes);
        h.lead(60.0, 0.005);
        let mut got: Vec<f64> = (0..MAX_VOICES)
            .filter_map(|v| h.voice_note(v, 0.005))
            .collect();
        got.sort_by(f64::total_cmp);
        assert_eq!(got, [48.0, 55.0]);
        notes.note_off(0, 48);
        h.set_notes(&notes);
        for _ in 0..48_000 {
            h.tick();
        }
        assert_eq!(h.mix.iter().filter(|m| m.active).count(), 1);
        assert_eq!(h.sounding(), 1u128 << 55);
    }

    #[test]
    fn chord_mode_snaps_intervals_to_the_chord() {
        let mut h = harm(HarmonyMode::Chord);
        h.settings.intervals = [2, 4, 0, 0];
        h.settings.chord = (1 << 2) | (1 << 5) | (1 << 9); // D minor
        h.apply_settings();
        // Lead on D: a third above is F, a fifth above A.
        h.lead(62.0, 0.005);
        assert_eq!(h.voice_note(0, 0.0), Some(65.0));
        assert_eq!(h.voice_note(1, 0.0), Some(69.0));
        // Lead on C (not in the chord): third above E -> F, fifth G -> A.
        h.lead_lost();
        h.lead(60.0, 0.005);
        assert_eq!(h.voice_note(0, 0.0), Some(65.0));
        assert_eq!(h.voice_note(1, 0.0), Some(69.0));
    }

    #[test]
    fn spread_pans_and_levels_stay_bounded() {
        let mut h = harm(HarmonyMode::Scale);
        h.settings.intervals = [2, -3, 4, 7];
        h.settings.spread = 1.0;
        h.apply_settings();
        for _ in 0..48_000 {
            h.tick();
        }
        let m = h.mix;
        assert!(m[0].gain[0] > m[0].gain[1]);
        assert!(m[1].gain[1] > m[1].gain[0]);
        let power: f32 = m
            .iter()
            .map(|v| v.gain[0].powi(2) + v.gain[1].powi(2))
            .sum();
        // Four voices: about twice one voice's power (2 per centred voice).
        assert!((power - 4.0).abs() < 0.5, "{power}");
    }
}
