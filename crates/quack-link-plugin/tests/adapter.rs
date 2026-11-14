//! Quackapella Link seen from a host: the audio passes through
//! untouched, what it sends arrives at the other end of the wire with
//! the level applied, and the audio thread does not allocate.
//!
//! One test, on channels of its own: the real ones belong to the
//! plugins running on the machine.

use quack_link::{Channel, Receiver, Role};
use quack_link_plugin::{LinkDsp, LinkParams, QuackLink};
use quack_ui::link::LinkState;
use truce::prelude::{AudioConfig, PluginLogic, ProcessContext, TransportInfo};
use truce_core::buffer::AudioBuffer;
use truce_core::events::EventList;

const SR: f64 = 48_000.0;
const N: usize = 64;

fn run(dsp: &mut LinkDsp, params: &LinkParams, l: &[f32], r: &[f32]) -> Vec<Vec<f32>> {
    let n = l.len();
    let inputs: Vec<&[f32]> = vec![l, r];
    let mut out = vec![vec![f32::NAN; n]; 2];
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    let mut buffer = AudioBuffer::from_slices_checked(&inputs, &mut outputs, n);
    let transport = TransportInfo::default();
    let mut out_events = EventList::default();
    let mut ctx = ProcessContext::new(&transport, SR, n, &mut out_events);
    QuackLink::process(dsp, params, &mut buffer, &EventList::default(), &mut ctx);
    out
}

#[test]
fn the_track_passes_through_and_reaches_quackapella() {
    quack_link::keep_private();
    let params = LinkParams::default();
    let mut dsp = LinkDsp::default();
    QuackLink::reset(&mut dsp, &params, &AudioConfig::new(SR, 512));
    assert_eq!(QuackLink::latency(&dsp), 0);
    let rx = |role| Receiver::new(Channel::open(role).expect("channel"), SR);
    let (mut mix, mut carrier) = (rx(Role::Mix), rx(Role::Carrier));

    // As the mix, at 0 dB: through untouched, and received as sent.
    let l: Vec<f32> = (0..N).map(|i| (i as f32 * 0.37).sin() * 0.5).collect();
    let r: Vec<f32> = l.iter().map(|x| -0.5 * x).collect();
    let (mut a, mut b) = (vec![0.0; N], vec![0.0; N]);
    let mut got = Vec::new();
    for _ in 0..40 {
        let out = run(&mut dsp, &params, &l, &r);
        assert_eq!(out[0], l);
        assert_eq!(out[1], r);
        assert!(mix.receive(&mut a, &mut b));
        assert!(!carrier.receive(&mut [0.0; N], &mut [0.0; N]));
        got.extend_from_slice(&a);
    }
    let peak = got.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.5).abs() < 0.01, "{peak}");
    assert_eq!(
        params.shared.view().state,
        LinkState::Sending { heard: true }
    );

    // As the carrier, 6 dB down: the mix is let go, the level applied.
    params.role.set_index(1);
    params.level.set_value(-6.0);
    let mut got = Vec::new();
    for _ in 0..40 {
        let out = run(&mut dsp, &params, &l, &r);
        assert_eq!(out[0], l);
        assert!(carrier.receive(&mut a, &mut b));
        got.extend_from_slice(&a);
    }
    let peak = got[got.len() - 10 * N..]
        .iter()
        .fold(0.0_f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.25).abs() < 0.01, "{peak}");
    // Half a second later the mix is known to be gone.
    for _ in 0..400 {
        mix.receive(&mut a, &mut b);
    }
    assert!(!mix.receive(&mut a, &mut b));

    // Bypassed: still through, nothing sent.
    params.bypass.set_value(true);
    let out = run(&mut dsp, &params, &l, &r);
    assert_eq!(out[0], l);
    assert_eq!(params.shared.view().state, LinkState::Off);
    params.bypass.set_value(false);

    // And the audio thread never allocates.
    run(&mut dsp, &params, &l, &r);
    truce_test::assert_no_audio_alloc(|| {
        for _ in 0..50 {
            run(&mut dsp, &params, &l, &r);
        }
    });
}
