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
            quack_dsp::latency::latency_samples(sr, VoiceRange::default())
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
        assert_eq!(lat, quack_dsp::latency::latency_samples(SR, *range));
        assert_eq!(params.shared.view().latency_samples as usize, lat);
        x[100] = 0.5;
        let out = run(&mut dsp, &params, &x, &EventList::default());
        let at = out[0].iter().position(|&v| v != 0.0).expect("impulse out");
        assert_eq!(at, 100 + lat, "{}", range.name());
    }
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
