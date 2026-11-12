//! Quackapella's voice engine.
//!
//! Everything that touches audio lives here, with no dependency on a
//! plugin framework, so the same code runs in the plugin, in tests and
//! in future offline tools. Rules for every module:
//!
//! - allocate only in `prepare` (never in `process` or event handlers);
//! - no locks, no I/O on the audio thread;
//! - parameters arrive as plain values through [`Engine::set_param`] and
//!   are smoothed here, not in the adapter.
//!
//! Module map (roadmap step in brackets, see `docs/architecture.md`):
//!
//! | Module | Role |
//! |---|---|
//! | [`params`] | Parameter registry: ids, ranges, defaults (source of truth) |
//! | [`chain`] | Layer routing: parallel or series, module order |
//! | [`engine`] | Signal chain, latency compensation, dry/wet, bypass [1] |
//! | [`latency`] | Latency per voice range, delay lines [1] |
//! | [`midi`] | Held notes from the MIDI input [1] |
//! | [`mode`] | Voice modes, including the *Natural* mode [1] |
//! | [`pitch`] | Real-time pitch detection (YIN), voicing, confidence [2] |
//! | [`shift`] | Pitch shifting with formant preservation [3] |
//! | [`autotune`] | Scales and note targeting [4] |
//! | [`creative`] | Robot and glitch of the Creative mode [5] |
//! | [`harmony`] | MIDI-driven harmony voices [6] |
//! | [`vocoder`] | Vocoder and talkbox [7] |
//! | [`denormal`] | Flush-to-zero while processing [9] |
//! | [`control`] | Macros, MIDI learn, program changes [8] |
//! | [`presets`] | Factory library and preset files [8] |
//! | [`snapshot`] | Snapshots A/B and the morph between them |
//! | [`midiout`] | Sung notes as MIDI output |

pub mod autotune;
pub mod chain;
pub mod control;
pub mod creative;
pub mod denormal;
pub mod engine;
pub mod harmony;
pub mod latency;
pub mod midi;
pub mod midiout;
pub mod mode;
pub mod params;
pub mod pitch;
pub mod presets;
pub mod shift;
pub mod smooth;
pub mod snapshot;
pub mod vocoder;

pub use engine::Engine;
pub use mode::VoiceMode;
pub use params::{ParamId, ParamKind, ParamSpec};

/// Decibels to linear gain.
#[inline]
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
