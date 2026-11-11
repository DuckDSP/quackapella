//! Performance controls that sit above the parameters: the four macros,
//! MIDI learn and program changes.
//!
//! [`ControlState`] is shared by the audio thread (MIDI CCs, macros
//! applied to the values each block) and the editor (assignments). Every
//! field the audio thread touches is an atomic: no lock, no allocation
//! there. The macro names sit behind a mutex only the editor and the
//! state save take.
//!
//! Macros: each of the four knobs ([`MACRO_PARAMS`], host ids 700-703)
//! moves up to [`MACRO_SLOTS`] parameters, each by its own depth (-100 to
//! +100 % of the target's range at full macro). The target's own value
//! is the base; the macro adds on top, clamped to the range. The engine
//! only ever sees the result, and the host keeps automating both.
//!
//! MIDI learn: one CC per parameter (macros included). The editor arms a
//! parameter, the next CC received binds to it. CC 64 (sustain) and the
//! channel mode messages (120-127) stay reserved.

use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};

use crate::params::{MACRO_PARAMS, PARAM_COUNT, ParamId, ParamValues};

pub const MACRO_COUNT: usize = 4;
/// Targets per macro.
pub const MACRO_SLOTS: usize = 8;

/// CCs MIDI learn never takes: sustain, and the channel mode messages.
#[must_use]
pub const fn cc_reserved(cc: u8) -> bool {
    cc == 64 || cc >= 120
}

/// Whether a macro may move this parameter. Not the mode (a jump in the
/// whole chain), bypass, the voice range and the latency switch (they
/// change the latency the host compensates), nor the macros themselves.
#[must_use]
pub fn can_modulate(id: ParamId) -> bool {
    !matches!(
        id,
        ParamId::Mode
            | ParamId::Bypass
            | ParamId::Voice
            | ParamId::LowLatency
            | ParamId::Macro1
            | ParamId::Macro2
            | ParamId::Macro3
            | ParamId::Macro4
    )
}

/// One macro target as stored: parameter and depth in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub id: ParamId,
    /// -100..=100, never 0.
    pub depth: i8,
}

fn pack(t: Option<Target>) -> u16 {
    #[allow(clippy::cast_possible_truncation)]
    t.map_or(0, |t| {
        ((t.id.index() as u16 + 1) << 8) | u16::from(t.depth.cast_unsigned())
    })
}

fn unpack(v: u16) -> Option<Target> {
    let idx = usize::from(v >> 8);
    if idx == 0 || idx > PARAM_COUNT {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let depth = (v & 0xff) as u8 as i8;
    (depth != 0).then(|| Target {
        id: ParamId::ALL[idx - 1],
        depth,
    })
}

/// Default macro names.
pub const DEFAULT_MACRO_NAMES: [&str; MACRO_COUNT] = ["Macro 1", "Macro 2", "Macro 3", "Macro 4"];

/// No factory preset (a user preset, or nothing loaded).
pub const NO_FACTORY: u32 = u32::MAX;

#[derive(Debug)]
pub struct ControlState {
    /// CC → parameter index + 1 (0 = not learned).
    learn: [AtomicU8; 128],
    /// Parameter index + 1 waiting for a CC (0 = none).
    armed: AtomicU8,
    /// Last CC received + 1 (0 = none yet), for the editor.
    last_cc: AtomicU8,
    slots: [[AtomicU16; MACRO_SLOTS]; MACRO_COUNT],
    names: Mutex<[String; MACRO_COUNT]>,
    /// Factory preset whose macro names are still to be copied into
    /// `names` (+1, 0 = none): set by a program change on the audio
    /// thread, which may not take the mutex.
    pending_names: AtomicU32,
    /// Program changes recall factory presets.
    program_change: AtomicBool,
    /// Factory preset loaded last, or [`NO_FACTORY`].
    factory: AtomicU32,
    /// Bumped by every preset recall (editor, program change, state
    /// load): the editor compares against what it showed.
    recall: AtomicU32,
    /// Editor zoom, percent.
    zoom: AtomicU16,
    /// User preset loaded last (editor only).
    user_preset: Mutex<String>,
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            learn: std::array::from_fn(|_| AtomicU8::new(0)),
            armed: AtomicU8::new(0),
            last_cc: AtomicU8::new(0),
            slots: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU16::new(0))),
            names: Mutex::new(DEFAULT_MACRO_NAMES.map(str::to_owned)),
            pending_names: AtomicU32::new(0),
            program_change: AtomicBool::new(false),
            factory: AtomicU32::new(NO_FACTORY),
            recall: AtomicU32::new(0),
            zoom: AtomicU16::new(100),
            user_preset: Mutex::new(String::new()),
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn idx1(id: ParamId) -> u8 {
    id.index() as u8 + 1
}

fn from_idx1(v: u8) -> Option<ParamId> {
    let i = usize::from(v);
    (1..=PARAM_COUNT).contains(&i).then(|| ParamId::ALL[i - 1])
}

impl ControlState {
    // ---- MIDI learn -------------------------------------------------

    /// The next CC received binds to `id`.
    pub fn arm(&self, id: ParamId) {
        self.armed.store(idx1(id), Ordering::Relaxed);
    }

    pub fn disarm(&self) {
        self.armed.store(0, Ordering::Relaxed);
    }

    #[must_use]
    pub fn armed(&self) -> Option<ParamId> {
        from_idx1(self.armed.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn last_cc(&self) -> Option<u8> {
        self.last_cc.load(Ordering::Relaxed).checked_sub(1)
    }

    /// The CC bound to `id`.
    #[must_use]
    pub fn cc_for(&self, id: ParamId) -> Option<u8> {
        let want = idx1(id);
        #[allow(clippy::cast_possible_truncation)]
        self.learn
            .iter()
            .position(|c| c.load(Ordering::Relaxed) == want)
            .map(|cc| cc as u8)
    }

    #[must_use]
    pub fn learned(&self, cc: u8) -> Option<ParamId> {
        from_idx1(self.learn.get(usize::from(cc))?.load(Ordering::Relaxed))
    }

    /// Bind `cc` to `id` (one CC per parameter, one parameter per CC).
    pub fn bind(&self, cc: u8, id: ParamId) {
        if cc_reserved(cc) || usize::from(cc) >= self.learn.len() {
            return;
        }
        self.forget(id);
        self.learn[usize::from(cc)].store(idx1(id), Ordering::Relaxed);
    }

    pub fn forget(&self, id: ParamId) {
        let want = idx1(id);
        for c in &self.learn {
            let _ = c.compare_exchange(want, 0, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    pub fn forget_all(&self) {
        for c in &self.learn {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Learned bindings, CC order.
    #[must_use]
    pub fn bindings(&self) -> Vec<(u8, ParamId)> {
        (0..128u8)
            .filter_map(|cc| self.learned(cc).map(|id| (cc, id)))
            .collect()
    }

    /// A CC from the MIDI input (audio thread): binds it if a parameter
    /// is armed, then returns the parameter it drives and its new
    /// normalized value.
    pub fn on_cc(&self, cc: u8, value: u8) -> Option<(ParamId, f64)> {
        if cc_reserved(cc) || cc >= 128 {
            return None;
        }
        self.last_cc.store(cc + 1, Ordering::Relaxed);
        let armed = self.armed.swap(0, Ordering::Relaxed);
        if let Some(id) = from_idx1(armed) {
            self.bind(cc, id);
        }
        let id = self.learned(cc)?;
        Some((id, f64::from(value.min(127)) / 127.0))
    }

    // ---- Macros -----------------------------------------------------

    /// Targets of macro `m`.
    #[must_use]
    pub fn targets(&self, m: usize) -> Vec<Target> {
        self.slots[m]
            .iter()
            .filter_map(|s| unpack(s.load(Ordering::Relaxed)))
            .collect()
    }

    /// Depth of `id` on macro `m`, percent (0 = not a target).
    #[must_use]
    pub fn depth(&self, m: usize, id: ParamId) -> i8 {
        self.slots[m]
            .iter()
            .filter_map(|s| unpack(s.load(Ordering::Relaxed)))
            .find(|t| t.id == id)
            .map_or(0, |t| t.depth)
    }

    /// Set the depth of `id` on macro `m` (0 removes it). Returns false
    /// when the macro has no free slot or `id` cannot be modulated.
    pub fn set_depth(&self, m: usize, id: ParamId, depth: i8) -> bool {
        if m >= MACRO_COUNT || !can_modulate(id) {
            return false;
        }
        let depth = depth.clamp(-100, 100);
        let slots = &self.slots[m];
        let at = slots
            .iter()
            .position(|s| unpack(s.load(Ordering::Relaxed)).is_some_and(|t| t.id == id));
        let new = (depth != 0).then_some(Target { id, depth });
        if let Some(i) = at {
            slots[i].store(pack(new), Ordering::Relaxed);
            return true;
        }
        if new.is_none() {
            return true;
        }
        match slots
            .iter()
            .position(|s| unpack(s.load(Ordering::Relaxed)).is_none())
        {
            Some(i) => {
                slots[i].store(pack(new), Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Replace macro `m`'s targets (extra ones are dropped).
    pub fn set_targets(&self, m: usize, targets: &[(ParamId, f64)]) {
        let mut it = targets
            .iter()
            .filter(|(id, d)| can_modulate(*id) && d.abs() >= 0.005)
            .map(|&(id, d)| {
                #[allow(clippy::cast_possible_truncation)]
                let depth = (d * 100.0).round().clamp(-100.0, 100.0) as i8;
                Target { id, depth }
            });
        for s in &self.slots[m] {
            s.store(pack(it.next()), Ordering::Relaxed);
        }
    }

    pub fn clear_macros(&self) {
        for m in 0..MACRO_COUNT {
            self.set_targets(m, &[]);
        }
    }

    /// Whether any macro moves `id`.
    #[must_use]
    pub fn modulated(&self, id: ParamId) -> bool {
        (0..MACRO_COUNT).any(|m| self.depth(m, id) != 0)
    }

    /// Apply the macros to plain values (audio thread, every block):
    /// each target moves by depth × macro × its range, clamped.
    pub fn apply_macros(&self, v: &mut ParamValues) {
        for (m, slots) in self.slots.iter().enumerate() {
            let amount = v.get(MACRO_PARAMS[m]) / 100.0;
            if amount <= 0.0 {
                continue;
            }
            for s in slots {
                if let Some(t) = unpack(s.load(Ordering::Relaxed)) {
                    let k = t.id.spec().kind;
                    let span = k.max() - k.min();
                    v.set(
                        t.id,
                        v.get(t.id) + f64::from(t.depth) / 100.0 * amount * span,
                    );
                }
            }
        }
    }

    // ---- Names, presets, editor -----------------------------------

    /// Macro names (resolves a program change's names first).
    #[must_use]
    pub fn names(&self) -> [String; MACRO_COUNT] {
        let mut names = self
            .names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.resolve_pending(&mut names);
        names.clone()
    }

    fn resolve_pending(&self, names: &mut [String; MACRO_COUNT]) {
        let p = self.pending_names.swap(0, Ordering::Relaxed);
        if let Some(f) = p
            .checked_sub(1)
            .and_then(|i| crate::presets::FACTORY.get(i as usize))
        {
            for (n, m) in names.iter_mut().zip(&f.macros) {
                m.name.clone_into(n);
            }
        }
    }

    pub fn set_name(&self, m: usize, name: &str) {
        let mut names = self
            .names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.resolve_pending(&mut names);
        if let Some(n) = names.get_mut(m) {
            let name = name.trim();
            if name.is_empty() {
                DEFAULT_MACRO_NAMES[m].clone_into(n);
            } else {
                name.chars().take(24).collect::<String>().clone_into(n);
            }
        }
    }

    #[must_use]
    pub fn program_change(&self) -> bool {
        self.program_change.load(Ordering::Relaxed)
    }

    pub fn set_program_change(&self, on: bool) {
        self.program_change.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn zoom(&self) -> u16 {
        self.zoom.load(Ordering::Relaxed)
    }

    pub fn set_zoom(&self, pct: u16) {
        self.zoom.store(pct.clamp(50, 200), Ordering::Relaxed);
    }

    /// Factory preset loaded last.
    #[must_use]
    pub fn factory(&self) -> Option<usize> {
        let f = self.factory.load(Ordering::Relaxed);
        (f != NO_FACTORY).then_some(f as usize)
    }

    /// User preset loaded last (empty when none).
    #[must_use]
    pub fn user_preset(&self) -> String {
        self.user_preset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn recall_count(&self) -> u32 {
        self.recall.load(Ordering::Relaxed)
    }

    /// Record which preset is loaded (editor thread).
    pub fn set_loaded(&self, factory: Option<usize>, user: &str) {
        #[allow(clippy::cast_possible_truncation)]
        self.factory
            .store(factory.map_or(NO_FACTORY, |f| f as u32), Ordering::Relaxed);
        user.clone_into(
            &mut self
                .user_preset
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.recall.fetch_add(1, Ordering::Relaxed);
    }

    /// Recall factory preset `i`'s macros (audio thread safe: atomics
    /// only; the names follow when the editor or the state save reads
    /// them). The caller sets the parameter values.
    pub fn recall_factory_macros(&self, i: usize) {
        let Some(f) = crate::presets::FACTORY.get(i) else {
            return;
        };
        for (m, def) in f.macros.iter().enumerate() {
            self.set_targets(m, def.targets);
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            self.pending_names.store(i as u32 + 1, Ordering::Relaxed);
            self.factory.store(i as u32, Ordering::Relaxed);
        }
        self.recall.fetch_add(1, Ordering::Relaxed);
    }

    // ---- Saved state ------------------------------------------------

    /// The state the host saves with the session, as text keyed by
    /// parameter key (stable across versions).
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let learn: Vec<String> = self
            .bindings()
            .into_iter()
            .map(|(cc, id)| format!("{cc}:{}", id.spec().key))
            .collect();
        let _ = writeln!(s, "learn = {}", learn.join(", "));
        write_macros(&mut s, &self.names(), |m| self.targets(m));
        let _ = writeln!(s, "program_change = {}", u8::from(self.program_change()));
        let _ = writeln!(s, "zoom = {}", self.zoom());
        if let Some(f) = self.factory() {
            let _ = writeln!(s, "factory = {f}");
        }
        let user = self.user_preset();
        if !user.is_empty() {
            let _ = writeln!(s, "user_preset = {user}");
        }
        s
    }

    /// Restore from [`Self::to_text`]. Unknown lines are skipped.
    pub fn load_text(&self, text: &str) {
        self.forget_all();
        self.clear_macros();
        self.disarm();
        let mut names = DEFAULT_MACRO_NAMES.map(str::to_owned);
        let mut factory = None;
        let mut user = String::new();
        for (k, v) in lines(text) {
            match k {
                "learn" => {
                    for item in v.split(',') {
                        if let Some((cc, key)) = item.trim().split_once(':')
                            && let (Ok(cc), Some(id)) =
                                (cc.trim().parse::<u8>(), by_key(key.trim()))
                        {
                            self.bind(cc, id);
                        }
                    }
                }
                "program_change" => self.set_program_change(v == "1"),
                "zoom" => {
                    if let Ok(z) = v.parse() {
                        self.set_zoom(z);
                    }
                }
                "factory" => factory = v.parse::<usize>().ok(),
                "user_preset" => v.clone_into(&mut user),
                _ => {
                    if let Some((m, field)) = macro_key(k) {
                        match field {
                            "name" => v.clone_into(&mut names[m]),
                            "targets" => self.set_targets(m, &parse_targets(v)),
                            _ => {}
                        }
                    }
                }
            }
        }
        *self
            .names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = names;
        self.pending_names.store(0, Ordering::Relaxed);
        self.set_loaded(factory, &user);
    }
}

/// `key = value` lines, comments (`#`) and blanks skipped.
pub(crate) fn lines(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            return None;
        }
        let (k, v) = l.split_once('=')?;
        Some((k.trim(), v.trim()))
    })
}

#[must_use]
pub fn by_key(key: &str) -> Option<ParamId> {
    ParamId::ALL.into_iter().find(|p| p.spec().key == key)
}

/// `macro_2.name` → (1, "name").
pub(crate) fn macro_key(k: &str) -> Option<(usize, &str)> {
    let rest = k.strip_prefix("macro_")?;
    let (n, field) = rest.split_once('.')?;
    let n: usize = n.parse().ok()?;
    (1..=MACRO_COUNT).contains(&n).then_some((n - 1, field))
}

/// `pitch:+50, formant:-25` → depths as fractions.
pub(crate) fn parse_targets(v: &str) -> Vec<(ParamId, f64)> {
    v.split(',')
        .filter_map(|item| {
            let (key, d) = item.trim().split_once(':')?;
            let id = by_key(key.trim())?;
            let d: f64 = d.trim().parse().ok()?;
            Some((id, d / 100.0))
        })
        .collect()
}

pub(crate) fn write_macros(
    s: &mut String,
    names: &[String; MACRO_COUNT],
    targets: impl Fn(usize) -> Vec<Target>,
) {
    for (m, name) in names.iter().enumerate() {
        let n = m + 1;
        let _ = writeln!(s, "macro_{n}.name = {name}");
        let t: Vec<String> = targets(m)
            .into_iter()
            .map(|t| format!("{}:{:+}", t.id.spec().key, t.depth))
            .collect();
        let _ = writeln!(s, "macro_{n}.targets = {}", t.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learn_binds_the_next_cc_once_per_parameter() {
        let c = ControlState::default();
        assert_eq!(c.on_cc(1, 64), None);
        c.arm(ParamId::Retune);
        let (id, v) = c.on_cc(11, 127).expect("bound");
        assert_eq!(id, ParamId::Retune);
        assert!((v - 1.0).abs() < 1e-9);
        assert_eq!(c.armed(), None);
        assert_eq!(c.cc_for(ParamId::Retune), Some(11));
        // Re-learning moves the binding.
        c.arm(ParamId::Retune);
        c.on_cc(12, 0);
        assert_eq!(c.cc_for(ParamId::Retune), Some(12));
        assert_eq!(c.learned(11), None);
        // Sustain and mode messages are never learned.
        c.arm(ParamId::Pitch);
        assert_eq!(c.on_cc(64, 127), None);
        assert_eq!(c.cc_for(ParamId::Pitch), None);
    }

    #[test]
    fn macros_add_depth_times_range_and_clamp() {
        let c = ControlState::default();
        assert!(c.set_depth(0, ParamId::Pitch, 50));
        assert!(c.set_depth(0, ParamId::Mix, -100));
        assert!(!c.set_depth(0, ParamId::Mode, 50));
        let mut v = ParamValues::default();
        v.set(ParamId::Macro1, 50.0);
        c.apply_macros(&mut v);
        // Pitch spans 48 st: 0.5 × 0.5 × 48 = 12.
        assert!((v.get(ParamId::Pitch) - 12.0).abs() < 1e-9);
        assert!((v.get(ParamId::Mix) - 50.0).abs() < 1e-9);
        v.set(ParamId::Macro1, 100.0);
        v.set(ParamId::Mix, 100.0);
        c.apply_macros(&mut v);
        assert!(v.get(ParamId::Mix).abs() < 1e-9);
        assert!(c.set_depth(0, ParamId::Pitch, 0));
        assert_eq!(c.targets(0).len(), 1);
    }

    #[test]
    fn a_macro_holds_eight_targets() {
        let c = ControlState::default();
        let ids = [
            ParamId::Pitch,
            ParamId::Fine,
            ParamId::Formant,
            ParamId::Mix,
            ParamId::Retune,
            ParamId::Robot,
            ParamId::Glitch,
            ParamId::Spread,
        ];
        for id in ids {
            assert!(c.set_depth(1, id, 10));
        }
        assert!(!c.set_depth(1, ParamId::Sub, 10));
        assert!(c.set_depth(1, ParamId::Pitch, -20));
        assert_eq!(c.depth(1, ParamId::Pitch), -20);
    }

    #[test]
    fn state_text_round_trips() {
        let c = ControlState::default();
        c.bind(1, ParamId::Macro1);
        c.bind(74, ParamId::Formant);
        c.set_depth(2, ParamId::Robot, 80);
        c.set_depth(2, ParamId::Pitch, -25);
        c.set_name(2, "Robot it");
        c.set_program_change(true);
        c.set_zoom(125);
        c.set_loaded(Some(3), "");
        let text = c.to_text();
        let d = ControlState::default();
        d.load_text(&text);
        assert_eq!(d.to_text(), text);
        assert_eq!(d.cc_for(ParamId::Formant), Some(74));
        assert_eq!(d.depth(2, ParamId::Pitch), -25);
        assert_eq!(d.names()[2], "Robot it");
        assert!(d.program_change());
        assert_eq!(d.factory(), Some(3));
    }
}
