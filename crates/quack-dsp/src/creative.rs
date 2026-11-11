//! Manipulator-style effects of the *Creative* mode (roadmap step 5).
//!
//! The pitch-synchronous parts (octave layers, freeze, formants that
//! follow the pitch) live in the shifter ([`crate::shift::Psola`]),
//! since they reuse its grains. Here:
//!
//! - [`Robot`]: a monotone voice. Each phrase is held on one note (the
//!   first one sung, after correction), or on the held MIDI note, so
//!   speech and melody come out flat, buzzy, on a single pitch.
//! - [`Glitch`]: tempo-synced slice effects on the processed voice. At
//!   each slice (1/4 to 1/32 note) a dice roll weighted by the amount
//!   picks pass, repeat, roll (stutter), reverse or half speed (tape
//!   drop). Slices are read from the voice just played, with short
//!   crossfades, so nothing clicks and the latency is unchanged.
//!
//! Real-time: buffers are allocated in `prepare`; `tick` does no
//! allocation and bounded work.

use crate::autotune::Retuner;
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
    /// Chance that a slice is glitched, 0 to 1 (0 = off).
    pub amount: f64,
    pub rate: GlitchRate,
    tempo: f64,
    /// Position in beats.
    beat: f64,
    slice: i64,
    cur: Reader,
    prev: Reader,
    fade: f64,
    rng: u32,
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
            tempo: 120.0,
            beat: 0.0,
            slice: 0,
            cur: Reader::default(),
            prev: Reader::default(),
            fade: 0.0,
            rng: 0x9E37_79B9,
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
        self.rng = 0x9E37_79B9;
    }

    /// Host tempo, and the beat position at the start of the block when
    /// the transport plays (slices then fall on the host's grid).
    pub fn set_transport(&mut self, tempo: f64, beat: Option<f64>) {
        if tempo.is_finite() && tempo > 0.0 {
            self.tempo = tempo.clamp(TEMPO_MIN, TEMPO_MAX);
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

    fn random(&mut self) -> f64 {
        // xorshift32: deterministic, cheap, good enough for dice.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        f64::from(x) / f64::from(u32::MAX)
    }

    fn start(&mut self, kind: Slice, len: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let l = len.round() as i64;
        let src = if kind == Slice::Pass {
            self.n
        } else {
            self.n - l
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
            let kind = if self.amount > 0.0 && self.random() < self.amount {
                match self.random() {
                    x if x < 0.3 => Slice::Repeat,
                    x if x < 0.6 => Slice::Roll,
                    x if x < 0.8 => Slice::Reverse,
                    _ => Slice::Half,
                }
            } else {
                Slice::Pass
            };
            self.start(kind, len);
        } else if self.amount <= 0.0 && self.cur.kind != Slice::Pass {
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
}

impl PitchControl for Steer<'_> {
    fn voiced(&mut self, period: f64, dt: f64) -> f64 {
        let ratio = self.retuner.voiced(period, dt);
        let transpose = self.retuner.transpose;
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
        let target = match r.note {
            Some(n) => n + transpose,
            None => r.phrase.unwrap_or(out),
        };
        let robot = ((target - sung) / 12.0).exp2();
        ratio.powf(1.0 - r.amount) * robot.powf(r.amount)
    }

    fn unvoiced(&mut self, dt: f64) {
        self.retuner.unvoiced(dt);
        self.robot.gap_s += dt.max(0.0);
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
        let mut g = Glitch {
            amount: 1.0,
            rate: GlitchRate::Sixteenth,
            ..Glitch::default()
        };
        g.set_transport(120.0, Some(0.0));
        // 1/16 at 120 bpm = 125 ms = 6000 samples at 48 kHz.
        let mut kinds = std::collections::HashSet::new();
        let mut changes = 0;
        let mut last = g.slice();
        let mut prev = 0.0f32;
        let mut max_step = 0.0f32;
        for k in 0..96_000 {
            #[allow(clippy::cast_precision_loss)]
            let x = 0.5 * (k as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin();
            let (y, _) = g.tick(x, x);
            if k > 6_000 {
                max_step = max_step.max((y - prev).abs());
            }
            prev = y;
            if g.slice() != last || k % 6_000 == 0 {
                kinds.insert(g.slice());
            }
            if g.slice() != last {
                changes += 1;
                last = g.slice();
            }
        }
        assert!(kinds.len() >= 3, "{kinds:?}");
        assert!(changes <= 16, "{changes}");
        // A 220 Hz sine at 0.5 steps by at most ~0.015 per sample; the
        // crossfades keep every splice well below a click.
        assert!(max_step < 0.06, "step {max_step}");
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
        let mut s = Steer {
            retuner: &mut t,
            robot: &mut r,
        };
        // Glide from A3 (220 Hz) up to C4: the output stays on A3.
        for k in 0..200 {
            let hz = 220.0 * (f64::from(k) / 200.0 * 3.0 / 12.0).exp2();
            let period = 48_000.0 / hz;
            let out = 48_000.0 / period * s.voiced(period, 1.0 / hz);
            assert!((out - 220.0).abs() < 0.5, "{out}");
        }
        // A pause, then a new phrase on E4.
        s.unvoiced(0.1);
        let period = 48_000.0 / 329.63;
        let out = 329.63 * s.voiced(period, 0.003);
        assert!((out - 329.63).abs() < 0.5, "{out}");
        // A held MIDI note wins.
        s.robot.note = Some(60.0);
        let out = 329.63 * s.voiced(period, 0.003);
        assert!((out - 261.63).abs() < 0.5, "{out}");
    }
}
