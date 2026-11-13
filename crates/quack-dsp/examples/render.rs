//! Offline render through the engine, for listening tests.
//!
//! ```text
//! cargo run --release -p quack-dsp --example render -- IN.wav OUT.wav [key=value ...]
//! cargo run --release -p quack-dsp --example render -- --demo OUT.wav
//! ```
//!
//! `key` is a parameter key of `quack_dsp::params` (`mode=1`,
//! `retune=0`, `scale=2`, `key=9`, `pitch=-5`...); `midi=60,64@1.5`
//! holds notes 60 and 64 from 1.5 s (repeat for a sequence, `midi=@4`
//! releases everything at 4 s); `key=value@t` changes a parameter at
//! `t` seconds (`freeze=1@2.1 freeze=0@4`); `preset=Radio Pop` loads a
//! factory preset (`preset=Radio Pop/B`: its snapshot B); `sc=FILE.wav` feeds the
//! side-chain input (mono or stereo, cut or padded to the input); `assist=Pop`
//! (or `assist=Pop:0.6` for the amount) listens to the whole file with
//! the settings given, prints what the mixing assistant hears and
//! renders with what it proposes. `--demo` writes a synthetic singer
//! (glottal source, formants, scoops, vibrato, off-key notes,
//! consonants) instead of reading a file. WAV in: 16/24-bit PCM or
//! 32-bit float, mono or stereo; out: 32-bit float, same channels
//! (always stereo with the harmonies, the vocoder or the effects on).

use std::f64::consts::{PI, TAU};
use std::fs;

use quack_dsp::Engine;
use quack_dsp::params::{ParamId, ParamValues};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: render IN.wav|--demo OUT.wav [key=value ...]");
        std::process::exit(2);
    }
    if args[0] == "--track" {
        // Pitch track every 10 ms: time, note (or -).
        let (sr, chans) = read_wav(&args[1]);
        let mut det = quack_dsp::pitch::Yin::new(f64::from(sr));
        let hop = sr as usize / 100;
        for (k, chunk) in chans[0].chunks(hop).enumerate() {
            quack_dsp::pitch::PitchDetector::push(&mut det, chunk);
            let e = quack_dsp::pitch::PitchDetector::estimate(&det);
            println!(
                "{:.2} {}",
                k as f64 / 100.0,
                e.map_or("-".to_owned(), |e| format!("{:.3}", e.note()))
            );
        }
        return;
    }
    if args[0] == "--demo" {
        let x = demo_singer(48_000.0);
        write_wav(&args[1], 48_000, &[x]);
        return;
    }
    let (sr, chans) = read_wav(&args[0]);
    let mut values = ParamValues::default();
    let mut midi: Vec<(f64, Vec<u8>)> = Vec::new();
    let mut autos: Vec<(f64, ParamId, f64)> = Vec::new();
    let mut side: Option<Vec<Vec<f32>>> = None;
    let mut assist: Option<(quack_dsp::assist::Style, f32)> = None;
    for kv in &args[2..] {
        let (k, v) = kv.split_once('=').expect("key=value");
        if k == "assist" {
            // `assist=Pop` or `assist=Pop:0.6` (the amount).
            let (name, amount) = v.split_once(':').unwrap_or((v, "1"));
            let style = quack_dsp::assist::Style::ALL
                .into_iter()
                .find(|s| s.name().eq_ignore_ascii_case(name))
                .unwrap_or_else(|| panic!("unknown style {name}"));
            assist = Some((style, amount.parse().expect("amount")));
            continue;
        }
        if k == "sc" {
            side = Some(read_wav(v).1);
            continue;
        }
        if k == "preset" {
            // `preset=Name` or `preset=Name/B` (its snapshot B); the
            // setup parameters keep what came before.
            let (name, b) = v.strip_suffix("/B").map_or((v, false), |n| (n, true));
            let f = quack_dsp::presets::FACTORY
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("unknown preset {name}"));
            let pv = match f.snapshots() {
                Some([_, snap_b]) if b => snap_b,
                _ => f.values(),
            };
            values = quack_dsp::presets::merge(&values, &pv);
            continue;
        }
        if k == "midi" {
            let (notes, at) = v.split_once('@').unwrap_or((v, "0"));
            let notes = notes
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().expect("note"))
                .collect();
            midi.push((at.parse().expect("time"), notes));
            continue;
        }
        let id = ParamId::ALL
            .into_iter()
            .find(|p| p.spec().key == k)
            .unwrap_or_else(|| panic!("unknown parameter {k}"));
        match v.split_once('@') {
            Some((v, at)) => {
                autos.push((at.parse().expect("time"), id, v.parse().expect("number")))
            }
            None => values.set(id, v.parse().expect("number")),
        }
    }
    midi.sort_by(|a, b| a.0.total_cmp(&b.0));
    autos.sort_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((style, amount)) = assist {
        listen(f64::from(sr), &chans, &mut values, style, amount);
    }
    let out = process(
        f64::from(sr),
        &chans,
        &values,
        &midi,
        &autos,
        side.as_deref(),
    );
    write_wav(&args[1], sr, &out);
}

/// The mixing assistant, offline: listen to the whole take with the
/// settings given, print what was heard, and add the proposal to them.
fn listen(
    sr: f64,
    chans: &[Vec<f32>],
    values: &mut ParamValues,
    style: quack_dsp::assist::Style,
    amount: f32,
) {
    use quack_dsp::assist;
    let mut e = Engine::new();
    e.prepare(sr, 512);
    e.set_all(values, true);
    let taps = e.taps();
    let mut l = chans[0].clone();
    let mut r = chans.get(1).unwrap_or(&chans[0]).clone();
    let mut ear = assist::Listener::new(sr);
    let (mut a, mut b) = ([0.0_f32; 256], [0.0_f32; 256]);
    for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        e.process(cl, cr);
        let n = cl.len();
        taps.input.read(&mut a[..n]);
        taps.layers.read(&mut b[..n]);
        let hz = e.pitch().map_or(0.0, |p| p.hz);
        ear.feed(&a[..n], &b[..n], hz);
    }
    let Some(heard) = ear.finish() else {
        eprintln!("assistant: not enough singing");
        return;
    };
    eprintln!("{heard:?}");
    for line in assist::describe(&heard, style, assist::Amounts::all(amount), 120.0) {
        eprintln!("  {line}");
    }
    for (id, v) in assist::propose(&heard, style, assist::Amounts::all(amount), 120.0).changes {
        eprintln!("    {} = {v}", id.spec().key);
        values.set(id, v);
    }
}

fn process(
    sr: f64,
    chans: &[Vec<f32>],
    values: &ParamValues,
    midi: &[(f64, Vec<u8>)],
    autos: &[(f64, ParamId, f64)],
    side: Option<&[Vec<f32>]>,
) -> Vec<Vec<f32>> {
    let mut e = Engine::new();
    e.prepare(sr, 512);
    e.set_all(values, true);
    let lat = e.latency_samples();
    let n = chans[0].len();
    let mut l: Vec<f32> = chans[0]
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0.0, lat))
        .collect();
    let mut r: Vec<f32> = chans
        .get(1)
        .unwrap_or(&chans[0])
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0.0, lat))
        .collect();
    let side: Option<[Vec<f32>; 2]> = side.map(|c| {
        let fit = |x: &Vec<f32>| {
            let mut x = x.clone();
            x.resize(l.len(), 0.0);
            x
        };
        [fit(&c[0]), fit(c.get(1).unwrap_or(&c[0]))]
    });
    let mut held: Vec<u8> = Vec::new();
    let mut next = 0;
    let mut next_auto = 0;
    let mut pos = 0;
    while pos < l.len() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let due_auto = autos
            .get(next_auto)
            .map_or(usize::MAX, |a| (a.0 * sr) as usize);
        if due_auto <= pos {
            e.set_param(autos[next_auto].1, autos[next_auto].2);
            next_auto += 1;
            continue;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let due = midi.get(next).map_or(usize::MAX, |m| (m.0 * sr) as usize);
        if due <= pos {
            for &n in &held {
                e.note_off(0, n);
            }
            held.clone_from(&midi[next].1);
            for &n in &held {
                e.note_on(0, n, 100);
            }
            next += 1;
            continue;
        }
        let end = (pos + 256)
            .min(l.len())
            .min(due.max(pos + 1))
            .min(due_auto.max(pos + 1));
        e.process_with_sidechain(
            &mut l[pos..end],
            &mut r[pos..end],
            side.as_ref().map(|s| (&s[0][pos..end], &s[1][pos..end])),
        );
        pos = end;
    }
    // Latency compensation, as the host does.
    let mut out = vec![l[lat..lat + n].to_vec()];
    // The harmonies spread in stereo: a mono take comes out stereo.
    if chans.len() > 1
        || values.get(ParamId::HarmonyOn) >= 0.5
        || values.get(ParamId::VocoderOn) >= 0.5
        || values.get(ParamId::FxOn) >= 0.5
    {
        out.push(r[lat..lat + n].to_vec());
    }
    out
}

/// A synthetic singer: a short phrase in A minor, sung loosely (off-key
/// notes, scoops into the notes, vibrato on the long ones, jitter),
/// with consonant bursts and breaths between the notes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
#[allow(clippy::needless_range_loop, clippy::too_many_lines)]
fn demo_singer(sr: f64) -> Vec<f32> {
    // (note, cents off, seconds, vowel)
    let phrase: [(f64, f64, f64, usize); 10] = [
        (57.0, -35.0, 0.45, 0),
        (60.0, 25.0, 0.45, 1),
        (64.0, -20.0, 0.9, 0),
        (62.0, 40.0, 0.4, 2),
        (60.0, -30.0, 0.4, 1),
        (59.0, 30.0, 0.45, 0),
        (57.0, -15.0, 1.3, 1),
        (64.0, 35.0, 0.45, 2),
        (67.0, -40.0, 0.5, 0),
        (69.0, 20.0, 1.6, 1),
    ];
    let vowels: [[(f64, f64, f64); 4]; 3] = [
        [
            (730.0, 80.0, 1.0),
            (1_090.0, 90.0, 0.5),
            (2_440.0, 120.0, 0.18),
            (3_400.0, 150.0, 0.08),
        ], // a
        [
            (570.0, 70.0, 1.0),
            (840.0, 80.0, 0.45),
            (2_410.0, 120.0, 0.12),
            (3_300.0, 150.0, 0.06),
        ], // o
        [
            (390.0, 60.0, 1.0),
            (2_000.0, 100.0, 0.35),
            (2_600.0, 120.0, 0.2),
            (3_500.0, 150.0, 0.08),
        ], // é
    ];
    let gap = 0.09;
    let total: f64 = phrase.iter().map(|p| p.2 + gap).sum::<f64>() + 0.5;
    let n = (total * sr) as usize;
    let mut src = vec![0.0f64; n];
    let mut vowel_at = vec![0usize; n];
    let mut noise = vec![0.0f64; n];
    let mut rng = 0x1234_5678_u32;
    let mut rand = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        f64::from(rng) / f64::from(u32::MAX) * 2.0 - 1.0
    };
    let mut t0 = 0.25;
    let mut phase = 0.0;
    for (k, &(note, cents, dur, vowel)) in phrase.iter().enumerate() {
        let start = (t0 * sr) as usize;
        let len = (dur * sr) as usize;
        // Consonant before the note.
        let c_len = (0.05 * sr) as usize;
        for i in start.saturating_sub(c_len)..start {
            let env = (PI * (i + c_len - start) as f64 / c_len as f64).sin();
            noise[i] += 0.12 * env * rand();
        }
        let prev = if k > 0 { phrase[k - 1].0 } else { note - 2.0 };
        let mut jit = 0.0;
        for i in 0..len {
            let t = i as f64 / sr;
            // Scoop from below (or from the previous note) over 70 ms.
            let scoop = (prev - note).clamp(-1.0, 1.0) * 0.6 * (-t / 0.035).exp();
            let vib_depth = 0.3 * ((t - 0.3) / 0.3).clamp(0.0, 1.0);
            let vib = vib_depth * (TAU * 5.6 * t).sin();
            jit += 0.02 * (rand() - jit);
            let m = note + cents / 100.0 + scoop + vib + 0.05 * jit;
            let hz = 440.0 * ((m - 69.0) / 12.0).exp2();
            phase += hz / sr;
            phase -= phase.floor();
            // Rosenberg-like glottal pulse, derivative taken by the
            // radiation below.
            let open = 0.6;
            let g = if phase < open {
                0.5 * (1.0 - (PI * phase / open).cos())
            } else {
                ((PI * (phase - open) / (2.0 * (1.0 - open))).cos()).max(0.0)
            };
            let env = (t / 0.03).min(1.0) * ((dur - t) / 0.04).clamp(0.0, 1.0);
            src[start + i] = g * env;
            vowel_at[start + i] = vowel;
        }
        t0 += dur + gap;
    }
    // Radiation (first difference), then the vowel's formants.
    let mut prev = 0.0;
    let mut out = vec![0.0f64; n];
    let mut states = [[0.0f64; 2]; 4];
    for i in 0..n {
        let x = src[i] - prev + 0.002 * rand() * src[i];
        prev = src[i];
        let v = &vowels[vowel_at[i]];
        let mut y = 0.0;
        for (st, &(f, bw, g)) in states.iter_mut().zip(v) {
            let r = (-PI * bw / sr).exp();
            let a1 = -2.0 * r * (TAU * f / sr).cos();
            let a2 = r * r;
            let o = (1.0 - r) * x - a1 * st[0] - a2 * st[1];
            st[1] = st[0];
            st[0] = o;
            y += g * o;
        }
        out[i] = y + noise[i];
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    out.iter().map(|v| (0.5 * v / peak) as f32).collect()
}

fn read_wav(path: &str) -> (u32, Vec<Vec<f32>>) {
    let b = fs::read(path).expect("read wav");
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32le = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    assert!(
        &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE",
        "not a WAV file"
    );
    let (mut fmt, mut chans, mut sr, mut bits, mut data) = (0, 0, 0, 0, (0, 0));
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32le(i + 4) as usize;
        if id == b"fmt " {
            fmt = u16le(i + 8);
            chans = usize::from(u16le(i + 10));
            sr = u32le(i + 12);
            bits = u16le(i + 22);
            if fmt == 0xFFFE {
                fmt = u16le(i + 32);
            }
        } else if id == b"data" {
            data = (i + 8, len.min(b.len() - i - 8));
        }
        i += 8 + len + (len & 1);
    }
    let bytes = usize::from(bits / 8);
    let frames = data.1 / (bytes * chans);
    let mut out = vec![Vec::with_capacity(frames); chans];
    for f in 0..frames {
        for (c, ch) in out.iter_mut().enumerate() {
            let at = data.0 + (f * chans + c) * bytes;
            #[allow(clippy::cast_precision_loss)]
            let v = match (fmt, bits) {
                (1, 16) => f32::from(i16::from_le_bytes([b[at], b[at + 1]])) / 32_768.0,
                (1, 24) => {
                    (i32::from_le_bytes([0, b[at], b[at + 1], b[at + 2]]) >> 8) as f32 / 8_388_608.0
                }
                (3, 32) => f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]),
                _ => panic!("unsupported WAV format {fmt}/{bits}"),
            };
            ch.push(v);
        }
    }
    (sr, out.into_iter().take(2).collect())
}

fn write_wav(path: &str, sr: u32, chans: &[Vec<f32>]) {
    let n = chans[0].len();
    #[allow(clippy::cast_possible_truncation)]
    let c = chans.len() as u16;
    #[allow(clippy::cast_possible_truncation)]
    let data_len = (n * chans.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&c.to_le_bytes());
    b.extend_from_slice(&sr.to_le_bytes());
    b.extend_from_slice(&(sr * 4 * u32::from(c)).to_le_bytes());
    b.extend_from_slice(&(4 * c).to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        for ch in chans {
            b.extend_from_slice(&ch[i].to_le_bytes());
        }
    }
    fs::write(path, b).expect("write wav");
}
