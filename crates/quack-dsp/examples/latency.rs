//! Latency measurements, module by module.
//!
//! ```text
//! cargo run --release -p quack-dsp --example latency            # table
//! cargo run --release -p quack-dsp --example latency -- IN.wav  # a take
//! cargo run --release -p quack-dsp --example latency -- --attack # quality
//! ```
//!
//! The table: for synthetic vowels from 82 to 660 Hz, the delay the
//! voice really has through the engine (reported latency plus what a
//! low voice adds), per voice range and host block size. With a WAV
//! file: mean and worst delay of the voice over the take, transposed or
//! not. `--attack`: what shorter grain attacks cost in formant
//! fidelity.

use std::f64::consts::PI;

use quack_dsp::Engine;
use quack_dsp::latency::{VoiceRange, latency_samples};
use quack_dsp::params::{ParamId, ParamValues};
use quack_dsp::pitch::{PitchDetector, Yin};
use quack_dsp::shift::{Psola, semitones_to_ratio};

const SR: f64 = 48_000.0;

fn vowel(hz: f64, secs: f64) -> Vec<f32> {
    vowel_with(hz, secs, 0.004)
}

/// Vowel with a 5.5 Hz vibrato of `depth` (ratio).
fn vowel_with(hz: f64, secs: f64, depth: f64) -> Vec<f32> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (SR * secs) as usize;
    let mut src = vec![0.0f64; n];
    let mut ph = 0.0;
    for (i, v) in src.iter_mut().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let t = i as f64 / SR;
        ph += hz * (1.0 + depth * (2.0 * PI * 5.5 * t).sin()) / SR;
        ph -= ph.floor();
        *v = 1.0 - 2.0 * ph;
    }
    let mut out = vec![0.0f64; n];
    for (f, bw, g) in [
        (700.0, 90.0, 1.0),
        (1_150.0, 110.0, 0.6),
        (2_600.0, 160.0, 0.2),
    ] {
        let r = (-PI * bw / SR).exp();
        let a1 = -2.0 * r * (2.0 * PI * f / SR).cos();
        let a2 = r * r;
        let (mut y1, mut y2) = (0.0, 0.0);
        for (o, &v) in out.iter_mut().zip(&src) {
            let y = (1.0 - r) * v - a1 * y1 - a2 * y2;
            y2 = y1;
            y1 = y;
            *o += g * y;
        }
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    #[allow(clippy::cast_possible_truncation)]
    out.iter().map(|v| (0.3 * v / peak) as f32).collect()
}

/// Mean and worst voice delay (ms) of `x` through an engine.
fn measure(x: &[f32], sr: f64, values: &ParamValues, block: usize) -> (f64, f64) {
    let mut e = Engine::new();
    e.prepare(sr, block);
    e.set_all(values, true);
    let (mut l, mut r) = (x.to_vec(), x.to_vec());
    let (mut sum, mut max, mut n) = (0.0, 0.0f64, 0.0);
    for (i, (cl, cr)) in l.chunks_mut(block).zip(r.chunks_mut(block)).enumerate() {
        e.process(cl, cr);
        // Skip the first half second (the first note sets the delay).
        #[allow(clippy::cast_precision_loss)]
        if (i * block) as f64 > 0.5 * sr {
            let d = e.voice_latency();
            sum += d;
            max = max.max(d);
            n += 1.0;
        }
    }
    (sum / n / sr * 1000.0, max / sr * 1000.0)
}

/// Sample rate and first channel of a WAV file.
fn read_wav(path: &str) -> (f64, Vec<f32>) {
    let b = std::fs::read(path).expect("read");
    let find = |id: &[u8]| b.windows(4).position(|w| w == id).expect("chunk");
    let fmt = find(b"fmt ") + 8;
    let ch = usize::from(u16::from_le_bytes([b[fmt + 2], b[fmt + 3]]));
    let bits = u16::from_le_bytes([b[fmt + 14], b[fmt + 15]]);
    let sr = u32::from_le_bytes([b[fmt + 4], b[fmt + 5], b[fmt + 6], b[fmt + 7]]);
    let data = find(b"data") + 8;
    let frame = ch * usize::from(bits / 8);
    let x = b[data..]
        .chunks_exact(frame)
        .map(|c| match bits {
            16 => f32::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0,
            #[allow(clippy::cast_precision_loss)]
            24 => (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0,
            32 => f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            _ => panic!("unsupported WAV"),
        })
        .collect();
    (f64::from(sr), x)
}

fn main() {
    let mut v = ParamValues::default();
    v.set(ParamId::Correction, 0.0);
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--attack") {
        attack_quality();
        return;
    }
    if let Some(path) = arg {
        let (sr, x) = read_wav(&path);
        for range in VoiceRange::ALL {
            v.set(ParamId::Voice, f64::from(range as u8));
            for st in [0.0, 5.0, -7.0] {
                v.set(ParamId::Pitch, st);
                let (mean, max) = measure(&x, sr, &v, 32);
                println!(
                    "{:<15} {st:+} st: voice delay {mean:5.2} ms mean, {max:5.2} ms worst",
                    range.name()
                );
            }
        }
        return;
    }
    let notes = [82.0, 110.0, 147.0, 196.0, 262.0, 440.0];
    let mut head = format!("{:<15} {:>9}", "voice", "reported");
    for hz in notes {
        head += &format!(" {hz:>5.0} Hz");
    }
    for block in [32, 64] {
        println!("-- host block {block}, voice delay in ms (+7 st)\n{head}");
        for range in VoiceRange::ALL {
            v.set(ParamId::Voice, f64::from(range as u8));
            v.set(ParamId::Pitch, 7.0);
            #[allow(clippy::cast_precision_loss)]
            let reported = latency_samples(SR, range) as f64 / SR * 1000.0;
            let mut row = format!("{:<15} {reported:>9.2}", range.name());
            for hz in notes {
                let (mean, _) = measure(&vowel(hz, 1.0), SR, &v, block);
                row += &format!(" {mean:>8.2}");
            }
            println!("{row}");
        }
    }
    println!("-- short attack (not used by the engine): latency per range, ms");
    for attack in [1.0, 0.5, 0.3] {
        let mut row = format!("attack {attack:<4}");
        for range in VoiceRange::ALL {
            #[allow(clippy::cast_precision_loss)]
            let ms = Psola::latency_for(SR, range.lowest_hz(), attack) as f64 / SR * 1000.0;
            row += &format!("  {} {ms:.1}", range.name());
        }
        println!("{row}");
    }
}

/// Short attack against the default grains: formant envelope error
/// (dB, output against input, harmonics peak-held) and periodicity at
/// the target period, for steady vowels shifted by a few intervals.
fn attack_quality() {
    fn spectrum(x: &[f32]) -> Vec<f64> {
        let n = x.len() as f64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bins = (4_000.0 / SR * n) as usize;
        (1..bins)
            .map(|b| {
                let (mut re, mut im) = (0.0, 0.0);
                for (i, &v) in x.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    let (t, b) = (i as f64, b as f64);
                    let w = 0.5 - 0.5 * (2.0 * PI * t / n).cos();
                    let ph = 2.0 * PI * b * t / n;
                    re += w * f64::from(v) * ph.cos();
                    im -= w * f64::from(v) * ph.sin();
                }
                re.hypot(im)
            })
            .collect()
    }
    fn envelope(s: &[f64], width: usize) -> Vec<f64> {
        let top = (0..s.len())
            .map(|i| {
                let (a, b) = (
                    i.saturating_sub(width / 2),
                    (i + width / 2).min(s.len() - 1),
                );
                20.0 * s[a..=b].iter().fold(1e-9f64, |m, v| m.max(*v)).log10()
            })
            .collect::<Vec<_>>();
        let max = top.iter().copied().fold(f64::MIN, f64::max);
        top.iter().map(|v| v - max).collect()
    }
    let bin = SR / 4096.0;
    for attack in [1.0, 0.5, 0.3] {
        let (mut env_sum, mut per_sum, mut n) = (0.0, 0.0, 0.0);
        let mut worst = 0.0f64;
        for hz in [110.0, 196.0, 330.0] {
            let x = steady(hz, 33_600);
            let xs = spectrum(&x[24_000..28_096]);
            for st in [-12.0f32, -5.0, 3.0, 7.0, 12.0] {
                let mut det = Yin::new(SR);
                let mut ps = Psola::new(SR, 1_200);
                ps.set_attack(attack);
                let pr = semitones_to_ratio(st);
                let y: Vec<f32> = x
                    .iter()
                    .map(|&v| {
                        det.push_sample(v);
                        #[allow(clippy::cast_possible_truncation)]
                        let p = det.estimate().map(|e| (SR / f64::from(e.hz)) as f32);
                        ps.tick(v, v, p, pr, 1.0).0
                    })
                    .collect();
                let ys = spectrum(&y[24_000..28_096]);
                let target = hz * f64::from(pr);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let (w, lo) = (
                    (hz.max(target) * 1.2 / bin) as usize,
                    (300.0 / bin) as usize,
                );
                let (ex, ey) = (envelope(&xs, w), envelope(&ys, w));
                #[allow(clippy::cast_precision_loss)]
                let env = (lo..ex.len()).map(|i| (ex[i] - ey[i]).abs()).sum::<f64>()
                    / (ex.len() - lo) as f64;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let t = (SR / target).round() as usize;
                let (mut ab, mut aa, mut bb) = (0.0f64, 0.0, 0.0);
                for i in 20_000..30_000 - t {
                    let (a, b) = (f64::from(y[i]), f64::from(y[i + t]));
                    ab += a * b;
                    aa += a * a;
                    bb += b * b;
                }
                env_sum += env;
                worst = worst.max(env);
                per_sum += ab / (aa * bb).sqrt();
                n += 1.0;
            }
        }
        println!(
            "attack {attack}: envelope error {:.2} dB mean, {worst:.2} dB worst; periodicity {:.4}",
            env_sum / n,
            per_sum / n
        );
    }
}

/// A steady vowel (no vibrato).
fn steady(hz: f64, n: usize) -> Vec<f32> {
    #[allow(clippy::cast_precision_loss)]
    let mut x = vowel_with(hz, n as f64 / SR + 0.01, 0.0);
    x.truncate(n);
    x
}
