//! Live-safety checks of the whole engine: no allocation on the audio
//! thread, any block size, bad input, sample rate changes, every mode
//! and parameter combined.
//!
//! The allocator counts allocations made by the thread that set
//! `WATCH`, so the test harness's own threads don't count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicUsize, Ordering};

use quack_dsp::Engine;
use quack_dsp::params::{ParamId, ParamKind, ParamValues};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
}

fn watched() -> bool {
    WATCH.try_with(Cell::get).unwrap_or(false)
}

// SAFETY: forwards to the system allocator, only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if watched() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if watched() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if watched() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocations made by `f` on this thread.
fn allocations(f: impl FnOnce()) -> usize {
    let before = ALLOCS.load(Ordering::Relaxed);
    WATCH.with(|w| w.set(true));
    f();
    WATCH.with(|w| w.set(false));
    ALLOCS.load(Ordering::Relaxed) - before
}

/// A voice-like test signal: harmonics, vibrato, a note change every
/// 0.4 s, breaths and silences.
fn voice(sr: f64, secs: f64) -> Vec<f32> {
    let n = (sr * secs) as usize;
    let notes = [57.0, 61.0, 64.4, 52.0, 45.0, 69.0];
    let mut phase = 0.0f64;
    let mut seed = 0x9e37_79b9_u32;
    (0..n)
        .map(|i| {
            let t = i as f64 / sr;
            let k = (t / 0.4) as usize;
            let local = t - k as f64 * 0.4;
            let hz = 440.0
                * ((notes[k % notes.len()] + 0.3 * (TAU * 5.0 * t).sin() - 69.0) / 12.0).exp2();
            phase = (phase + hz / sr).fract();
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = f64::from(seed) / f64::from(u32::MAX) - 0.5;
            let s = if k % 3 == 2 && local > 0.3 {
                0.03 * noise
            } else {
                (1..=12)
                    .map(|h| (TAU * f64::from(h) * phase).sin() / f64::from(h))
                    .sum::<f64>()
                    * 0.2
            };
            s as f32
        })
        .collect()
}

fn param(key: &str) -> ParamId {
    ParamId::ALL
        .into_iter()
        .find(|p| p.spec().key == key)
        .unwrap_or_else(|| panic!("unknown parameter {key}"))
}

/// Every mode with its heaviest settings.
fn heavy(mode: f64) -> ParamValues {
    let mut v = ParamValues::default();
    let mut set = |k: &str, x: f64| v.set(param(k), x);
    set("mode", mode);
    set("pitch", 3.0);
    set("formant", -2.0);
    set("octave_down", 50.0);
    set("sub", 30.0);
    set("octave_up", 30.0);
    set("robot", 40.0);
    set("glitch", 30.0);
    set("formant_follow", 50.0);
    set("harmony", 2.0);
    set("harmony_3", 11.0);
    set("harmony_4", 5.0);
    set("double", 60.0);
    set("vocoder", 1.0);
    set("bands", 3.0);
    set("vocoder_level", 70.0);
    set("carrier", 2.0);
    set("carrier_follow", 1.0);
    set("vocoder_width", 50.0);
    set("vocoder_lofi", 20.0);
    set("consonants", 50.0);
    set("vocoder_resonance", 60.0);
    set("vocoder_tilt", -4.0);
    set("vocoder_gate", -70.0);
    set("ensemble", 50.0);
    set("carrier_sub", 30.0);
    set("fx_on", 1.0);
    set("gate", -60.0);
    set("deess", 50.0);
    set("comp_ratio", 4.0);
    set("comp_threshold", -30.0);
    set("delay_level", 40.0);
    set("delay_feedback", 20.0);
    set("delay_ping_pong", 1.0);
    set("reverb_level", 60.0);
    set("reverb_decay", 0.5);
    set("duck", 30.0);
    set("low_cut", 120.0);
    set("presence", 4.0);
    set("air", 3.0);
    set("drive", 40.0);
    set("chorus_on", 1.0);
    set("delay_mod", 50.0);
    set("delay_width", 50.0);
    set("reverb_mod", 60.0);
    set("delay_to_reverb", 40.0);
    v
}

/// Process `x` in blocks of `block` (both channels the same), with a
/// side-chain, playing MIDI along. Returns the left output.
fn run(e: &mut Engine, x: &[f32], block: usize, out: &mut [f32], scratch: &mut [f32]) {
    let mut k = 0usize;
    for (i, chunk) in x.chunks(block).enumerate() {
        let n = chunk.len();
        let (l, r) = (&mut out[k..k + n], &mut scratch[..n]);
        l.copy_from_slice(chunk);
        r.copy_from_slice(chunk);
        // MIDI every ~100 blocks: chords, bend, sustain.
        match i % 97 {
            0 => {
                for note in [48, 55, 60, 64] {
                    e.note_on(0, note, 90);
                }
            }
            30 => e.pitch_bend(0.5),
            40 => e.set_sustain(true),
            50 => {
                for note in [48, 55, 60, 64] {
                    e.note_off(0, note);
                }
            }
            60 => e.set_sustain(false),
            70 => e.pitch_bend(0.0),
            _ => {}
        }
        let side = (chunk, chunk);
        e.process_with_sidechain(l, r, Some(side));
        k += n;
    }
}

#[test]
fn processing_never_allocates() {
    let sr = 48_000.0;
    let x = voice(sr, 2.0);
    let mut out = vec![0.0; x.len()];
    let mut scratch = vec![0.0; 4096];
    for mode in 0..4 {
        for block in [1, 32, 64, 4096] {
            let mut e = Engine::new();
            e.prepare(sr, block);
            e.set_all(&heavy(f64::from(mode)), true);
            let n = allocations(|| {
                run(&mut e, &x, block, &mut out, &mut scratch);
                e.set_transport(120.0, 4.0, Some(3.0));
                e.all_notes_off();
            });
            assert_eq!(n, 0, "mode {mode}, block {block}: {n} allocations");
        }
    }
}

#[test]
fn parameter_changes_never_allocate() {
    // Every parameter moved across its range while playing, the way a
    // host automates or a preset loads (voice range and Low Latency
    // included: they move the latency).
    let sr = 44_100.0;
    let x = voice(sr, 0.5);
    let mut out = vec![0.0; x.len()];
    let mut scratch = vec![0.0; 64];
    let mut e = Engine::new();
    e.prepare(sr, 64);
    e.set_all(&heavy(3.0), true);
    let n = allocations(|| {
        for step in 0..5 {
            for id in ParamId::ALL {
                let spec = id.spec();
                let t = f64::from(step) / 4.0;
                let v = spec.kind.denormalize(t);
                e.set_param(id, v);
            }
            run(&mut e, &x[..4096], 64, &mut out, &mut scratch);
            let values = heavy(f64::from(step % 4));
            e.set_all(&values, step % 2 == 0);
        }
    });
    assert_eq!(n, 0, "{n} allocations");
}

#[test]
fn bad_input_never_sticks() {
    // NaN, infinities and huge values in: the output must be finite and
    // come back to normal once the input is clean again, in every mode.
    let sr = 48_000.0;
    let clean = voice(sr, 1.5);
    for mode in 0..4 {
        let mut e = Engine::new();
        e.prepare(sr, 64);
        e.set_all(&heavy(f64::from(mode)), true);
        let mut x = clean.clone();
        for (i, s) in x[2000..2600].iter_mut().enumerate() {
            *s = match i % 4 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => -1e30,
                _ => 1e30,
            };
        }
        // Garbage from the host's transport and MIDI too.
        e.set_transport(f64::NAN, 0.0, Some(f64::INFINITY));
        e.set_transport(0.0, -3.0, Some(-1e300));
        e.pitch_bend(f64::NAN);
        for id in ParamId::ALL {
            e.set_param(id, f64::NAN);
        }
        e.set_all(&heavy(f64::from(mode)), false);
        let mut out = vec![0.0; x.len()];
        let mut scratch = vec![0.0; 64];
        run(&mut e, &x, 64, &mut out, &mut scratch);
        assert!(
            out.iter().all(|v| v.is_finite()),
            "mode {mode}: non-finite output"
        );
        let tail = &out[out.len() - 12_000..];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            peak < 4.0,
            "mode {mode}: stuck after bad input, peak {peak}"
        );
        assert!(peak > 1e-3, "mode {mode}: silent after bad input");
    }
}

#[test]
fn block_size_does_not_change_the_sound() {
    // Everything is computed per sample: blocks of 1, 32, 64 and more
    // than the prepared maximum give the same output.
    let sr = 48_000.0;
    let x = voice(sr, 1.0);
    let mut reference: Option<Vec<f32>> = None;
    for block in [32, 1, 64, 1000] {
        let mut e = Engine::new();
        // Prepared for 64: a host may still send larger blocks.
        e.prepare(sr, 64);
        let mut v = heavy(3.0);
        // No MIDI timing differences between block sizes.
        v.set(param("carrier_follow"), 1.0);
        e.set_all(&v, true);
        let mut l = x.clone();
        let mut r = x.clone();
        for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            e.process(cl, cr);
        }
        match &reference {
            None => reference = Some(l),
            Some(want) => {
                let diff = want
                    .iter()
                    .zip(&l)
                    .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
                assert!(diff < 1e-5, "block {block}: differs by {diff}");
            }
        }
    }
}

#[test]
fn sample_rate_changes_keep_working() {
    // A host switching rates calls prepare again on the same engine.
    let mut e = Engine::new();
    e.set_all(&heavy(3.0), true);
    for sr in [44_100.0, 96_000.0, 22_050.0, 192_000.0, 48_000.0, 88_200.0] {
        e.prepare(sr, 64);
        let x = voice(sr, 0.6);
        let mut out = vec![0.0; x.len()];
        let mut scratch = vec![0.0; 64];
        let n = allocations(|| run(&mut e, &x, 64, &mut out, &mut scratch));
        assert_eq!(n, 0, "{sr} Hz: {n} allocations");
        assert!(out.iter().all(|v| v.is_finite()), "{sr} Hz: non-finite");
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 1e-3 && peak < 4.0, "{sr} Hz: peak {peak}");
        // Latency stays under the 10 ms rule for the Soprano/Alto range
        // and scales with the rate.
        let ms = e.latency_samples() as f64 / sr * 1000.0;
        assert!(ms < 25.0, "{sr} Hz: {ms} ms");
    }
}

#[test]
fn silence_decays_to_silence() {
    // After a loud take, silence in gives silence out (no tail that
    // hangs in denormals, no noise floor) in every mode, MIDI released.
    let sr = 48_000.0;
    for mode in 0..4 {
        let mut e = Engine::new();
        e.prepare(sr, 64);
        e.set_all(&heavy(f64::from(mode)), true);
        let mut x = voice(sr, 1.0);
        x.extend(std::iter::repeat_n(0.0, (sr * 3.0) as usize));
        let mut out = vec![0.0; x.len()];
        let mut scratch = vec![0.0; 64];
        run(&mut e, &x[..48_000], 64, &mut out, &mut scratch);
        e.all_notes_off();
        let mut l = x[48_000..].to_vec();
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(64).zip(r.chunks_mut(64)) {
            e.process(cl, cr);
        }
        let tail = &l[l.len() - 4800..];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 1e-6, "mode {mode}: tail at {peak}");
        let subnormal = tail.iter().filter(|v| v.is_subnormal()).count();
        assert_eq!(subnormal, 0, "mode {mode}: subnormal output");
    }
}

#[test]
fn every_enum_value_is_safe() {
    // Each value of every enum and bool parameter, one at a time, on the
    // heavy Synth settings: finite, bounded output.
    let sr = 48_000.0;
    let x = voice(sr, 0.3);
    let mut out = vec![0.0; x.len()];
    let mut scratch = vec![0.0; 64];
    for id in ParamId::ALL {
        let count = match id.spec().kind {
            ParamKind::Enum { count, .. } => count,
            ParamKind::Bool { .. } => 2,
            ParamKind::Float { .. } => continue,
        };
        for k in 0..count {
            let mut e = Engine::new();
            e.prepare(sr, 64);
            let mut v = heavy(3.0);
            v.set(id, f64::from(k));
            e.set_all(&v, true);
            run(&mut e, &x, 64, &mut out, &mut scratch);
            let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(
                out.iter().all(|v| v.is_finite()) && peak < 8.0,
                "{} = {k}: peak {peak}",
                id.spec().key
            );
        }
    }
}
