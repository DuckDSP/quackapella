//! The wire between Quackapella Link and Quackapella.
//!
//! Link sits on another track (the instrumental bus, a synth) and sends
//! what passes through it; Quackapella receives it. Two plugins are two
//! libraries, and some hosts run them in two processes, so the audio
//! goes through a small region of shared memory, one per [`Role`]:
//! a header of atomics and a ring of the last [`FRAMES`] stereo frames.
//!
//! Both ends are wait-free: the sender stores samples and moves a
//! counter, the receiver copies and keeps its own position. Nothing is
//! allocated or locked after [`Channel::open`], which belongs to
//! `reset`, never to `process`.
//!
//! The voice never waits for the link: what arrives late is silence,
//! and only the linked sound is delayed (by the sender's block, see
//! [`Receiver`]).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

mod region;

use region::Mapping;

/// Stereo frames kept by a channel (a power of two): 340 ms at 48 kHz.
pub const FRAMES: usize = 16_384;
const MAGIC: u32 = 0x514B_4C4B; // "QKLK"
const VERSION: u32 = 1;

/// A suffix for the channels' names, for this process only.
static PRIVATE: OnceLock<String> = OnceLock::new();

/// Keep this process's channels to itself (tests): they then never
/// meet the plugins running on the machine, in a host or in another
/// test. To call before the first [`Channel::open`].
pub fn keep_private() {
    PRIVATE.get_or_init(|| format!("-{:x}", std::process::id()));
}

/// What the linked sound is for. One sender at a time per role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// The rest of the song, for the mixing assistant to hear.
    #[default]
    Mix,
    /// The vocoder's carrier (or the voice, with Voice from Side-chain).
    Carrier,
}

impl Role {
    pub const ALL: [Self; 2] = [Self::Mix, Self::Carrier];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        if i >= 0.5 { Self::Carrier } else { Self::Mix }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mix => "Mix",
            Self::Carrier => "Carrier",
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Mix => "dkds-quack-mix",
            Self::Carrier => "dkds-quack-carrier",
        }
    }
}

/// The shared region. Zeroed memory is a valid, empty one.
#[repr(C)]
pub(crate) struct Region {
    magic: AtomicU32,
    version: AtomicU32,
    /// The sender's sample rate, Hz, and its largest block, frames.
    sample_rate: AtomicU32,
    block: AtomicU32,
    /// The sender that holds the channel (0: nobody).
    owner: AtomicU64,
    /// Moves with every block sent: the sender is alive.
    beat: AtomicU64,
    /// Frames written since the channel was taken.
    write: AtomicU64,
    /// Moves with every block received: someone listens.
    heard: AtomicU64,
    data: [AtomicU32; 2 * FRAMES],
}

/// One role's region, mapped. Cheap to clone the handle is not: open
/// one per end.
pub struct Channel {
    map: Mapping,
}

impl Channel {
    /// The channel every plugin of this user shares for `role`. `None`
    /// when the system refuses shared memory (the link is then simply
    /// absent). Not for the audio thread.
    #[must_use]
    pub fn open(role: Role) -> Option<Self> {
        match PRIVATE.get() {
            Some(suffix) => Self::open_named(&format!("{}{suffix}", role.key())),
            None => Self::open_named(role.key()),
        }
    }

    /// A channel under another name (tests).
    #[must_use]
    pub fn open_named(name: &str) -> Option<Self> {
        let map = Mapping::open(name)?;
        let ch = Self { map };
        let r = ch.region();
        // First user: sign it. Someone else's layout: leave it alone.
        let _ = r
            .magic
            .compare_exchange(0, MAGIC, Ordering::AcqRel, Ordering::Acquire);
        let _ = r
            .version
            .compare_exchange(0, VERSION, Ordering::AcqRel, Ordering::Acquire);
        (r.magic.load(Ordering::Acquire) == MAGIC && r.version.load(Ordering::Acquire) == VERSION)
            .then_some(ch)
    }

    fn region(&self) -> &Region {
        self.map.region()
    }
}

/// What a [`Sender`] did with a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// On the wire.
    Sent,
    /// Another Link holds this role: nothing was sent.
    Busy,
}

/// The sending end (Quackapella Link's audio thread).
pub struct Sender {
    ch: Channel,
    id: u64,
    sample_rate: u32,
    /// The other owner's beat when last looked at, and for how many of
    /// our frames it has not moved.
    their_beat: u64,
    still: u64,
    /// The receivers' count when last looked at, and frames since it
    /// moved.
    heard: u64,
    deaf: u64,
}

impl Sender {
    /// `id` tells this sender from the others: any non-zero number of
    /// its own.
    #[must_use]
    pub fn new(ch: Channel, id: u64, sample_rate: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sample_rate = sample_rate.round().max(1.0) as u32;
        Self {
            ch,
            id: id.max(1),
            sample_rate,
            their_beat: 0,
            still: 0,
            heard: 0,
            deaf: u64::MAX / 2,
        }
    }

    /// Send a block (`gain` applied to the copy, not to `l` and `r`).
    /// Wait-free.
    pub fn send(&mut self, l: &[f32], r: &[f32], gain: f32) -> Sent {
        let reg = self.ch.region();
        let n = l.len().min(r.len());
        let owner = reg.owner.load(Ordering::Acquire);
        if owner != self.id {
            // Free, or held by a sender that stopped half a second ago.
            let beat = reg.beat.load(Ordering::Acquire);
            if beat == self.their_beat {
                self.still += n as u64;
            } else {
                self.their_beat = beat;
                self.still = 0;
            }
            let gone = owner == 0 || self.still > u64::from(self.sample_rate / 2);
            if !gone
                || reg
                    .owner
                    .compare_exchange(owner, self.id, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
            {
                return Sent::Busy;
            }
            reg.block.store(0, Ordering::Release);
        }
        reg.sample_rate.store(self.sample_rate, Ordering::Release);
        #[allow(clippy::cast_possible_truncation)]
        reg.block.fetch_max(n as u32, Ordering::AcqRel);
        let w = reg.write.load(Ordering::Acquire);
        for i in 0..n {
            #[allow(clippy::cast_possible_truncation)]
            let at = 2 * ((w as usize).wrapping_add(i) & (FRAMES - 1));
            reg.data[at].store((l[i] * gain).to_bits(), Ordering::Relaxed);
            reg.data[at + 1].store((r[i] * gain).to_bits(), Ordering::Relaxed);
        }
        reg.write.store(w.wrapping_add(n as u64), Ordering::Release);
        reg.beat.fetch_add(1, Ordering::AcqRel);
        Sent::Sent
    }

    /// Whether a Quackapella received within the last half second of
    /// the `frames` counted here (call once per block).
    pub fn listened(&mut self, frames: usize) -> bool {
        let h = self.ch.region().heard.load(Ordering::Acquire);
        if h == self.heard {
            self.deaf = self.deaf.saturating_add(frames as u64);
        } else {
            self.heard = h;
            self.deaf = 0;
        }
        self.deaf <= u64::from(self.sample_rate / 2)
    }

    /// Let go of the channel (bypass, removal): the next sender takes
    /// it at once.
    pub fn release(&self) {
        let _ = self.ch.region().owner.compare_exchange(
            self.id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.release();
    }
}

/// The receiving end (Quackapella's audio thread).
///
/// It reads a little behind the sender: by the sender's largest block
/// plus two of its own, so that a host that renders the other track in
/// big blocks (Logic does, for tracks that only play back) never
/// leaves it short. That delay is on the linked sound only.
pub struct Receiver {
    ch: Channel,
    sample_rate: u32,
    read: u64,
    synced: bool,
    /// Waiting for the sender to get a margin ahead (silence till then,
    /// never the ring's older contents).
    filling: bool,
    beat: u64,
    /// Frames of ours since the sender's beat moved.
    idle: u64,
    /// Largest block asked of us.
    block: usize,
}

impl Receiver {
    #[must_use]
    pub fn new(ch: Channel, sample_rate: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sample_rate = sample_rate.round().max(1.0) as u32;
        Self {
            ch,
            sample_rate,
            read: 0,
            synced: false,
            filling: false,
            beat: 0,
            idle: u64::MAX / 2,
            block: 0,
        }
    }

    /// The next block of the linked sound. `false` (and silence) when
    /// no Link sends at this sample rate. Wait-free.
    pub fn receive(&mut self, l: &mut [f32], r: &mut [f32]) -> bool {
        let reg = self.ch.region();
        let n = l.len().min(r.len());
        reg.heard.fetch_add(1, Ordering::AcqRel);
        let beat = reg.beat.load(Ordering::Acquire);
        if beat == self.beat {
            self.idle = self.idle.saturating_add(n as u64);
        } else {
            self.beat = beat;
            self.idle = 0;
        }
        let alive = reg.owner.load(Ordering::Acquire) != 0
            && self.idle <= u64::from(self.sample_rate / 2)
            && reg.sample_rate.load(Ordering::Acquire) == self.sample_rate;
        if !alive {
            self.synced = false;
            l[..n].fill(0.0);
            r[..n].fill(0.0);
            return false;
        }
        self.block = self.block.max(n);
        let w = reg.write.load(Ordering::Acquire);
        let margin =
            (reg.block.load(Ordering::Acquire) as usize + 2 * self.block).min(FRAMES / 2) as u64;
        let lag = w.wrapping_sub(self.read);
        // Out of step (start, sender restarted, fell too far behind):
        // take our place behind the sender again.
        if !self.synced || lag > (FRAMES as u64) - (n as u64) || lag > 2 * margin + n as u64 {
            self.read = w;
            self.synced = true;
            self.filling = true;
        }
        if self.filling {
            if w.wrapping_sub(self.read) < margin {
                l[..n].fill(0.0);
                r[..n].fill(0.0);
                return true;
            }
            self.read = w.wrapping_sub(margin);
            self.filling = false;
        }
        let have = w.wrapping_sub(self.read).min(n as u64) as usize;
        for i in 0..have {
            #[allow(clippy::cast_possible_truncation)]
            let at = 2 * ((self.read as usize).wrapping_add(i) & (FRAMES - 1));
            l[i] = f32::from_bits(reg.data[at].load(Ordering::Relaxed));
            r[i] = f32::from_bits(reg.data[at + 1].load(Ordering::Relaxed));
        }
        // Short: the rest is silence, and the place is taken again.
        l[have..n].fill(0.0);
        r[have..n].fill(0.0);
        self.read = self.read.wrapping_add(have as u64);
        if have < n {
            self.synced = false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(name: &str) -> (Sender, Receiver) {
        let name = format!("{name}-{}", std::process::id());
        let a = Channel::open_named(&name).expect("channel");
        let b = Channel::open_named(&name).expect("channel");
        (Sender::new(a, 7, 48_000.0), Receiver::new(b, 48_000.0))
    }

    /// A ramp, so that any gap or repeat shows.
    #[allow(clippy::cast_precision_loss)]
    fn ramp(from: usize, n: usize) -> Vec<f32> {
        (from..from + n).map(|i| i as f32).collect()
    }

    #[test]
    fn nothing_is_received_without_a_sender() {
        let (_tx, mut rx) = pair("qk-test-idle");
        let (mut l, mut r) = ([1.0; 64], [1.0; 64]);
        assert!(!rx.receive(&mut l, &mut r));
        assert!(l.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn small_blocks_arrive_whole_and_in_order() {
        let (mut tx, mut rx) = pair("qk-test-small");
        let mut got = Vec::new();
        for k in 0..200 {
            let x = ramp(k * 32, 32);
            assert_eq!(tx.send(&x, &x, 1.0), Sent::Sent);
            let (mut l, mut r) = ([0.0; 32], [0.0; 32]);
            assert!(rx.receive(&mut l, &mut r));
            got.extend_from_slice(&l);
        }
        // After the start, every sample follows the one before.
        let tail = &got[got.len() - 3200..];
        assert!(tail.windows(2).all(|w| (w[1] - w[0] - 1.0).abs() < 0.5));
        // And the delay is the margin: three blocks.
        let last = *tail.last().expect("samples");
        assert!((6400.0 - 1.0 - last - 96.0).abs() < 33.0, "{last}");
    }

    #[test]
    fn a_sender_in_big_blocks_leaves_no_gap() {
        let (mut tx, mut rx) = pair("qk-test-big");
        let mut got = Vec::new();
        for k in 0..40 {
            let x = ramp(k * 1024, 1024);
            tx.send(&x, &x, 1.0);
            for _ in 0..32 {
                let (mut l, mut r) = ([0.0; 32], [0.0; 32]);
                rx.receive(&mut l, &mut r);
                got.extend_from_slice(&l);
            }
        }
        let tail = &got[got.len() - 8192..];
        assert!(tail.windows(2).all(|w| (w[1] - w[0] - 1.0).abs() < 0.5));
    }

    #[test]
    fn a_second_sender_waits_then_takes_over() {
        let (mut tx, mut rx) = pair("qk-test-two");
        let name = format!("qk-test-two-{}", std::process::id());
        let mut other = Sender::new(Channel::open_named(&name).expect("channel"), 9, 48_000.0);
        let x = [0.5; 480];
        assert_eq!(tx.send(&x, &x, 1.0), Sent::Sent);
        assert_eq!(other.send(&x, &x, 1.0), Sent::Busy);
        // The first one stops: after half a second the other has it.
        let mut state = Sent::Busy;
        for _ in 0..60 {
            state = other.send(&x, &x, 1.0);
        }
        assert_eq!(state, Sent::Sent);
        // Released, it is taken at once.
        other.release();
        assert_eq!(tx.send(&x, &x, 1.0), Sent::Sent);
        let (mut l, mut r) = ([0.0; 32], [0.0; 32]);
        assert!(rx.receive(&mut l, &mut r));
        assert!(tx.listened(32));
    }

    #[test]
    fn another_sample_rate_is_not_received() {
        let (mut tx, _rx) = pair("qk-test-rate");
        let name = format!("qk-test-rate-{}", std::process::id());
        let mut rx = Receiver::new(Channel::open_named(&name).expect("channel"), 44_100.0);
        let x = [0.5; 64];
        tx.send(&x, &x, 1.0);
        let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
        assert!(!rx.receive(&mut l, &mut r));
    }
}
