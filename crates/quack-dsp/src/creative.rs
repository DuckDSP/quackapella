//! Manipulator-style effects of the *Creative* mode (roadmap step 5).
//!
//! The pitch-synchronous parts (octave layers, freeze, formants that
//! follow the pitch) live in the shifter ([`crate::shift::Psola`]),
//! since they reuse its grains. Here:
//!
//! - [`Robot`]: a monotone voice. Each phrase is held on one note (the
//!   first one sung, after correction), or on the held MIDI note, so
//!   speech and melody come out flat, buzzy, on a single pitch.
//! - [`Glitch`]: tempo-synced slice effects on the processed voice,
//!   with nothing left to chance. The effect is chosen (repeat, stutter,
//!   reverse, tape drop) and the amount is a density: it glitches that
//!   share of the steps of each bar, always the same ones (the last step
//!   first, like a fill, then spread over the bar), so a bar sounds the
//!   same every time and turning the knob up only adds steps. *Hold*
//!   glitches every step while it is on (a live button). Consecutive
//!   glitched steps keep the slice they caught (a beat repeat, not a
//!   delay). Slices are read from the voice just played, with short
//!   crossfades, so nothing clicks and the latency is unchanged.
//!
//! Real-time: buffers are allocated in `prepare`; `tick` does no
//! allocation and bounded work.

use crate::autotune::Retuner;
use crate::harmony::Harmonizer;
use crate::shift::PitchControl;

/// Glitch slice length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlitchRate {
    Quarter,
    Eighth,
    #[default]
    Sixteenth,
    ThirtySecond,
}

impl GlitchRate {
    pub const COUNT: u32 = 4;
    pub const ALL: [Self; 4] = [
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
    ];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
        }
    }

    /// Slice length in beats (quarter notes).
    #[must_use]
    pub const fn beats(self) -> f64 {
        match self {
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
        }
    }
}

/// The effect of the glitched steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlitchMode {
    #[default]
    Repeat,
    Stutter,
    Reverse,
    Tape,
}

impl GlitchMode {
    pub const COUNT: u32 = 4;
    pub const ALL: [Self; 4] = [Self::Repeat, Self::Stutter, Self::Reverse, Self::Tape];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Repeat => "Repeat",
            Self::Stutter => "Stutter",
            Self::Reverse => "Reverse",
            Self::Tape => "Tape",
        }
    }

    const fn slice(self) -> Slice {
        match self {
            Self::Repeat => Slice::Repeat,
            Self::Stutter => Slice::Roll,
            Self::Reverse => Slice::Reverse,
            Self::Tape => Slice::Half,
        }
    }
}

/// Whether step `step` of `steps` is glitched at `amount` (0 to 1).
/// Steps are ranked once: the last of the bar first, then the others
/// spread over the bar (bit-reversed order from the end). The first
/// `amount × steps` of that ranking are glitched, so a higher amount only
/// adds steps.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn glitch_step(step: u32, steps: u32, amount: f64) -> bool {
    let steps = steps.max(1);
    let hits = (amount.clamp(0.0, 1.0) * f64::from(steps)).round() as u32;
    if hits == 0 {
        return false;
    }
    // Rank of the step: position, counted from the end of the bar, in
    // bit-reversed order over the next power of two (skipping the
    // positions past the bar).
    let from_end = steps - 1 - step.min(steps - 1);
    let bits = steps.next_power_of_two().trailing_zeros();
    let rev = |x: u32| {
        if bits == 0 {
            0
        } else {
            x.reverse_bits() >> (32 - bits)
        }
    };
    let rank = (0..steps.next_power_of_two())
        .map(rev)
        .filter(|&p| p < steps)
        .position(|p| p == from_end)
        .unwrap_or(0) as u32;
    rank < hits
}

/// What a glitch slice plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Slice {
    /// The live voice.
    #[default]
    Pass,
    /// The previous slice again.
    Repeat,
    /// The start of the previous slice, looped four times per slice.
    Roll,
    /// The previous slice backwards.
    Reverse,
    /// The previous slice at half speed (an octave down, tape drop).
    Half,
}

#[derive(Debug, Clone, Copy, Default)]
struct Reader {
    kind: Slice,
    /// Absolute input index the slice reads from.
    src: i64,
    /// Absolute index where the slice started.
    t0: i64,
    len: f64,
}

/// Slowest and fastest tempo followed.
const TEMPO_MIN: f64 = 30.0;
const TEMPO_MAX: f64 = 300.0;
/// Slice crossfade and roll loop crossfade.
const FADE_S: f64 = 0.003;
/// Longest slice kept (a quarter note at the slowest tempo) plus margin.
const BUFFER_S: f64 = 4.5;

/// Tempo-synced slice effects. See the module docs.
#[derive(Debug, Clone)]
pub struct Glitch {
    sample_rate: f64,
    buf: [Vec<f32>; 2],
    mask: usize,
    /// Next input index.
    n: i64,
    /// Share of the steps of a bar that are glitched, 0 to 1 (0 = off).
    pub amount: f64,
    pub rate: GlitchRate,
    pub mode: GlitchMode,
    /// Glitch every step while on.
    pub hold: bool,
    tempo: f64,
    /// Bar length in beats (quarter notes).
    bar_beats: f64,
    /// Position in beats.
    beat: f64,
    slice: i64,
    cur: Reader,
    prev: Reader,
    fade: f64,
}

impl Default for Glitch {
    fn default() -> Self {
        let mut g = Self {
            sample_rate: 48_000.0,
            buf: [Vec::new(), Vec::new()],
            mask: 0,
            n: 0,
            amount: 0.0,
            rate: GlitchRate::default(),
            mode: GlitchMode::default(),
            hold: false,
            tempo: 120.0,
            bar_beats: 4.0,
            beat: 0.0,
            slice: 0,
            cur: Reader::default(),
            prev: Reader::default(),
            fade: 0.0,
        };
        g.prepare(48_000.0);
        g
    }
}

impl Glitch {
    /// Allocates. Not for the audio thread.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8_000.0);
        let len = ((self.sample_rate * BUFFER_S) as usize).next_power_of_two();
        self.buf = [vec![0.0; len], vec![0.0; len]];
        self.mask = len - 1;
        self.fade = (self.sample_rate * FADE_S).max(1.0);
        self.reset();
    }

    /// Clears audio state. Real-time safe.
    pub fn reset(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.n = 0;
        self.beat = 0.0;
        self.slice = 0;
        self.cur = Reader::default();
        self.prev = Reader::default();
    }

    /// Host tempo, bar length in beats, and the beat position at the
    /// start of the block when the transport plays (slices then fall on
    /// the host's grid and bars).
    pub fn set_transport(&mut self, tempo: f64, bar_beats: f64, beat: Option<f64>) {
        if tempo.is_finite() && tempo > 0.0 {
            self.tempo = tempo.clamp(TEMPO_MIN, TEMPO_MAX);
        }
        if bar_beats.is_finite() && bar_beats > 0.0 {
            self.bar_beats = bar_beats.clamp(0.5, 32.0);
        }
        if let Some(b) = beat.filter(|b| b.is_finite())
            && (b - self.beat).abs() > 0.01
        {
            self.beat = b;
        }
    }

    /// What the current slice plays (for tests and the editor).
    #[must_use]
    pub fn slice(&self) -> Slice {
        self.cur.kind
    }

    fn start(&mut self, kind: Slice, len: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let l = len.round() as i64;
        let (src, len) = if kind == Slice::Pass {
            (self.n, len)
        } else if self.cur.kind != Slice::Pass {
            // Glitched steps in a row keep the slice they caught.
            (self.cur.src, self.cur.len)
        } else {
            (self.n - l, len)
        };
        self.prev = self.cur;
        self.cur = Reader {
            kind,
            src,
            t0: self.n,
            len,
        };
    }

    /// Absolute (fractional) read position of reader `r` at time `n`.
    #[allow(clippy::cast_precision_loss)]
    fn read(&self, r: &Reader, n: i64, ch: usize) -> f32 {
        let t = (n - r.t0) as f64;
        let at = |x: f64| self.at(ch, x);
        match r.kind {
            Slice::Pass | Slice::Repeat => at((r.src + n - r.t0) as f64),
            Slice::Reverse => at(r.src as f64 + r.len - 1.0 - t),
            Slice::Half => at(r.src as f64 + 0.5 * t),
            Slice::Roll => {
                let seg = (0.25 * r.len).max(2.0 * self.fade);
                let u = t % seg;
                let base = r.src as f64 + u;
                if t >= seg && u < self.fade {
                    let x = (u / self.fade) as f32;
                    x * at(base) + (1.0 - x) * at(base + seg)
                } else {
                    at(base)
                }
            }
        }
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn at(&self, ch: usize, x: f64) -> f32 {
        let i = x.floor();
        let t = (x - i) as f32;
        let i = i as i64;
        let b = &self.buf[ch];
        let a = b[(i as usize) & self.mask];
        if t == 0.0 {
            return a;
        }
        let c = b[((i + 1) as usize) & self.mask];
        a + t * (c - a)
    }

    /// One stereo sample in, one out (same time: no added latency).
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        let i = (self.n as usize) & self.mask;
        self.buf[0][i] = l;
        self.buf[1][i] = r;
        let per_beat = self.sample_rate * 60.0 / self.tempo;
        let slice_beats = self.rate.beats();
        let idx = (self.beat / slice_beats).floor() as i64;
        let len = slice_beats * per_beat;
        if idx != self.slice {
            self.slice = idx;
            let steps = ((self.bar_beats / slice_beats).round() as i64).max(1);
            let step = idx.rem_euclid(steps) as u32;
            let hit = self.hold || glitch_step(step, steps as u32, self.amount);
            let kind = if hit { self.mode.slice() } else { Slice::Pass };
            self.start(kind, len);
        } else if !self.hold && self.amount <= 0.0 && self.cur.kind != Slice::Pass {
            // Turned off mid-slice: back to the live voice now.
            self.start(Slice::Pass, len);
        }
        self.beat += 1.0 / per_beat;
        let n = self.n;
        self.n += 1;
        let since = (n - self.cur.t0) as f64;
        if self.cur.kind == Slice::Pass && (self.prev.kind == Slice::Pass || since >= self.fade) {
            return (l, r);
        }
        let a = (self.read(&self.cur, n, 0), self.read(&self.cur, n, 1));
        if since >= self.fade {
            return a;
        }
        let x = (since / self.fade) as f32;
        let b = (self.read(&self.prev, n, 0), self.read(&self.prev, n, 1));
        (b.0 + x * (a.0 - b.0), b.1 + x * (a.1 - b.1))
    }
}

/// Phrase gap after which the robot picks a new note.
const ROBOT_PHRASE_GAP_S: f64 = 0.06;

/// Monotone voice. See the module docs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Robot {
    /// 0 (off) to 1 (one flat note).
    pub amount: f64,
    /// Held MIDI note, if any: the robot sings it.
    pub note: Option<f64>,
    sample_rate: f64,
    phrase: Option<f64>,
    gap_s: f64,
}

impl Robot {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8_000.0);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.phrase = None;
        self.gap_s = 0.0;
    }
}

/// The autotune, then the robot, deciding each grain's ratio.
pub struct Steer<'a> {
    pub retuner: &'a mut Retuner,
    pub robot: &'a mut Robot,
    /// Harmony voices, told the lead's note at each grain.
    pub harmony: &'a mut Harmonizer,
}

impl Steer<'_> {
    /// The lead's ratio (the autotune's), and the robot's phrase note
    /// kept for the creative copy.
    fn lead_ratio(&mut self, period: f64, dt: f64) -> f64 {
        let ratio = self.retuner.voiced(period, dt);
        let r = &mut *self.robot;
        let gap = std::mem::take(&mut r.gap_s);
        if r.amount <= 0.0 || period <= 0.0 {
            r.phrase = None;
            return ratio;
        }
        let sung = crate::pitch::hz_to_midi_f64(r.sample_rate / period);
        let out = sung + 12.0 * ratio.log2();
        if r.phrase.is_none() || gap > ROBOT_PHRASE_GAP_S {
            r.phrase = Some(out.round());
        }
        ratio
    }
}

impl PitchControl for Steer<'_> {
    fn voiced(&mut self, period: f64, dt: f64) -> f64 {
        let ratio = self.lead_ratio(period, dt);
        if self.harmony.enabled() && period > 0.0 {
            let sung = crate::pitch::hz_to_midi_f64(self.robot.sample_rate / period);
            self.harmony.lead(sung + 12.0 * ratio.log2(), dt);
        }
        ratio
    }

    fn unvoiced(&mut self, dt: f64) {
        self.retuner.unvoiced(dt);
        self.robot.gap_s += dt.max(0.0);
        self.harmony.unvoiced(dt);
    }

    fn voice(&mut self, v: usize, dt: f64) -> Option<f64> {
        self.harmony.voice_note(v, dt)
    }

    /// The robot sings the creative copy: the held MIDI note (with the
    /// transposition) or one note per phrase, blended with the lead's
    /// by its amount.
    fn copy(&mut self, lead: f64, _dt: f64) -> f64 {
        let r = &*self.robot;
        if r.amount <= 0.0 {
            return lead;
        }
        let target = match r.note {
            Some(n) => n + self.retuner.transpose,
            None => r.phrase.unwrap_or(lead),
        };
        lead + (target - lead) * r.amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glitch_off_is_transparent() {
        let mut g = Glitch::default();
        for k in 0..100_000 {
            #[allow(clippy::cast_precision_loss)]
            let x = (k as f32 * 0.01).sin();
            assert_eq!(g.tick(x, -x), (x, -x));
        }
    }

    #[test]
    fn glitch_slices_follow_the_tempo_and_never_click() {
        // 1/16 at 120 bpm = 125 ms = 6000 samples at 48 kHz.
        for mode in GlitchMode::ALL {
            let mut g = Glitch {
                amount: 0.25,
                rate: GlitchRate::Sixteenth,
                mode,
                ..Glitch::default()
            };
            g.set_transport(120.0, 4.0, Some(0.0));
            let mut prev = 0.0f32;
            let mut max_step = 0.0f32;
            let mut bars = [[false; 16]; 3];
            for k in 0..288_000 {
                #[allow(clippy::cast_precision_loss)]
                let x = 0.5 * (k as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin();
                let (y, _) = g.tick(x, x);
                if k > 6_000 {
                    max_step = max_step.max((y - prev).abs());
                }
                prev = y;
                if k % 6_000 == 3_000 {
                    assert!(matches!(g.slice(), Slice::Pass) || g.slice() == mode.slice());
                    bars[k / 96_000][(k % 96_000) / 6_000] = g.slice() != Slice::Pass;
                }
            }
            // The same four steps of every bar, the last one included.
            assert_eq!(bars[0], bars[1]);
            assert_eq!(bars[1], bars[2]);
            assert_eq!(bars[0].iter().filter(|&&b| b).count(), 4);
            assert!(bars[0][15]);
            // A 220 Hz sine at 0.5 steps by at most ~0.015 per sample; the
            // crossfades keep every splice well below a click.
            assert!(max_step < 0.06, "{mode:?}: step {max_step}");
        }
    }

    #[test]
    fn glitch_density_only_adds_steps() {
        for steps in [4, 8, 16, 32, 12] {
            let mut was = vec![false; steps as usize];
            for k in 0..=steps {
                let amount = f64::from(k) / f64::from(steps);
                let now: Vec<bool> = (0..steps).map(|s| glitch_step(s, steps, amount)).collect();
                assert_eq!(now.iter().filter(|&&b| b).count() as u32, k);
                assert!(was.iter().zip(&now).all(|(a, b)| !a || *b));
                was = now;
            }
        }
    }

    #[test]
    fn glitch_hold_catches_one_slice() {
        let mut g = Glitch {
            hold: true,
            mode: GlitchMode::Repeat,
            ..Glitch::default()
        };
        g.set_transport(120.0, 4.0, Some(0.0));
        // A ramp: while held, the output keeps replaying the same slice.
        let out: Vec<f32> = (0..60_000)
            .map(|k| {
                #[allow(clippy::cast_precision_loss)]
                let x = k as f32 / 60_000.0;
                g.tick(x, x).0
            })
            .collect();
        let a = out[20_000];
        let b = out[20_000 + 6_000];
        let c = out[20_000 + 18_000];
        assert!((a - b).abs() < 1e-4 && (a - c).abs() < 1e-4, "{a} {b} {c}");
    }

    #[test]
    fn robot_holds_one_note_per_phrase() {
        let mut t = Retuner::default();
        t.prepare(48_000.0);
        let mut r = Robot {
            amount: 1.0,
            ..Robot::default()
        };
        r.prepare(48_000.0);
        let mut h = Harmonizer::default();
        let mut s = Steer {
            retuner: &mut t,
            robot: &mut r,
            harmony: &mut h,
        };
        // The copy's pitch for a grain of `period`, in Hz.
        let sing = |s: &mut Steer, period: f64, dt: f64| {
            let ratio = s.voiced(period, dt);
            let lead = crate::pitch::hz_to_midi_f64(48_000.0 / period) + 12.0 * ratio.log2();
            440.0 * ((s.copy(lead, dt) - 69.0) / 12.0).exp2()
        };
        // Glide from A3 (220 Hz) up to C4: the copy stays on A3.
        for k in 0..200 {
            let hz = 220.0 * (f64::from(k) / 200.0 * 3.0 / 12.0).exp2();
            let out = sing(&mut s, 48_000.0 / hz, 1.0 / hz);
            assert!((out - 220.0).abs() < 0.5, "{out}");
        }
        // A pause, then a new phrase on E4.
        s.unvoiced(0.1);
        let period = 48_000.0 / 329.63;
        let out = sing(&mut s, period, 0.003);
        assert!((out - 329.63).abs() < 0.5, "{out}");
        // A held MIDI note wins.
        s.robot.note = Some(60.0);
        let out = sing(&mut s, period, 0.003);
        assert!((out - 261.63).abs() < 0.5, "{out}");
    }
}
