//! Flush-to-zero for the audio thread.
//!
//! A filter or a delay line fading out goes through subnormal numbers
//! (below ~1e-38 for `f32`, ~1e-308 for `f64`). On x86 every operation
//! on one costs about a hundred times more: a reverb tail or a release
//! can then blow the block's time budget for a moment. The usual cure,
//! as in every plugin framework, is to have the CPU treat them as zero
//! for the time of `process`, and to restore the host's setting after.
//!
//! Apple silicon handles subnormals at full speed, but the flag is set
//! there too: same numbers on every machine.

/// Sets flush-to-zero (and denormals-are-zero on x86) while alive, then
/// restores the previous mode. Cheap (two register moves).
pub struct NoDenormals {
    #[allow(dead_code)]
    saved: usize,
}

impl NoDenormals {
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self {
            saved: imp::enter(),
        }
    }
}

impl Default for NoDenormals {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NoDenormals {
    #[inline]
    fn drop(&mut self) {
        imp::leave(self.saved);
    }
}

#[cfg(target_arch = "x86_64")]
mod imp {
    /// MXCSR: FTZ (bit 15) and DAZ (bit 6).
    const FLAGS: u32 = (1 << 15) | (1 << 6);

    #[inline]
    pub fn enter() -> usize {
        let mut csr: u32 = 0;
        // SAFETY: stmxcsr/ldmxcsr only read and write the SSE control
        // register; the new value only adds the two flush flags.
        unsafe {
            std::arch::asm!("stmxcsr [{}]", in(reg) &raw mut csr, options(nostack));
            let new = csr | FLAGS;
            std::arch::asm!("ldmxcsr [{}]", in(reg) &raw const new, options(nostack));
        }
        csr as usize
    }

    #[inline]
    pub fn leave(saved: usize) {
        #[allow(clippy::cast_possible_truncation)]
        let csr = saved as u32;
        // SAFETY: restores the value read in `enter`.
        unsafe {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &raw const csr, options(nostack));
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    /// FPCR: FZ (bit 24).
    const FZ: usize = 1 << 24;

    #[inline]
    pub fn enter() -> usize {
        let fpcr: usize;
        // SAFETY: reads and writes the FP control register only; the
        // new value only adds the flush-to-zero flag.
        unsafe {
            std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack));
            std::arch::asm!("msr fpcr, {}", in(reg) fpcr | FZ, options(nomem, nostack));
        }
        fpcr
    }

    #[inline]
    pub fn leave(saved: usize) {
        // SAFETY: restores the value read in `enter`.
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) saved, options(nomem, nostack));
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod imp {
    #[inline]
    pub fn enter() -> usize {
        0
    }
    #[inline]
    pub fn leave(_: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flushes_inside_and_restores_after() {
        let tiny = std::hint::black_box(f32::MIN_POSITIVE);
        let half = std::hint::black_box(0.5f32);
        assert!((tiny * half).is_subnormal());
        {
            let _guard = NoDenormals::new();
            assert_eq!(std::hint::black_box(tiny) * std::hint::black_box(half), 0.0);
        }
        assert!((tiny * half).is_subnormal());
    }
}
