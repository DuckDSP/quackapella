//! Adapter tests: host declaration matches the registry, latency is
//! reported and real, MIDI reaches the engine at its offset, audio
//! thread does not allocate.

use quack_dsp::latency::VoiceRange;
use quack_dsp::{ParamId, ParamKind};
use quack_plugin::{Plugin, QuackDsp, QuackParams, Quackapella};
use truce::prelude::{AudioConfig, PluginLogic, ProcessContext, TransportInfo};
use truce_core::buffer::AudioBuffer;
use truce_core::events::{Event, EventBody, EventList};
use truce_core::export::PluginExport;
use truce_params::{ParamFlags, ParamRange, Params};

const SR: f64 = 48_000.0;

#[test]
fn host_params_match_the_registry() {
    let plugin = Plugin::create();
    let infos = plugin.params().param_infos();
    assert_eq!(infos.len(), ParamId::ALL.len());
    for (info, id) in infos.iter().zip(ParamId::ALL) {
        let spec = id.spec();
        assert_eq!(info.id, spec.host_id, "{}", spec.key);
        assert_eq!(info.name, spec.name, "{}", spec.key);
        assert!(
            (info.default_plain - spec.kind.default_plain()).abs() < 1e-12,
            "{}",
            spec.key
        );
        match spec.kind {
            ParamKind::Float { min, max, .. } => match info.range {
                ParamRange::Linear { min: a, max: b } => assert_eq!((a, b), (min, max)),
                ref other => panic!("{}: {other:?}", spec.key),
            },
            ParamKind::Enum { count, .. } => match info.range {
                ParamRange::Enum { count: c } => assert_eq!(c as u32, count),
                ref other => panic!("{}: {other:?}", spec.key),
            },
            ParamKind::Bool { .. } => {}
        }
    }
    let bypass = infos
        .iter()
        .find(|i| i.id == ParamId::Bypass.spec().host_id)
        .unwrap();
    assert!(bypass.flags.contains(ParamFlags::IS_BYPASS));
}

fn run(
    dsp: &mut QuackDsp,
    params: &QuackParams,
    input: &[f32],
    events: &EventList,
) -> Vec<Vec<f32>> {
    let n = input.len();
    let inputs: Vec<&[f32]> = vec![input, input];
    let mut out = vec![vec![f32::NAN; n]; 2];
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    let mut buffer = AudioBuffer::from_slices_checked(&inputs, &mut outputs, n);
    let transport = TransportInfo::default();
    let mut out_events = EventList::default();
    let mut ctx = ProcessContext::new(&transport, SR, n, &mut out_events);
    Quackapella::process(dsp, params, &mut buffer, events, &mut ctx);
    out
}

#[test]
fn reported_latency_is_the_real_one() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let params = QuackParams::default();
        let mut dsp = QuackDsp::default();
        Quackapella::reset(&mut dsp, &params, &AudioConfig::new(sr, 512));
        let lat = Quackapella::latency(&dsp) as usize;
        assert_eq!(
            lat,
            quack_dsp::latency::latency_samples(sr, VoiceRange::default(), false)
        );
        assert_eq!(params.shared.view().latency_samples as usize, lat);
        // Longer than one scratch pass.
        let mut x = vec![0.0f32; 3000];
        x[700] = 0.5;
        let out = run(&mut dsp, &params, &x, &EventList::default());
        for ch in &out {
            let at = ch.iter().position(|&v| v != 0.0).expect("impulse out");
            assert_eq!(at, 700 + lat, "{sr} Hz");
            assert!((ch[at] - 0.5).abs() < 1e-6);
        }
    }
}

#[test]
fn voice_range_moves_the_reported_latency() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 512));
    for (i, range) in VoiceRange::ALL.iter().enumerate() {
        params.voice.set_index(i as u32);
        let mut x = vec![0.0f32; 2048];
        // The range applies at the next block; the impulse comes after.
        run(&mut dsp, &params, &x, &EventList::default());
        let lat = Quackapella::latency(&dsp) as usize;
        assert_eq!(lat, quack_dsp::latency::latency_samples(SR, *range, false));
        assert_eq!(params.shared.view().latency_samples as usize, lat);
        x[100] = 0.5;
        let out = run(&mut dsp, &params, &x, &EventList::default());
        let at = out[0].iter().position(|&v| v != 0.0).expect("impulse out");
        assert_eq!(at, 100 + lat, "{}", range.name());
    }
    // Low Latency: shorter, still the real one.
    params.low_latency.set_value(true);
    let mut x = vec![0.0f32; 2048];
    run(&mut dsp, &params, &x, &EventList::default());
    let lat = Quackapella::latency(&dsp) as usize;
    assert_eq!(
        lat,
        quack_dsp::latency::latency_samples(SR, VoiceRange::Bass, true)
    );
    x[100] = 0.5;
    let out = run(&mut dsp, &params, &x, &EventList::default());
    let at = out[0].iter().position(|&v| v != 0.0).expect("impulse out");
    assert_eq!(at, 100 + lat);
}

#[test]
fn midi_notes_reach_the_engine() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 512));
    let mut ev = EventList::with_capacity(8);
    let on = |note| EventBody::NoteOn {
        group: 0,
        channel: 0,
        note,
        velocity: 100,
    };
    ev.push(Event::new(10, on(60)));
    ev.push(Event::new(20, on(64)));
    ev.push(Event::new(
        30,
        EventBody::NoteOff {
            group: 0,
            channel: 0,
            note: 60,
            velocity: 0,
        },
    ));
    run(&mut dsp, &params, &[0.0; 256], &ev);
    assert_eq!(params.shared.view().held_notes, 1u128 << 64);

    let mut ev = EventList::with_capacity(1);
    ev.push(Event::new(
        0,
        EventBody::ControlChange {
            group: 0,
            channel: 0,
            cc: 123,
            value: 0,
        },
    ));
    run(&mut dsp, &params, &[0.0; 64], &ev);
    assert_eq!(params.shared.view().held_notes, 0);
}

#[test]
fn detected_pitch_reaches_the_editor() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 512));
    #[allow(clippy::cast_precision_loss)]
    let x: Vec<f32> = (0..9_600)
        .map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / SR as f32).sin())
        .collect();
    run(&mut dsp, &params, &x, &EventList::default());
    let v = params.shared.view();
    assert!((v.pitch_hz - 220.0).abs() < 1.0, "{}", v.pitch_hz);
    assert!(v.pitch_confidence > 0.9);
    run(&mut dsp, &params, &[0.0; 9_600], &EventList::default());
    assert_eq!(params.shared.view().pitch_hz, 0.0);
}

#[cfg(feature = "rt-paranoid")]
#[test]
fn process_does_not_allocate() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 4096));
    let x = vec![0.1f32; 4096];
    let mut ev = EventList::with_capacity(4);
    ev.push(Event::new(
        5,
        EventBody::NoteOn {
            group: 0,
            channel: 0,
            note: 62,
            velocity: 90,
        },
    ));
    // Buffers are built outside the checked region.
    let inputs: Vec<&[f32]> = vec![&x, &x];
    let mut out = vec![vec![0.0f32; 4096]; 2];
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    let mut buffer = AudioBuffer::from_slices_checked(&inputs, &mut outputs, 4096);
    let transport = TransportInfo::default();
    let mut out_events = EventList::with_capacity(16);
    truce_test::assert_no_audio_alloc(|| {
        let mut ctx = ProcessContext::new(&transport, SR, 4096, &mut out_events);
        Quackapella::process(&mut dsp, &params, &mut buffer, &ev, &mut ctx);
    });
}

#[test]
fn learned_cc_and_program_change_move_the_parameters() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 512));
    params.control.arm(ParamId::Retune);
    let cc = |cc, value| EventBody::ControlChange {
        group: 0,
        channel: 0,
        cc,
        value,
    };
    let mut ev = EventList::with_capacity(2);
    ev.push(Event::new(0, cc(21, 127)));
    run(&mut dsp, &params, &[0.0; 64], &ev);
    assert_eq!(params.control.cc_for(ParamId::Retune), Some(21));
    let retune = params.get_plain(ParamId::Retune.spec().host_id).unwrap();
    assert!((retune - 400.0).abs() < 1e-6, "{retune}");

    // Program changes only when switched on; the setup stays.
    params.set_plain(ParamId::Voice.spec().host_id, 3.0);
    let pc = |program| EventBody::ProgramChange {
        group: 0,
        channel: 0,
        program,
    };
    let hard = quack_dsp::presets::FACTORY
        .iter()
        .position(|f| f.name == "Hard Tune")
        .unwrap();
    #[allow(clippy::cast_possible_truncation)]
    let program = hard as u8;
    let mut ev = EventList::with_capacity(1);
    ev.push(Event::new(0, pc(program)));
    run(&mut dsp, &params, &[0.0; 64], &ev);
    assert_eq!(params.control.factory(), None);
    params.control.set_program_change(true);
    run(&mut dsp, &params, &[0.0; 64], &ev);
    assert_eq!(params.control.factory(), Some(hard));
    assert_eq!(params.get_plain(ParamId::Retune.spec().host_id), Some(0.0));
    assert_eq!(params.get_plain(ParamId::Mode.spec().host_id), Some(1.0));
    assert_eq!(params.get_plain(ParamId::Voice.spec().host_id), Some(3.0));
    assert_eq!(params.control.names()[0], "Hardness");
}

#[test]
fn control_state_is_saved_with_the_session() {
    let a = QuackParams::default();
    a.control.bind(7, ParamId::Macro2);
    a.control.set_depth(1, ParamId::Formant, -40);
    a.control.set_name(1, "Darker");
    let blob = a.serialize_persist();
    let b = QuackParams::default();
    b.load_persist(&blob);
    assert_eq!(b.control.cc_for(ParamId::Macro2), Some(7));
    assert_eq!(b.control.depth(1, ParamId::Formant), -40);
    assert_eq!(b.control.names()[1], "Darker");
}

#[test]
fn macros_reach_the_engine_values() {
    let params = QuackParams::default();
    params.control.set_depth(0, ParamId::Pitch, 25);
    params.set_plain(ParamId::Macro1.spec().host_id, 100.0);
    let v = params.engine_values();
    assert!((v.get(ParamId::Pitch) - 12.0).abs() < 1e-9);
    assert_eq!(params.to_values().get(ParamId::Pitch), 0.0);
}

/// Like `run`, returning the MIDI the plugin sent.
fn run_out(
    dsp: &mut QuackDsp,
    params: &QuackParams,
    input: &[f32],
    events: &EventList,
) -> Vec<EventBody> {
    let n = input.len();
    let inputs: Vec<&[f32]> = vec![input, input];
    let mut out = vec![vec![0.0; n]; 2];
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    let mut buffer = AudioBuffer::from_slices_checked(&inputs, &mut outputs, n);
    let transport = TransportInfo::default();
    let mut out_events = EventList::with_capacity(256);
    let mut ctx = ProcessContext::new(&transport, SR, n, &mut out_events);
    Quackapella::process(dsp, params, &mut buffer, events, &mut ctx);
    out_events.iter().map(|e| e.body).collect()
}

fn plain(params: &QuackParams, id: ParamId) -> f64 {
    params.get_plain(id.spec().host_id).unwrap()
}

fn set(params: &QuackParams, id: ParamId, v: f64) {
    params.set_plain(id.spec().host_id, v);
}

#[test]
fn morph_writes_the_parameters_between_the_snapshots() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 64));
    set(&params, ParamId::Pitch, -12.0);
    set(&params, ParamId::HarmonyOn, 0.0);
    set(&params, ParamId::Retune, 80.0);
    params.control.store_snapshot(0, &params.to_values());
    set(&params, ParamId::Pitch, 12.0);
    set(&params, ParamId::HarmonyOn, 1.0);
    params.control.store_snapshot(1, &params.to_values());
    set(&params, ParamId::MorphOn, 1.0);
    set(&params, ParamId::Morph, 25.0);
    // A hand tweak on a control A and B share holds.
    set(&params, ParamId::Retune, 10.0);
    run(&mut dsp, &params, &[0.0; 64], &EventList::default());
    assert!((plain(&params, ParamId::Pitch) + 6.0).abs() < 1e-6);
    assert_eq!(plain(&params, ParamId::HarmonyOn), 0.0);
    assert_eq!(plain(&params, ParamId::Retune), 10.0);
    // A macro on the morph moves it too.
    params.control.set_depth(0, ParamId::Morph, 50);
    set(&params, ParamId::Macro1, 100.0);
    run(&mut dsp, &params, &[0.0; 64], &EventList::default());
    assert!((plain(&params, ParamId::Pitch) - 6.0).abs() < 1e-6);
    assert_eq!(plain(&params, ParamId::HarmonyOn), 1.0);
}

#[test]
fn snapshot_keys_glide_and_go_no_further() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 64));
    set(&params, ParamId::Pitch, 0.0);
    params.control.store_snapshot(0, &params.to_values());
    set(&params, ParamId::Pitch, 10.0);
    params.control.store_snapshot(1, &params.to_values());
    set(&params, ParamId::SnapshotKeys, 2.0); // C0 / C#0
    set(&params, ParamId::MorphOn, 1.0);
    set(&params, ParamId::MorphTime, 100.0);
    set(&params, ParamId::Morph, 0.0);
    let mut ev = EventList::with_capacity(1);
    ev.push(Event::new(
        0,
        EventBody::NoteOn {
            group: 0,
            channel: 0,
            note: 13,
            velocity: 100,
        },
    ));
    run(&mut dsp, &params, &[0.0; 64], &ev);
    assert_eq!(params.shared.view().held_notes, 0, "the key is not a note");
    // 100 ms at 48 kHz: 75 blocks of 64.
    for _ in 0..40 {
        run(&mut dsp, &params, &[0.0; 64], &EventList::default());
    }
    let mid = plain(&params, ParamId::Morph);
    assert!(mid > 30.0 && mid < 70.0, "{mid}");
    for _ in 0..40 {
        run(&mut dsp, &params, &[0.0; 64], &EventList::default());
    }
    assert_eq!(plain(&params, ParamId::Morph), 100.0);
    assert!((plain(&params, ParamId::Pitch) - 10.0).abs() < 1e-6);

    // Morph off: the A key recalls A at once.
    set(&params, ParamId::MorphOn, 0.0);
    let mut ev = EventList::with_capacity(1);
    ev.push(Event::new(
        0,
        EventBody::NoteOn {
            group: 0,
            channel: 0,
            note: 12,
            velocity: 100,
        },
    ));
    run(&mut dsp, &params, &[0.0; 64], &ev);
    assert_eq!(plain(&params, ParamId::Pitch), 0.0);
}

#[test]
fn midi_out_plays_the_sung_note() {
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 64));
    set(&params, ParamId::MidiOut, 1.0);
    set(&params, ParamId::MidiOutChannel, 2.0);
    // A3 (220 Hz), 0.5 s.
    let hz = 220.0_f32;
    let mut sent = Vec::new();
    let mut first_on = None;
    for b in 0..375 {
        let x: Vec<f32> = (0..64)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = (b * 64 + i) as f32 / SR as f32;
                0.4 * (std::f32::consts::TAU * hz * t).sin()
            })
            .collect();
        let ev = run_out(&mut dsp, &params, &x, &EventList::default());
        if first_on.is_none() && !ev.is_empty() {
            first_on = Some(b);
        }
        sent.extend(ev);
    }
    let ons: Vec<_> = sent
        .iter()
        .filter_map(|e| match *e {
            EventBody::NoteOn { channel, note, .. } => Some((channel, note)),
            _ => None,
        })
        .collect();
    assert_eq!(ons, vec![(2, 57)], "{sent:?}");
    // Detection time: well under 40 ms.
    let ms = first_on.unwrap() as f64 * 64.0 / SR * 1000.0;
    eprintln!("MIDI out note-on {ms:.1} ms after the onset");
    assert!(ms < 40.0, "{ms} ms");
    // Silence: the note ends.
    let mut tail = Vec::new();
    for _ in 0..60 {
        tail.extend(run_out(
            &mut dsp,
            &params,
            &[0.0; 64],
            &EventList::default(),
        ));
    }
    assert!(tail.iter().any(|e| matches!(
        e,
        EventBody::NoteOff {
            note: 57,
            channel: 2,
            ..
        }
    )));
    // Switched off with a note on: released.
    set(&params, ParamId::MidiOut, 0.0);
    assert!(run_out(&mut dsp, &params, &[0.0; 64], &EventList::default()).is_empty());
}

/// How long the MIDI output takes to start a note, per voice range
/// (`cargo test -p quack-plugin --test adapter -- --nocapture`).
#[test]
fn midi_out_onset_per_voice_range() {
    for (i, range) in VoiceRange::ALL.iter().enumerate() {
        for low in [false, true] {
            let params = QuackParams::default();
            let mut dsp = QuackDsp::default();
            params.voice.set_index(i as u32);
            params.low_latency.set_value(low);
            Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 64));
            set(&params, ParamId::MidiOut, 1.0);
            // A singer starts after some silence.
            for _ in 0..150 {
                run_out(&mut dsp, &params, &[0.0; 64], &EventList::default());
            }
            let hz = match i {
                0 => 440.0_f32,
                1 => 220.0,
                2 => 165.0,
                _ => 110.0,
            };
            let mut at = None;
            for b in 0..200 {
                let x: Vec<f32> = (0..64)
                    .map(|k| {
                        #[allow(clippy::cast_precision_loss)]
                        let t = (b * 64 + k) as f32 / SR as f32;
                        0.4 * (std::f32::consts::TAU * hz * t).sin()
                    })
                    .collect();
                if !run_out(&mut dsp, &params, &x, &EventList::default()).is_empty() {
                    at = Some(b);
                    break;
                }
            }
            let ms = at.expect("a note") as f64 * 64.0 / SR * 1000.0;
            eprintln!("{} {hz} Hz low latency {low}: {ms:.1} ms", range.name());
            assert!(ms < 25.0);
        }
    }
}

/// What a Quackapella Link sends as the mix reaches the assistant's
/// tap. (Only the mix here: these tests run side by side and share the
/// machine's channels; a carrier would land in the others' side-chain.)
#[test]
fn a_linked_mix_reaches_its_tap() {
    use quack_link::{Channel, Role, Sender};
    let params = QuackParams::default();
    let mut dsp = QuackDsp::default();
    Quackapella::reset(&mut dsp, &params, &AudioConfig::new(SR, 512));
    let x = vec![0.0f32; 64];
    run(&mut dsp, &params, &x, &EventList::default());
    assert_eq!(params.shared.view().link, [false, false]);

    let mut tx = Sender::new(Channel::open(Role::Mix).expect("channel"), 77, SR);
    let tone: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin() * 0.4).collect();
    for _ in 0..80 {
        tx.send(&tone, &tone, 1.0);
        run(&mut dsp, &params, &x, &EventList::default());
    }
    assert_eq!(params.shared.view().link, [true, false]);
    let mut heard = vec![0.0f32; 1024];
    params.shared.taps.link.read(&mut heard);
    let peak = heard.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.4).abs() < 0.02, "{peak}");
    assert!(tx.listened(64));
}
