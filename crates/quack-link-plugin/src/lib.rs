//! Quackapella Link: the companion that sends a track to Quackapella.
//!
//! Put it on the instrumental bus and the mixing assistant hears the
//! rest of the song; put it on a synth and that synth plays the
//! vocoder. No side-chain routing, on any kind of track.
//!
//! The audio passes through untouched (no latency, nothing added): the
//! plugin only copies it to the wire (`quack-link`). The copy is
//! wait-free and nothing is allocated after `reset`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use quack_link::{Channel, LINKS, Role, Sender, Sent};
use quack_ui::link::{LinkState, LinkView};
use truce::prelude::*;
use truce_params::FloatParamReadF64;

mod editor;

/// Frames sent at a time (larger host blocks go in several passes).
const CHUNK: usize = 1024;

/// The roles, in `quack_link::Role` order.
#[derive(ParamEnum)]
pub enum RoleParam {
    #[name = "Mix"]
    Mix,
    #[name = "Carrier"]
    Carrier,
}

/// Link channels, in `quack_link::LINK_NAMES` order.
#[derive(ParamEnum)]
pub enum LinkParam {
    #[name = "A"]
    A,
    #[name = "B"]
    B,
}

#[derive(Params)]
pub struct LinkParams {
    #[param(id = 0, name = "Send As", default = 0)]
    pub role: EnumParam<RoleParam>,

    #[param(
        id = 1,
        name = "Send Level",
        range = "linear(-24, 12)",
        default = 0.0,
        unit = "dB",
        smooth = "none"
    )]
    pub level: FloatParam,

    #[param(id = 2, name = "Bypass", flags = "automatable | bypass")]
    pub bypass: BoolParam,

    /// Which Quackapella hears it: the one set on the same letter.
    #[param(id = 3, name = "Link", default = 0)]
    pub link: EnumParam<LinkParam>,

    /// Audio → editor. Not a parameter, not saved.
    #[skip]
    pub shared: Arc<LinkShared>,
}

/// What the audio thread publishes for the editor.
#[derive(Debug, Default)]
pub struct LinkShared {
    /// Peaks of what was sent since the editor last looked (f32 bits).
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    /// 0 off, 1 sending, 2 sending and heard, 3 role taken, 4 no wire.
    state: AtomicU32,
    /// Moves with every block: the host is running the plugin.
    blocks: AtomicU64,
}

impl LinkShared {
    fn peak(slot: &AtomicU32, x: f32) {
        if x > f32::from_bits(slot.load(Ordering::Relaxed)) {
            slot.store(x.to_bits(), Ordering::Relaxed);
        }
    }

    /// For the editor: the peaks are taken (and start again).
    #[must_use]
    pub fn view(&self) -> LinkView {
        LinkView {
            peak: [
                f32::from_bits(self.peak_l.swap(0, Ordering::Relaxed)),
                f32::from_bits(self.peak_r.swap(0, Ordering::Relaxed)),
            ],
            state: match self.state.load(Ordering::Relaxed) {
                1 => LinkState::Sending { heard: false },
                2 => LinkState::Sending { heard: true },
                3 => LinkState::Busy,
                4 => LinkState::Unavailable,
                _ => LinkState::Off,
            },
            blocks: self.blocks.load(Ordering::Relaxed),
        }
    }
}

/// Audio-thread state.
pub struct LinkDsp {
    /// One sender per link and role (link × 2 + role, roles in
    /// `Role::ALL` order; `None`: the system gave no shared memory).
    senders: [Option<Sender>; 2 * LINKS],
    left: Box<[f32; CHUNK]>,
    right: Box<[f32; CHUNK]>,
    /// Linear send gain at the end of the last block.
    gain: f32,
}

impl Default for LinkDsp {
    fn default() -> Self {
        Self {
            senders: Default::default(),
            left: Box::new([0.0; CHUNK]),
            right: Box::new([0.0; CHUNK]),
            gain: 1.0,
        }
    }
}

/// A number of this instance's own, for the wire.
fn instance_id(state: &LinkDsp) -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    #[allow(clippy::cast_possible_truncation)]
    let id = (t as u64) ^ (std::ptr::from_ref(state) as u64).rotate_left(32);
    id | 1
}

pub struct QuackLink;

impl PluginLogic for QuackLink {
    type Params = LinkParams;
    type DspState = LinkDsp;

    fn bus_layouts() -> Vec<BusLayout> {
        vec![BusLayout::stereo(), BusLayout::mono()]
    }

    fn reset(state: &mut LinkDsp, params: &LinkParams, config: &AudioConfig) {
        let id = instance_id(state);
        state.senders = std::array::from_fn(|i| {
            Channel::open_on(Role::ALL[i % 2], i / 2)
                .map(|ch| Sender::new(ch, id, config.sample_rate))
        });
        state.gain = db_to_gain(FloatParamReadF64::value(&params.level));
        params.shared.state.store(0, Ordering::Relaxed);
    }

    fn process(
        state: &mut LinkDsp,
        params: &LinkParams,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        _context: &mut ProcessContext,
    ) -> ProcessStatus {
        let shared = &params.shared;
        shared.blocks.fetch_add(1, Ordering::Relaxed);
        let total = buffer.num_samples();
        let (ins, outs) = (buffer.num_input_channels(), buffer.num_output_channels());
        if ins == 0 || outs == 0 {
            return ProcessStatus::Normal;
        }
        let role = usize::from(matches!(params.role.value(), RoleParam::Carrier))
            + 2 * usize::from(matches!(params.link.value(), LinkParam::B));
        let bypass = params.bypass.value();
        // The other roles and links are left to whoever wants them.
        for (i, s) in state.senders.iter().enumerate() {
            if let Some(s) = s
                && (i != role || bypass)
            {
                s.release();
            }
        }
        let target = db_to_gain(FloatParamReadF64::value(&params.level));
        let from = state.gain;
        let mut result = if state.senders[role].is_some() { 0 } else { 4 };
        let mut done = 0;
        while done < total {
            let n = (total - done).min(CHUNK);
            let (l, r) = (&mut state.left[..n], &mut state.right[..n]);
            l.copy_from_slice(&buffer.input(0)[done..done + n]);
            if ins > 1 {
                r.copy_from_slice(&buffer.input(1)[done..done + n]);
            } else {
                r.copy_from_slice(l);
            }
            // Through, untouched.
            buffer.output(0)[done..done + n].copy_from_slice(l);
            if outs > 1 {
                buffer.output(1)[done..done + n].copy_from_slice(r);
            }
            if !bypass && let Some(sender) = &mut state.senders[role] {
                // The level moves over the block, without a step.
                #[allow(clippy::cast_precision_loss)]
                for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
                    let t = (done + i + 1) as f32 / total as f32;
                    let g = from + (target - from) * t;
                    *a *= g;
                    *b *= g;
                }
                let sent = sender.send(l, r, 1.0);
                let heard = sender.listened(n);
                result = match sent {
                    Sent::Sent if heard => 2,
                    Sent::Sent => 1,
                    Sent::Busy => 3,
                };
                if sent == Sent::Sent {
                    let peak = |x: &[f32]| x.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
                    LinkShared::peak(&shared.peak_l, peak(l));
                    LinkShared::peak(&shared.peak_r, peak(r));
                }
            }
            done += n;
        }
        state.gain = target;
        shared
            .state
            .store(if bypass { 0 } else { result }, Ordering::Relaxed);
        ProcessStatus::Normal
    }

    fn latency(_state: &LinkDsp) -> u32 {
        0
    }

    fn editor(params: Arc<LinkParams>) -> Box<dyn Editor> {
        editor::build(params)
    }
}

fn db_to_gain(db: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let g = 10.0_f64.powf(db / 20.0) as f32;
    g
}

truce::plugin! {
    logic: QuackLink,
    params: LinkParams,
}

truce::enable_rt_paranoid!();
