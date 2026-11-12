//! CPU load of the engine, per layer and module, at live block sizes.
//!
//! ```text
//! cargo run --release -p quack-dsp --example bench [-- --secs 10] [-- --sr 96000]
//! ```
//!
//! Renders a synthetic singer (harmonic voice with vibrato and note
//! changes, breaths and silences) through each configuration, in host
//! blocks of 32 and 64 samples, and prints the share of one core used
//! in real time (100 % = the core is full) and the worst block, as a
//! share of the block's duration. The worst block is what decides a
//! dropout in a live set, not the average.

use std::f64::consts::TAU;
use std::time::Instant;

use quack_dsp::Engine;
use quack_dsp::params::{ParamId, ParamValues};

/// One configuration: parameter `key=value` pairs and held MIDI notes.
struct Case {
    name: &'static str,
    params: &'static [(&'static str, f64)],
    notes: &'static [u8],
}

const CASES: &[Case] = &[
    Case {
        name: "bypass",
        params: &[("bypass", 1.0)],
        notes: &[],
    },
    Case {
        name: "natural",
        params: &[],
        notes: &[],
    },
    Case {
        name: "natural +5 st",
        params: &[("pitch", 5.0), ("formant", 0.0)],
        notes: &[],
    },
    Case {
        name: "tune hard",
        params: &[("mode", 1.0), ("retune", 0.0)],
        notes: &[],
    },
    Case {
        name: "creative octaves+robot+glitch",
        params: &[
            ("creative_on", 1.0),
            ("octave_down", 60.0),
            ("sub", 40.0),
            ("octave_up", 40.0),
            ("robot", 50.0),
            ("glitch", 25.0),
            ("formant_follow", 50.0),
        ],
        notes: &[],
    },
    Case {
        name: "synth harmony 4 voices",
        params: &[
            ("harmony_on", 1.0),
            ("harmony", 2.0),
            ("harmony_3", 31.0),
            ("harmony_4", 16.0),
            ("double", 50.0),
        ],
        notes: &[],
    },
    Case {
        name: "synth harmony midi chord",
        params: &[("harmony_on", 1.0), ("harmony", 0.0)],
        notes: &[55, 59, 62, 66],
    },
    Case {
        name: "synth vocoder 32 bands",
        params: &[
            ("vocoder_on", 1.0),
            ("vocoder", 0.0),
            ("bands", 3.0),
            ("vocoder_level", 100.0),
            ("carrier", 2.0),
            ("vocoder_width", 60.0),
        ],
        notes: &[48, 55, 60, 64, 67, 71, 74, 79],
    },
    Case {
        name: "synth talkbox",
        params: &[
            ("vocoder_on", 1.0),
            ("vocoder", 1.0),
            ("vocoder_level", 100.0),
            ("carrier", 2.0),
        ],
        notes: &[48, 55, 60, 64, 67, 71, 74, 79],
    },
    Case {
        name: "every layer",
        params: &[
            ("creative_on", 1.0),
            ("octave_down", 60.0),
            ("robot", 30.0),
            ("harmony_on", 1.0),
            ("harmony", 2.0),
            ("harmony_3", 31.0),
            ("harmony_4", 16.0),
            ("double", 50.0),
            ("vocoder_on", 1.0),
            ("vocoder", 0.0),
            ("bands", 3.0),
            ("vocoder_level", 80.0),
            ("carrier", 2.0),
            ("vocoder_width", 60.0),
            ("vocoder_lofi", 30.0),
            ("consonants", 50.0),
            ("vocoder_resonance", 60.0),
            ("vocoder_tilt", 4.0),
            ("vocoder_gate", -70.0),
            ("ensemble", 60.0),
            ("carrier_sub", 40.0),
        ],
        notes: &[48, 55, 60, 64, 67, 71, 74, 79],
    },
    Case {
        name: "natural + every effect",
        params: &[
            ("fx_on", 1.0),
            ("gate", -50.0),
            ("deess", 50.0),
            ("comp_ratio", 4.0),
            ("delay_level", 40.0),
            ("delay_ping_pong", 1.0),
            ("reverb_level", 60.0),
            ("duck", 40.0),
        ],
        notes: &[],
    },
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str, default: f64| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map_or(default, |v| v.parse().expect("number"))
    };
    let secs = arg("--secs", 6.0);
    let sr = arg("--sr", 48_000.0);
    let only = args
        .iter()
        .position(|a| a == "--case")
        .and_then(|i| args.get(i + 1));
    let voice = singer(sr, secs);
    println!(
        "{:<32} {:>6} {:>9} {:>11}",
        "case", "block", "avg core", "worst block"
    );
    for case in CASES {
        if only.is_some_and(|o| !case.name.contains(o.as_str())) {
            continue;
        }
        for block in [32, 64] {
            let (avg, worst) = run(case, sr, block, &voice);
            println!(
                "{:<32} {:>6} {:>8.1}% {:>10.0}%",
                case.name,
                block,
                avg * 100.0,
                worst * 100.0
            );
        }
    }
}

/// Average and worst-block CPU share for one case. The render is run
/// [`REPEATS`] times on the same input and each block keeps its fastest
/// time: the engine is deterministic, so what remains of a slow block
/// is the engine's own work, not the OS preempting the benchmark.
fn run(case: &Case, sr: f64, block: usize, voice: &[f32]) -> (f64, f64) {
    let blocks = voice.len().div_ceil(block);
    let mut best = vec![f64::INFINITY; blocks];
    for _ in 0..REPEATS {
        let times = render(case, sr, block, voice);
        for (b, t) in best.iter_mut().zip(times) {
            *b = b.min(t);
        }
    }
    let block_s = block as f64 / sr;
    let total: f64 = best.iter().sum();
    let worst = best.iter().copied().fold(0.0, f64::max);
    (total / (voice.len() as f64 / sr), worst / block_s)
}

const REPEATS: usize = 5;

/// Time of each block of one render.
fn render(case: &Case, sr: f64, block: usize, voice: &[f32]) -> Vec<f64> {
    let mut values = ParamValues::default();
    for &(k, v) in case.params {
        let id = ParamId::ALL
            .into_iter()
            .find(|p| p.spec().key == k)
            .unwrap_or_else(|| panic!("unknown parameter {k}"));
        values.set(id, v);
    }
    let mut e = Engine::new();
    e.prepare(sr, block);
    e.set_all(&values, true);
    for &n in case.notes {
        e.note_on(0, n, 100);
    }
    let mut l = vec![0.0f32; block];
    let mut r = vec![0.0f32; block];
    let mut times = Vec::with_capacity(voice.len().div_ceil(block));
    for chunk in voice.chunks(block) {
        let n = chunk.len();
        l[..n].copy_from_slice(chunk);
        r[..n].copy_from_slice(chunk);
        let t = Instant::now();
        e.process(&mut l[..n], &mut r[..n]);
        times.push(t.elapsed().as_secs_f64());
        std::hint::black_box(&l);
    }
    times
}

/// A rough singer: harmonic voice with vibrato, a note every 0.6 s
/// (some off-key), a breath (noise) and a short silence between
/// phrases.
fn singer(sr: f64, secs: f64) -> Vec<f32> {
    let n = (sr * secs) as usize;
    let notes = [57.0, 59.3, 61.0, 62.0, 64.4, 52.0, 45.0, 69.0];
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let mut seed = 0x1234_5678_u32;
    for i in 0..n {
        let t = i as f64 / sr;
        let k = (t / 0.6) as usize;
        let local = t - k as f64 * 0.6;
        let note = notes[k % notes.len()];
        let vib = 0.4 * (TAU * 5.5 * t).sin();
        let hz = 440.0 * ((note + vib - 69.0) / 12.0).exp2();
        phase = (phase + hz / sr).fract();
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let noise = f64::from(seed) / f64::from(u32::MAX) - 0.5;
        let s = if k % 4 == 3 && local > 0.45 {
            // Silence then breath at the end of a phrase.
            if local > 0.52 { 0.05 * noise } else { 0.0 }
        } else {
            let env = (local / 0.03).min(1.0) * ((0.6 - local) / 0.03).min(1.0);
            let mut v = 0.0;
            for h in 1..=24 {
                let h = f64::from(h);
                // Rough formant hump around 700 Hz and 2.5 kHz.
                let f = hz * h;
                let w = 1.0 / (1.0 + ((f - 700.0) / 400.0).powi(2))
                    + 0.4 / (1.0 + ((f - 2500.0) / 600.0).powi(2));
                v += w / h * (TAU * h * phase).sin();
            }
            0.3 * env * v + 0.005 * noise
        };
        #[allow(clippy::cast_possible_truncation)]
        out.push(s as f32);
    }
    out
}
