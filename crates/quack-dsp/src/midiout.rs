//! The voice as MIDI: notes sung (or heard) sent to a synth.
//!
//! [`Tracker`] turns the engine's readings, once per block, into note
//! on/off and pitch-bend messages. No allocation: the messages go into a
//! fixed [`MidiBuf`] the adapter drains to the host's MIDI output (and,
//! on macOS, to a virtual MIDI source).
//!
//! Timing: a note starts when the pitch detector locks on the voice,
//! which takes a few periods of the voice (about 10 to 25 ms after the
//! onset, as for the autotune). It adds no audio latency.
//!
//! Sources ([`OutSource`]):
//! - Sung: the nearest note to the detected pitch, with hysteresis
//!   (a change needs more than [`HYSTERESIS`] of a semitone, held
//!   [`CHANGE_MS`]) so a vibrato does not retrigger.
//! - Sung + Bend: the same, plus pitch bend (±2 semitones, the usual
//!   synth default) following the voice between notes: vibrato, slides.
//! - Heard: the autotune's target plus the Pitch transposition, what
//!   the lead voice plays; the detected pitch when the correction is off.
//! - Heard + Harmony: also the notes the harmony voices sing.

/// Fraction of a semitone the voice must leave its note by to change.
pub const HYSTERESIS: f32 = 0.65;
/// How long a new note must hold before it replaces the current one.
pub const CHANGE_MS: f64 = 15.0;
/// Silence that ends a note (bridges short detector dropouts).
pub const RELEASE_MS: f64 = 40.0;
/// Pitch-bend range assumed on the receiving synth, in semitones.
pub const BEND_RANGE: f32 = 2.0;
const BEND_CENTRE: u16 = 8192;

/// What the MIDI output plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutSource {
    #[default]
    Sung,
    SungBend,
    Heard,
    HeardHarmony,
}

impl OutSource {
    pub const COUNT: u32 = 4;
    pub const ALL: [Self; 4] = [Self::Sung, Self::SungBend, Self::Heard, Self::HeardHarmony];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = i.max(0.0).round() as usize;
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sung => "Sung",
            Self::SungBend => "Sung + Bend",
            Self::Heard => "Heard",
            Self::HeardHarmony => "Heard + Harmony",
        }
    }
}

/// One MIDI 1.0 channel message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Msg {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl Msg {
    #[must_use]
    pub const fn note_on(ch: u8, note: u8, vel: u8) -> Self {
        Self {
            status: 0x90 | (ch & 15),
            data1: note & 127,
            data2: vel & 127,
        }
    }

    #[must_use]
    pub const fn note_off(ch: u8, note: u8) -> Self {
        Self {
            status: 0x80 | (ch & 15),
            data1: note & 127,
            data2: 0,
        }
    }

    #[must_use]
    pub const fn bend(ch: u8, value: u16) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        Self {
            status: 0xE0 | (ch & 15),
            data1: (value & 127) as u8,
            data2: ((value >> 7) & 127) as u8,
        }
    }

    /// Bytes on the wire.
    #[must_use]
    pub const fn bytes(self) -> [u8; 3] {
        [self.status, self.data1, self.data2]
    }

    /// Packed into a `u32` (for lock-free queues), and back.
    #[must_use]
    pub const fn pack(self) -> u32 {
        (self.status as u32) << 16 | (self.data1 as u32) << 8 | self.data2 as u32
    }

    #[must_use]
    pub const fn unpack(v: u32) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        Self {
            status: (v >> 16) as u8,
            data1: (v >> 8) as u8,
            data2: v as u8,
        }
    }
}

/// Messages of one block (fixed capacity, extra messages dropped).
#[derive(Debug, Clone)]
pub struct MidiBuf {
    msgs: [Msg; Self::CAPACITY],
    len: usize,
}

impl Default for MidiBuf {
    fn default() -> Self {
        Self {
            msgs: [Msg::default(); Self::CAPACITY],
            len: 0,
        }
    }
}

impl MidiBuf {
    pub const CAPACITY: usize = 160;

    pub fn push(&mut self, m: Msg) {
        if self.len < Self::CAPACITY {
            self.msgs[self.len] = m;
            self.len += 1;
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Msg] {
        &self.msgs[..self.len]
    }
}

/// The engine's readings for one block.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reading {
    /// Detected pitch, fractional MIDI note (`None` while unvoiced).
    pub sung: Option<f32>,
    /// Autotune target, fractional MIDI note (`None` when off).
    pub target: Option<f32>,
    /// Pitch + Fine transposition, semitones.
    pub transpose: f32,
    /// Notes the harmony voices sing (bit n = note n).
    pub harmony: u128,
    /// Input peak of the block, linear.
    pub peak: f32,
}

/// Turns readings into notes. See the module docs.
#[derive(Debug, Clone)]
pub struct Tracker {
    sample_rate: f64,
    channel: u8,
    lead: Option<u8>,
    /// Candidate replacement and how long it has held, in samples.
    candidate: Option<(u8, f64)>,
    silent: f64,
    harmony: u128,
    bend: u16,
    /// Level envelope (linear peak, fast attack, slow release).
    env: f32,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            channel: 0,
            lead: None,
            candidate: None,
            silent: 0.0,
            harmony: 0,
            bend: BEND_CENTRE,
            env: 0.0,
        }
    }
}

/// Velocity from a linear peak: -48 dBFS → 1, -6 dBFS and up → 127.
#[must_use]
pub fn velocity(peak: f32) -> u8 {
    let db = 20.0 * peak.max(1e-6).log10();
    let t = ((db + 48.0) / 42.0).clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (1.0 + t * 126.0).round() as u8;
    v
}

fn to_note(p: f32) -> Option<u8> {
    let n = p.round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (0.0..=127.0).contains(&n).then_some(n as u8)
}

impl Tracker {
    pub fn set_sample_rate(&mut self, sr: f64) {
        self.sample_rate = sr.max(1.0);
    }

    /// The lead note sounding now.
    #[must_use]
    pub fn lead(&self) -> Option<u8> {
        self.lead
    }

    /// Every note sounding now (lead and harmony), bit n = note n.
    #[must_use]
    pub fn sounding(&self) -> u128 {
        self.harmony | self.lead.map_or(0, |n| 1u128 << n)
    }

    /// Release everything (MIDI out switched off, channel change, reset).
    pub fn all_off(&mut self, out: &mut MidiBuf) {
        let ch = self.channel;
        if let Some(n) = self.lead.take() {
            out.push(Msg::note_off(ch, n));
        }
        let mut h = std::mem::take(&mut self.harmony);
        while h != 0 {
            #[allow(clippy::cast_possible_truncation)]
            let n = h.trailing_zeros() as u8;
            out.push(Msg::note_off(ch, n));
            h &= h - 1;
        }
        if self.bend != BEND_CENTRE {
            self.bend = BEND_CENTRE;
            out.push(Msg::bend(ch, BEND_CENTRE));
        }
        self.candidate = None;
    }

    /// One block of `frames` samples.
    pub fn update(
        &mut self,
        r: &Reading,
        source: OutSource,
        channel: u8,
        frames: usize,
        out: &mut MidiBuf,
    ) {
        if channel != self.channel {
            self.all_off(out);
            self.channel = channel;
        }
        let ch = self.channel;
        #[allow(clippy::cast_precision_loss)]
        let frames = frames as f64;
        // Level: instant attack, ~100 ms release.
        #[allow(clippy::cast_possible_truncation)]
        let rel = (-frames / (0.1 * self.sample_rate)).exp() as f32;
        self.env = r.peak.max(self.env * rel);

        let heard = matches!(source, OutSource::Heard | OutSource::HeardHarmony);
        let pitch = if heard {
            r.target.or(r.sung).map(|p| p + r.transpose)
        } else {
            r.sung
        };

        match pitch {
            None => {
                self.silent += frames;
                self.candidate = None;
                if self.silent >= RELEASE_MS * 1e-3 * self.sample_rate
                    && let Some(n) = self.lead.take()
                {
                    out.push(Msg::note_off(ch, n));
                    if self.bend != BEND_CENTRE {
                        self.bend = BEND_CENTRE;
                        out.push(Msg::bend(ch, BEND_CENTRE));
                    }
                }
            }
            Some(p) => {
                self.silent = 0.0;
                self.track(p, heard, source == OutSource::SungBend, frames, out);
            }
        }

        let want = if source == OutSource::HeardHarmony && self.lead.is_some() {
            r.harmony & !self.lead.map_or(0, |n| 1u128 << n)
        } else {
            0
        };
        self.harmony_to(want, out);
    }

    fn track(&mut self, p: f32, heard: bool, bend: bool, frames: f64, out: &mut MidiBuf) {
        let ch = self.channel;
        let Some(near) = to_note(p) else {
            return;
        };
        let vel = velocity(self.env);
        match self.lead {
            None => {
                self.candidate = None;
                if bend {
                    self.send_bend(p - f32::from(near), out);
                }
                out.push(Msg::note_on(ch, near, vel));
                self.lead = Some(near);
            }
            Some(l) => {
                // Heard notes are already settled by the autotune: a
                // change of target is a new note at once.
                let off = (p - f32::from(l)).abs();
                let leaving = if heard { near != l } else { off > HYSTERESIS };
                if leaving {
                    let held = match self.candidate {
                        Some((c, t)) if c == near => t + frames,
                        _ => frames,
                    };
                    let need = if heard {
                        0.0
                    } else {
                        CHANGE_MS * 1e-3 * self.sample_rate
                    };
                    if held >= need {
                        self.candidate = None;
                        // New note first, then the old one off: a mono
                        // synth slides (legato), a poly one retriggers.
                        if bend {
                            self.send_bend(p - f32::from(near), out);
                        }
                        out.push(Msg::note_on(ch, near, vel));
                        out.push(Msg::note_off(ch, l));
                        self.lead = Some(near);
                        return;
                    }
                    self.candidate = Some((near, held));
                } else {
                    self.candidate = None;
                }
                if bend {
                    self.send_bend(p - f32::from(l), out);
                }
            }
        }
    }

    fn send_bend(&mut self, semis: f32, out: &mut MidiBuf) {
        let t = (semis / BEND_RANGE).clamp(-1.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (f32::from(BEND_CENTRE) + t * 8191.0)
            .round()
            .clamp(0.0, 16383.0) as u16;
        // About 1 cent of change before a new message.
        if v.abs_diff(self.bend) >= 41 {
            self.bend = v;
            out.push(Msg::bend(self.channel, v));
        }
    }

    fn harmony_to(&mut self, want: u128, out: &mut MidiBuf) {
        let ch = self.channel;
        let vel = velocity(self.env);
        let mut gone = self.harmony & !want;
        while gone != 0 {
            #[allow(clippy::cast_possible_truncation)]
            let n = gone.trailing_zeros() as u8;
            out.push(Msg::note_off(ch, n));
            gone &= gone - 1;
        }
        let mut new = want & !self.harmony;
        while new != 0 {
            #[allow(clippy::cast_possible_truncation)]
            let n = new.trailing_zeros() as u8;
            out.push(Msg::note_on(ch, n, vel));
            new &= new - 1;
        }
        self.harmony = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;
    const BLOCK: usize = 64;

    fn run(t: &mut Tracker, r: Reading, source: OutSource, blocks: usize) -> Vec<Msg> {
        let mut all = Vec::new();
        let mut buf = MidiBuf::default();
        for _ in 0..blocks {
            buf.clear();
            t.update(&r, source, 0, BLOCK, &mut buf);
            all.extend_from_slice(buf.as_slice());
        }
        all
    }

    fn sung(p: f32) -> Reading {
        Reading {
            sung: Some(p),
            peak: 0.3,
            ..Reading::default()
        }
    }

    fn tracker() -> Tracker {
        let mut t = Tracker::default();
        t.set_sample_rate(SR);
        t
    }

    #[test]
    fn a_sung_note_starts_at_once_and_ends_after_the_release() {
        let mut t = tracker();
        let on = run(&mut t, sung(60.2), OutSource::Sung, 1);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].status, 0x90);
        assert_eq!(on[0].data1, 60);
        assert!(on[0].data2 > 90, "velocity {}", on[0].data2);
        // A short dropout (20 ms) keeps the note.
        assert!(run(&mut t, Reading::default(), OutSource::Sung, 15).is_empty());
        assert!(run(&mut t, sung(60.0), OutSource::Sung, 1).is_empty());
        // A real silence ends it.
        let off = run(&mut t, Reading::default(), OutSource::Sung, 40);
        assert_eq!(off, vec![Msg::note_off(0, 60)]);
    }

    #[test]
    fn vibrato_does_not_retrigger_but_a_new_note_does() {
        let mut t = tracker();
        run(&mut t, sung(60.0), OutSource::Sung, 1);
        for i in 0..200 {
            #[allow(clippy::cast_precision_loss)]
            let p = 60.0 + 0.6 * (i as f32 * 0.3).sin();
            assert!(run(&mut t, sung(p), OutSource::Sung, 1).is_empty());
        }
        // A brief overshoot is not a note.
        assert!(run(&mut t, sung(62.0), OutSource::Sung, 5).is_empty());
        assert!(run(&mut t, sung(60.0), OutSource::Sung, 1).is_empty());
        // Held past CHANGE_MS, it is: on first, then off.
        let m = run(&mut t, sung(62.0), OutSource::Sung, 20);
        assert_eq!(
            m,
            vec![Msg::note_on(0, 62, m[0].data2), Msg::note_off(0, 60)]
        );
    }

    #[test]
    fn bend_follows_the_voice_around_its_note() {
        let mut t = tracker();
        let m = run(&mut t, sung(60.4), OutSource::SungBend, 1);
        assert_eq!(m[0].status, 0xE0);
        let v = u16::from(m[0].data1) | u16::from(m[0].data2) << 7;
        // +0.4 semitone of a ±2 range: a fifth of the way up.
        assert!((i32::from(v) - (8192 + 1638)).abs() < 3, "{v}");
        assert_eq!(m[1].status, 0x90);
    }

    #[test]
    fn heard_plays_the_target_transposed_and_the_harmony() {
        let mut t = tracker();
        let r = Reading {
            sung: Some(60.4),
            target: Some(60.0),
            transpose: 12.0,
            harmony: 1 << 76 | 1 << 79,
            peak: 0.5,
        };
        let m = run(&mut t, r, OutSource::HeardHarmony, 1);
        let ons: Vec<u8> = m
            .iter()
            .filter(|m| m.status == 0x90)
            .map(|m| m.data1)
            .collect();
        assert_eq!(ons, vec![72, 76, 79]);
        assert_eq!(t.sounding().count_ones(), 3);
        let mut buf = MidiBuf::default();
        t.all_off(&mut buf);
        assert_eq!(buf.as_slice().len(), 3);
        assert_eq!(t.sounding(), 0);
    }

    #[test]
    fn a_channel_change_releases_the_old_channel() {
        let mut t = tracker();
        run(&mut t, sung(64.0), OutSource::Sung, 1);
        let mut buf = MidiBuf::default();
        t.update(&sung(64.0), OutSource::Sung, 3, BLOCK, &mut buf);
        assert_eq!(
            buf.as_slice(),
            &[
                Msg::note_off(0, 64),
                Msg::note_on(3, 64, buf.as_slice()[1].data2)
            ]
        );
    }

    #[test]
    fn messages_pack_round_trip() {
        for m in [
            Msg::note_on(5, 61, 99),
            Msg::bend(15, 16383),
            Msg::note_off(0, 0),
        ] {
            assert_eq!(Msg::unpack(m.pack()), m);
        }
        assert_eq!(velocity(1.0), 127);
        assert_eq!(velocity(0.0), 1);
    }
}
