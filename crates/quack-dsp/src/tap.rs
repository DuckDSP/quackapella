//! Analysis taps: what the audio thread lends to the editor's spectrum
//! (and, later, to the mixing assistant), off the audio path.
//!
//! A tap is a ring of the last [`TAP_LEN`] mono samples. The audio
//! thread writes one atomic per sample and never waits; the reader
//! copies the ring whenever it likes. A copy made while the writer
//! moves may hold a few samples of the next turn of the ring: fine for
//! a spectrum, and the price of taking nothing from the audio thread.
//! Nothing here is on the signal path: no latency.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Samples kept by a tap (a power of two).
pub const TAP_LEN: usize = 4096;

/// One ring of recent samples, written by the audio thread.
#[derive(Debug)]
pub struct Tap {
    buf: Box<[AtomicU32]>,
    /// Samples written since the start (the next slot, modulo the
    /// length).
    pos: AtomicUsize,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            buf: (0..TAP_LEN).map(|_| AtomicU32::new(0)).collect(),
            pos: AtomicUsize::new(0),
        }
    }
}

impl Tap {
    /// Audio thread: one sample. Wait-free.
    #[inline]
    pub fn push(&self, x: f32) {
        let i = self.pos.load(Ordering::Relaxed);
        self.buf[i & (TAP_LEN - 1)].store(x.to_bits(), Ordering::Relaxed);
        self.pos.store(i.wrapping_add(1), Ordering::Relaxed);
    }

    /// Samples written so far (wraps with `usize`).
    #[must_use]
    pub fn written(&self) -> usize {
        self.pos.load(Ordering::Relaxed)
    }

    /// Reader: the last `out.len()` samples (at most [`TAP_LEN`]),
    /// oldest first.
    pub fn read(&self, out: &mut [f32]) {
        let n = out.len().min(TAP_LEN);
        let end = self.pos.load(Ordering::Relaxed);
        let start = end.wrapping_sub(n);
        for (k, o) in out.iter_mut().take(n).enumerate() {
            let i = start.wrapping_add(k) & (TAP_LEN - 1);
            *o = f32::from_bits(self.buf[i].load(Ordering::Relaxed));
        }
    }
}

/// The engine's taps. Cloning shares the rings.
#[derive(Debug, Clone, Default)]
pub struct Taps {
    /// The voice as it comes in (after the input gain): the source the
    /// input effects clean.
    pub input: Arc<Tap>,
    /// After the layers, just before the output EQ: the transformed
    /// sound, without what the EQ and the output dynamics do to it.
    pub layers: Arc<Tap>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_last_samples_in_order() {
        let t = Tap::default();
        #[allow(clippy::cast_precision_loss)]
        for i in 0..(TAP_LEN + 100) {
            t.push(i as f32);
        }
        let mut out = [0.0; 8];
        t.read(&mut out);
        #[allow(clippy::cast_precision_loss)]
        let last = (TAP_LEN + 99) as f32;
        assert!((out[7] - last).abs() < 0.5);
        assert!((out[0] - (last - 7.0)).abs() < 0.5);
        assert_eq!(t.written(), TAP_LEN + 100);
    }
}
