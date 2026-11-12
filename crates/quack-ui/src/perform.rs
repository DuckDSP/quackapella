//! Step 8: the preset bar, the macro strip and the right-click menu
//! every control shares (MIDI learn, macro depths). Then the perform
//! row: snapshots A/B with their morph, and the MIDI output.
//!
//! All of it goes through [`ParamAccess`]: parameter edits as host edits,
//! macros and MIDI learn through [`ParamAccess::control`] (absent in a
//! host without it: the menus then show nothing).

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use quack_dsp::ParamId;
use quack_dsp::control::{MACRO_COUNT, can_modulate};
use quack_dsp::midiout::OutSource;
use quack_dsp::params::{MACRO_PARAMS, ParamValues};
use quack_dsp::presets::{self, CATEGORIES, FACTORY, Preset};
use quack_dsp::snapshot::SnapshotKeys;

use crate::library::{self, Entry};
use crate::paint::{panel, pill, rounded, text, value};
use crate::theme::{color, radius, size, space};
use crate::{ParamAccess, UiState, controls, set_once};

/// Zoom steps of the editor, percent.
pub const ZOOMS: [u16; 6] = [75, 100, 125, 150, 175, 200];

/// Preset browser and macro state kept between frames.
#[derive(Debug, Default)]
pub struct PresetUi {
    /// Host values this frame, and with the macros applied.
    pub values: ParamValues,
    pub modded: ParamValues,
    /// The loaded preset's values (for the "modified" mark).
    baseline: Option<ParamValues>,
    /// Recall count the baseline belongs to.
    seen: Option<u32>,
    menu: bool,
    saving: Option<String>,
    users: Option<Vec<Entry>>,
    /// Message under the bar (saved, error) and when it was set.
    note: Option<(String, f64)>,
}

impl PresetUi {
    fn users(&mut self) -> &[Entry] {
        self.users.get_or_insert_with(|| {
            library::user_dir()
                .map(|d| library::list(&d))
                .unwrap_or_default()
        })
    }
}

/// Read the values once per frame, and follow recalls made elsewhere
/// (program change, session load).
pub(crate) fn refresh(params: &dyn ParamAccess, st: &mut UiState) {
    let pu = &mut st.presets;
    for id in ParamId::ALL {
        pu.values.set(id, params.get_plain(id));
    }
    pu.modded = pu.values;
    let Some(ctl) = params.control() else {
        return;
    };
    ctl.apply_macros(&mut pu.modded);
    let count = ctl.recall_count();
    if pu.seen != Some(count) {
        pu.seen = Some(count);
        pu.baseline = match ctl.factory() {
            Some(i) => FACTORY.get(i).map(presets::Factory::values),
            None => {
                let user = ctl.user_preset();
                (!user.is_empty())
                    .then(|| library::user_dir().and_then(|d| library::find(&d, &user)))
                    .flatten()
                    .map(|p| p.values)
            }
        };
    }
}

/// Load a preset: one host edit per changed parameter, the setup
/// parameters left alone.
fn recall(params: &dyn ParamAccess, st: &mut UiState, preset: &Preset, factory: Option<usize>) {
    let pu = &mut st.presets;
    let target = presets::merge(&pu.values, &preset.values);
    for id in ParamId::ALL {
        if target.get(id) != pu.values.get(id) {
            set_once(params, id, target.get(id));
        }
    }
    pu.values = target;
    pu.baseline = Some(preset.values);
    if let Some(ctl) = params.control() {
        preset.recall_macros(ctl);
        ctl.set_loaded(factory, if factory.is_some() { "" } else { &preset.name });
        pu.seen = Some(ctl.recall_count());
    }
}

/// Name shown in the bar.
fn current_name(params: &dyn ParamAccess) -> String {
    let Some(ctl) = params.control() else {
        return "Presets".to_owned();
    };
    if let Some(f) = ctl.factory().and_then(|i| FACTORY.get(i)) {
        return f.name.to_owned();
    }
    let user = ctl.user_preset();
    if user.is_empty() {
        "No preset".to_owned()
    } else {
        user
    }
}

/// Factory then user presets, as one list for previous / next.
fn step(params: &dyn ParamAccess, st: &mut UiState, dir: i32) {
    let Some(ctl) = params.control() else {
        return;
    };
    let users: Vec<Entry> = st.presets.users().to_vec();
    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    let total = (FACTORY.len() + users.len()) as i32;
    let at = match ctl.factory() {
        #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
        Some(i) => i as i32,
        None => {
            let user = ctl.user_preset();
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            users
                .iter()
                .position(|e| e.name == user)
                .map_or(-1, |i| (FACTORY.len() + i) as i32)
        }
    };
    let next = if at < 0 && dir < 0 {
        total - 1
    } else {
        (at + dir).rem_euclid(total)
    };
    #[allow(clippy::cast_sign_loss)]
    let next = next as usize;
    if let Some(f) = FACTORY.get(next) {
        recall(params, st, &f.to_preset(), Some(next));
    } else if let Some(p) = users
        .get(next - FACTORY.len())
        .and_then(|e| library::load(&e.path))
    {
        recall(params, st, &p, None);
    }
}

/// Header preset bar: ‹ name › and Save. The name opens the browser.
pub(crate) fn preset_bar(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, r: Rect) {
    let p = ui.painter().clone();
    let save_w = 64.0;
    let bar = Rect::from_min_max(r.min, pos2(r.right() - save_w - space::S, r.bottom()));
    rounded(&p, bar, radius::CONTROL, color::WELL, color::BORDER);
    let arrow_w = bar.height();
    for (i, (label, dir)) in [("‹", -1), ("›", 1)].into_iter().enumerate() {
        let a = if i == 0 {
            Rect::from_min_size(bar.min, vec2(arrow_w, bar.height()))
        } else {
            Rect::from_min_size(
                pos2(bar.right() - arrow_w, bar.top()),
                vec2(arrow_w, bar.height()),
            )
        };
        let resp = ui.interact(a, ui.id().with(("quack-preset-step", i)), Sense::click());
        if resp.hovered() {
            rounded(
                &p,
                a.shrink(2.0),
                radius::CONTROL - 1,
                color::RAISED,
                color::RAISED,
            );
        }
        if resp.clicked() {
            step(params, st, dir);
        }
        text(
            &p,
            a.center(),
            Align2::CENTER_CENTER,
            label,
            size::TITLE,
            color::TEXT_DIM,
        );
    }
    let mid = Rect::from_min_max(
        pos2(bar.left() + arrow_w, bar.top()),
        pos2(bar.right() - arrow_w, bar.bottom()),
    );
    let resp = ui.interact(mid, ui.id().with("quack-preset-name"), Sense::click());
    if resp.hovered() {
        rounded(
            &p,
            mid.shrink(2.0),
            radius::CONTROL - 1,
            color::RAISED,
            color::RAISED,
        );
    }
    if resp.clicked() {
        st.presets.menu = !st.presets.menu;
        st.presets.users = None;
        st.presets.saving = None;
    }
    let modified = st
        .presets
        .baseline
        .is_some_and(|b| !presets::matches(&st.presets.values, &b));
    let name = current_name(params);
    let t = text(
        &p,
        mid.center() - vec2(6.0, 0.0),
        Align2::CENTER_CENTER,
        &name,
        size::BODY,
        color::TEXT,
    );
    if modified {
        text(
            &p,
            pos2(t.right() + 3.0, t.top() + 2.0),
            Align2::LEFT_TOP,
            "*",
            size::BODY,
            color::QUACK,
        );
    }
    let (x, y) = (t.right() + if modified { 20.0 } else { 14.0 }, t.center().y);
    p.add(egui::Shape::convex_polygon(
        vec![
            pos2(x - 4.0, y - 2.0),
            pos2(x + 4.0, y - 2.0),
            pos2(x, y + 3.0),
        ],
        color::TEXT_DIM,
        egui::Stroke::NONE,
    ));
    let category = params
        .control()
        .and_then(|c| c.factory())
        .and_then(|i| FACTORY.get(i))
        .map_or("User", |f| f.category);
    if name != "No preset" && params.control().is_some() {
        text(
            &p,
            pos2(mid.left() + space::S, mid.center().y),
            Align2::LEFT_CENTER,
            category,
            size::CAPTION - 2.0,
            color::TEXT_FAINT,
        );
    }

    let save = Rect::from_min_size(pos2(r.right() - save_w, r.top()), vec2(save_w, r.height()));
    let sresp = ui.interact(save, ui.id().with("quack-preset-save"), Sense::click());
    pill(
        &p,
        save,
        "Save",
        st.presets.saving.is_some(),
        sresp.hovered(),
        color::QUACK,
    );
    if sresp.clicked() && params.control().is_some() {
        st.presets.menu = false;
        st.presets.saving = if st.presets.saving.is_some() {
            None
        } else {
            let user = params
                .control()
                .map(|c| c.user_preset())
                .unwrap_or_default();
            Some(user)
        };
    }

    if st.presets.menu {
        browser(ui, params, st, r);
    }
    if st.presets.saving.is_some() {
        save_box(ui, params, st, r);
    }
    let now = ui.input(|i| i.time);
    if let Some((msg, at)) = &st.presets.note {
        if now - at < 3.0 {
            text(
                &p,
                pos2(r.center().x, r.bottom() + 9.0),
                Align2::CENTER_CENTER,
                msg,
                size::CAPTION - 2.0,
                color::TEXT_DIM,
            );
        } else {
            st.presets.note = None;
        }
    }
}

/// A browser entry: name, factory index or user file.
type BrowserItem = (String, Option<usize>, Option<std::path::PathBuf>);

/// The preset browser: one column per category, user presets last.
fn browser(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, anchor: Rect) {
    let users: Vec<Entry> = st.presets.users().to_vec();
    let col_w = 150.0;
    let row = 24.0;
    let mut cols: Vec<(&str, Vec<BrowserItem>)> = CATEGORIES
        .iter()
        .map(|c| {
            (
                *c,
                FACTORY
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.category == *c)
                    .map(|(i, f)| (f.name.to_owned(), Some(i), None))
                    .collect(),
            )
        })
        .collect();
    cols.push((
        "User",
        users
            .iter()
            .map(|e| (e.name.clone(), None, Some(e.path.clone())))
            .collect(),
    ));
    let rows = cols.iter().map(|c| c.1.len()).max().unwrap_or(1).max(3);
    #[allow(clippy::cast_precision_loss)]
    let size_v = vec2(
        col_w * cols.len() as f32 + 16.0,
        row * (rows as f32 + 1.0) + 24.0,
    );
    let screen = ui.ctx().content_rect();
    let x = (anchor.center().x - size_v.x / 2.0)
        .min(screen.right() - size_v.x - 8.0)
        .max(screen.left() + 8.0);
    let r = Rect::from_min_size(pos2(x, anchor.bottom() + 6.0), size_v);
    let current = params.control().and_then(|c| c.factory());
    let current_user = params
        .control()
        .map(|c| c.user_preset())
        .unwrap_or_default();
    let mut pick: Option<(Option<usize>, Option<std::path::PathBuf>)> = None;
    let mut close = false;
    egui::Area::new(ui.id().with("quack-preset-browser"))
        .fixed_pos(r.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let (r, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter();
            rounded(p, r, radius::PANEL, color::RAISED, color::BORDER);
            for (ci, (cat, items)) in cols.iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let x0 = r.left() + 8.0 + col_w * ci as f32;
                text(
                    p,
                    pos2(x0 + 10.0, r.top() + 20.0),
                    Align2::LEFT_CENTER,
                    cat,
                    size::CAPTION,
                    color::TEXT_FAINT,
                );
                if items.is_empty() {
                    text(
                        p,
                        pos2(x0 + 10.0, r.top() + 22.0 + row),
                        Align2::LEFT_CENTER,
                        "Save one with Save",
                        size::CAPTION - 2.0,
                        color::TEXT_FAINT.gamma_multiply(0.7),
                    );
                }
                for (i, (name, f, path)) in items.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    let item = Rect::from_min_size(
                        pos2(x0, r.top() + 34.0 + row * i as f32),
                        vec2(col_w - 6.0, row - 2.0),
                    );
                    let resp = ui.interact(
                        item,
                        ui.id().with(("quack-preset-item", ci, i)),
                        Sense::click(),
                    );
                    let lit = (f.is_some() && *f == current)
                        || (f.is_none() && current.is_none() && *name == current_user);
                    if resp.hovered() || lit {
                        let fill = if lit {
                            color::QUACK.gamma_multiply(0.2)
                        } else {
                            color::BORDER
                        };
                        rounded(p, item, radius::CONTROL - 1, fill, fill);
                    }
                    if resp.clicked() {
                        pick = Some((*f, path.clone()));
                    }
                    text(
                        p,
                        pos2(item.left() + 10.0, item.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        size::BODY - 1.0,
                        if lit { color::QUACK } else { color::TEXT },
                    );
                }
            }
            let out = ui.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|q| !r.contains(q) && !anchor.contains(q))
            });
            close = out || ui.input(|i| i.key_pressed(egui::Key::Escape));
        });
    match pick {
        Some((Some(i), _)) => {
            recall(params, st, &FACTORY[i].to_preset(), Some(i));
            st.presets.menu = false;
        }
        Some((None, Some(path))) => {
            if let Some(p) = library::load(&path) {
                recall(params, st, &p, None);
            }
            st.presets.menu = false;
        }
        _ => {
            if close {
                st.presets.menu = false;
            }
        }
    }
}

/// Name field and Save for a user preset.
fn save_box(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, anchor: Rect) {
    let r = Rect::from_min_size(
        pos2(anchor.right() - 320.0, anchor.bottom() + 6.0),
        vec2(320.0, 92.0),
    );
    let mut name = st.presets.saving.clone().unwrap_or_default();
    let mut done = None;
    egui::Area::new(ui.id().with("quack-preset-save-box"))
        .fixed_pos(r.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let (r, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            rounded(ui.painter(), r, radius::PANEL, color::RAISED, color::BORDER);
            text(
                ui.painter(),
                pos2(r.left() + 14.0, r.top() + 18.0),
                Align2::LEFT_CENTER,
                "Save as user preset",
                size::CAPTION,
                color::TEXT_DIM,
            );
            let field = Rect::from_min_size(
                pos2(r.left() + 12.0, r.top() + 34.0),
                vec2(r.width() - 24.0, 24.0),
            );
            let edit = ui.put(
                field,
                egui::TextEdit::singleline(&mut name)
                    .hint_text("Preset name")
                    .char_limit(48),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let ok =
                Rect::from_min_size(pos2(r.right() - 92.0, r.bottom() - 30.0), vec2(80.0, 22.0));
            let cancel = Rect::from_min_size(pos2(ok.left() - 88.0, ok.top()), vec2(80.0, 22.0));
            let okr = ui.interact(ok, ui.id().with("quack-save-ok"), Sense::click());
            let cr = ui.interact(cancel, ui.id().with("quack-save-cancel"), Sense::click());
            pill(
                ui.painter(),
                ok,
                "Save",
                !name.trim().is_empty(),
                okr.hovered(),
                color::QUACK,
            );
            pill(
                ui.painter(),
                cancel,
                "Cancel",
                false,
                cr.hovered(),
                color::QUACK,
            );
            if (okr.clicked() || enter) && !name.trim().is_empty() {
                done = Some(true);
            } else if cr.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
    st.presets.saving = Some(name.clone());
    match done {
        Some(true) => {
            let now = ui.input(|i| i.time);
            if let (Some(ctl), Some(dir)) = (params.control(), library::user_dir()) {
                let preset = Preset::capture(&name, "User", &st.presets.values, ctl);
                match library::save(&dir, &preset) {
                    Ok(_) => {
                        ctl.set_loaded(None, preset.name.as_str());
                        st.presets.seen = Some(ctl.recall_count());
                        st.presets.baseline = Some(preset.values);
                        st.presets.note =
                            Some((format!("Saved \u{201c}{}\u{201d}", preset.name), now));
                    }
                    Err(e) => st.presets.note = Some((format!("Could not save: {e}"), now)),
                }
            }
            st.presets.users = None;
            st.presets.saving = None;
        }
        Some(false) => st.presets.saving = None,
        None => {}
    }
}

/// Short description of a macro's targets.
fn targets_text(params: &dyn ParamAccess, m: usize) -> Vec<String> {
    params.control().map_or_else(Vec::new, |c| {
        c.targets(m)
            .into_iter()
            .map(|t| format!("{} {:+}%", short_name(t.id), t.depth))
            .collect()
    })
}

fn short_name(id: ParamId) -> &'static str {
    match id {
        ParamId::HarmonyHumanize => "Harm. Humanize",
        ParamId::HarmonyLevel => "Voices",
        ParamId::CarrierRelease => "Release",
        ParamId::CarrierNoise => "Noise",
        ParamId::CarrierGlide => "Carrier Glide",
        _ => id.spec().name,
    }
}

/// Bottom strip: the four macros, program change, learn status, zoom.
pub(crate) fn macro_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter().clone();
    panel(&p, r);
    text(
        &p,
        r.left_top() + vec2(space::L, 20.0),
        Align2::LEFT_CENTER,
        "Macros",
        size::LABEL,
        color::TEXT_DIM,
    );
    let names = params.control().map_or_else(
        || quack_dsp::control::DEFAULT_MACRO_NAMES.map(str::to_owned),
        |c| c.names(),
    );
    let cell_w = 176.0;
    let x0 = r.left() + space::L + 70.0;
    let pct = |v: f64| format!("{v:.0}");
    for m in 0..MACRO_COUNT {
        #[allow(clippy::cast_precision_loss)]
        let cx = x0 + cell_w * m as f32;
        let centre = pos2(cx + 28.0, r.center().y - 7.0);
        controls::knob_named(
            ui,
            params,
            st,
            MACRO_PARAMS[m],
            centre,
            50.0,
            0.0,
            accent,
            &pct,
            &names[m],
        );
        let lines = targets_text(params, m);
        let tx = cx + 64.0;
        if lines.is_empty() {
            text(
                &p,
                pos2(tx, r.center().y - 8.0),
                Align2::LEFT_CENTER,
                "Right-click a control",
                size::CAPTION - 2.5,
                color::TEXT_FAINT.gamma_multiply(0.8),
            );
            text(
                &p,
                pos2(tx, r.center().y + 8.0),
                Align2::LEFT_CENTER,
                "to assign it",
                size::CAPTION - 2.5,
                color::TEXT_FAINT.gamma_multiply(0.8),
            );
        }
        let shown = lines.len().min(4);
        for (i, l) in lines.iter().take(shown).enumerate() {
            let l = if i == 3 && lines.len() > 4 {
                format!("+{} more", lines.len() - 3)
            } else {
                l.clone()
            };
            #[allow(clippy::cast_precision_loss)]
            let y = r.center().y - 24.0 + 16.0 * i as f32 + (4.0 - shown as f32) * 8.0;
            text(
                &p,
                pos2(tx, y),
                Align2::LEFT_CENTER,
                &l,
                size::CAPTION - 2.5,
                color::TEXT_DIM,
            );
        }
    }

    // Right: program change and zoom, learn status under them.
    let Some(ctl) = params.control() else {
        return;
    };
    let right = r.right() - space::L;
    let pc = Rect::from_min_size(pos2(right - 150.0, r.top() + 12.0), vec2(150.0, 26.0));
    let resp = ui.interact(pc, ui.id().with("quack-program-change"), Sense::click());
    if resp.clicked() {
        ctl.set_program_change(!ctl.program_change());
    }
    pill(
        &p,
        pc,
        "Program Change",
        ctl.program_change(),
        resp.hovered(),
        color::QUACK,
    );
    resp.on_hover_text("MIDI program changes recall the factory presets, in list order");

    let zr = Rect::from_min_size(pos2(right - 150.0, pc.bottom() + 8.0), vec2(150.0, 26.0));
    rounded(&p, zr, radius::CONTROL, color::WELL, color::BORDER);
    let z = ctl.zoom();
    let at = ZOOMS.iter().position(|&s| s >= z).unwrap_or(1);
    for (i, (label, dir)) in [("−", -1i32), ("+", 1)].into_iter().enumerate() {
        let a = if i == 0 {
            Rect::from_min_size(zr.min, vec2(zr.height(), zr.height()))
        } else {
            Rect::from_min_size(
                pos2(zr.right() - zr.height(), zr.top()),
                vec2(zr.height(), zr.height()),
            )
        };
        let resp = ui.interact(a, ui.id().with(("quack-zoom", i)), Sense::click());
        if resp.hovered() {
            rounded(
                &p,
                a.shrink(2.0),
                radius::CONTROL - 1,
                color::RAISED,
                color::RAISED,
            );
        }
        if resp.clicked() {
            #[allow(
                clippy::cast_possible_wrap,
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation
            )]
            let n = (at as i32 + dir).clamp(0, ZOOMS.len() as i32 - 1) as usize;
            ctl.set_zoom(ZOOMS[n]);
        }
        text(
            &p,
            a.center(),
            Align2::CENTER_CENTER,
            label,
            size::TITLE,
            color::TEXT_DIM,
        );
    }
    value(
        &p,
        zr.center(),
        Align2::CENTER_CENTER,
        &format!("Zoom {z} %"),
        size::VALUE,
        color::TEXT_DIM,
    );
}

/// MIDI learn status, in the footer.
pub(crate) fn learn_status(ui: &egui::Ui, params: &dyn ParamAccess, at: egui::Pos2) {
    let Some(ctl) = params.control() else {
        return;
    };
    let (msg, c) = if let Some(id) = ctl.armed() {
        (
            format!("MIDI learn: move a controller for {}", id.spec().name),
            color::QUACK,
        )
    } else {
        let n = ctl.bindings().len();
        match (n, ctl.last_cc()) {
            (0, _) => (
                "Right-click a control: MIDI learn, macro depth".to_owned(),
                color::TEXT_FAINT,
            ),
            (n, Some(cc)) => (
                format!("{n} MIDI learned \u{b7} last CC {cc}"),
                color::TEXT_FAINT,
            ),
            (n, None) => (format!("{n} MIDI learned"), color::TEXT_FAINT),
        }
    };
    text(
        ui.painter(),
        at,
        Align2::CENTER_CENTER,
        &msg,
        size::CAPTION - 1.5,
        c,
    );
}

/// Right-click menu of a control: MIDI learn and macro depths; and, on
/// a macro knob, its name.
pub(crate) fn attach(ui: &egui::Ui, resp: &egui::Response, params: &dyn ParamAccess, id: ParamId) {
    let Some(ctl) = params.control() else {
        return;
    };
    // Armed: a ring until a CC arrives.
    if ctl.armed() == Some(id) {
        let t = ui.input(|i| i.time);
        #[allow(clippy::cast_possible_truncation)]
        let a = (0.55 + 0.45 * (t * 6.0).sin()) as f32;
        ui.painter().rect_stroke(
            resp.rect.expand(3.0),
            radius::CONTROL,
            egui::Stroke::new(2.0_f32, color::QUACK.gamma_multiply(a)),
            egui::StrokeKind::Outside,
        );
    } else if ctl.cc_for(id).is_some() {
        ui.painter().circle_filled(
            resp.rect.right_top() + vec2(-2.0, 2.0),
            2.5,
            color::TEXT_FAINT,
        );
    }
    resp.context_menu(|ui| {
        ui.set_min_width(230.0);
        ui.label(egui::RichText::new(id.spec().name).strong());
        ui.separator();
        if ctl.armed() == Some(id) {
            if ui.button("Cancel MIDI learn").clicked() {
                ctl.disarm();
                ui.close();
            }
        } else if ui.button("MIDI learn").clicked() {
            ctl.arm(id);
            ui.close();
        }
        if let Some(cc) = ctl.cc_for(id)
            && ui.button(format!("Forget CC {cc}")).clicked()
        {
            ctl.forget(id);
            ui.close();
        }
        if can_modulate(id) {
            ui.separator();
            ui.label(egui::RichText::new("Macro depth").small());
            let names = ctl.names();
            for (m, name) in names.iter().enumerate() {
                let mut d = i32::from(ctl.depth(m, id));
                ui.horizontal(|ui| {
                    ui.add_sized([86.0, 18.0], egui::Label::new(name.as_str()).truncate());
                    let r = ui.add(egui::Slider::new(&mut d, -100..=100).suffix(" %"));
                    if r.changed() {
                        #[allow(clippy::cast_possible_truncation)]
                        let ok = ctl.set_depth(m, id, d as i8);
                        if !ok {
                            ui.label("full");
                        }
                    }
                });
            }
        }
        if let Some(m) = MACRO_PARAMS.iter().position(|&p| p == id) {
            ui.separator();
            let key = ui.id().with(("quack-macro-name", m, ctl.recall_count()));
            let mut name = ui
                .data(|d| d.get_temp::<String>(key))
                .unwrap_or_else(|| ctl.names()[m].clone());
            ui.horizontal(|ui| {
                ui.label("Name");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
                if r.changed() {
                    ctl.set_name(m, &name);
                }
            });
            ui.data_mut(|d| d.insert_temp(key, name));
            if ui.button("Clear assignments").clicked() {
                ctl.set_targets(m, &[]);
                ui.close();
            }
        }
    });
}

/// Labels of the snapshot keys, in `SnapshotKeys` order.
fn keys_labels() -> Vec<&'static str> {
    SnapshotKeys::ALL.iter().map(|k| k.name()).collect()
}

/// Labels of the MIDI out sources, in `OutSource` order.
fn source_labels() -> Vec<&'static str> {
    OutSource::ALL.iter().map(|s| s.name()).collect()
}

const CHANNELS: [&str; 16] = [
    "Ch 1", "Ch 2", "Ch 3", "Ch 4", "Ch 5", "Ch 6", "Ch 7", "Ch 8", "Ch 9", "Ch 10", "Ch 11",
    "Ch 12", "Ch 13", "Ch 14", "Ch 15", "Ch 16",
];

/// A small text button; returns whether it was clicked.
fn button(
    ui: &egui::Ui,
    r: Rect,
    key: &str,
    label: &str,
    lit: bool,
    enabled: bool,
    accent: Color32,
) -> egui::Response {
    let resp = ui.interact(r, ui.id().with(("quack-btn", key)), Sense::click());
    let p = ui.painter();
    if enabled {
        pill(p, r, label, lit, resp.hovered(), accent);
    } else {
        rounded(p, r, radius::CONTROL, color::PANEL, color::BORDER);
        text(
            p,
            r.center(),
            Align2::CENTER_CENTER,
            label,
            size::BODY,
            color::TEXT_FAINT.gamma_multiply(0.7),
        );
    }
    resp
}

/// Snapshots A and B and the morph between them.
pub(crate) fn snapshot_panel(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, r: Rect) {
    let accent = color::QUACK;
    let p = ui.painter().clone();
    panel(&p, r);
    let head_y = r.top() + 20.0;
    text(
        &p,
        pos2(r.left() + space::L, head_y),
        Align2::LEFT_CENTER,
        "Snapshots",
        size::LABEL,
        color::TEXT_DIM,
    );
    let ctl = params.control();
    let has = |s: usize| ctl.is_some_and(|c| c.has_snapshot(s));
    let morph_on = params.get_plain(ParamId::MorphOn) >= 0.5;
    let morph = params.get_plain(ParamId::Morph);

    // Header, right: store the current sound.
    let right = r.right() - space::L;
    let bh = 22.0;
    for (i, label) in [(1usize, "B"), (0, "A")] {
        #[allow(clippy::cast_precision_loss)]
        let x = right - 34.0 - (1 - i) as f32 * 40.0;
        let br = Rect::from_min_size(pos2(x, head_y - bh / 2.0), vec2(34.0, bh));
        let resp = button(
            ui,
            br,
            &format!("save-{i}"),
            label,
            false,
            ctl.is_some(),
            accent,
        );
        if resp.clicked()
            && let Some(c) = ctl
        {
            c.store_snapshot(i, &st.presets.values);
        }
        resp.on_hover_text(format!(
            "Save the current settings as snapshot {label}{}",
            if has(i) { " (replaces it)" } else { "" }
        ));
    }
    text(
        &p,
        pos2(right - 82.0, head_y),
        Align2::RIGHT_CENTER,
        "Save to",
        size::CAPTION,
        color::TEXT_FAINT,
    );
    let hint = match (has(0), has(1), morph_on) {
        (false, false, _) => "Save two sounds, then morph between them",
        (true, false, _) | (false, true, _) => "Save the other snapshot to morph",
        (true, true, false) => "Morph off: A and B recall at once",
        (true, true, true) => "",
    };
    if !hint.is_empty() {
        text(
            &p,
            pos2(r.left() + 110.0, head_y),
            Align2::LEFT_CENTER,
            hint,
            size::CAPTION,
            color::TEXT_FAINT,
        );
    }

    // Controls row.
    let row = Rect::from_min_size(
        pos2(r.left() + space::L, r.bottom() - space::M - 28.0),
        vec2(r.width() - 2.0 * space::L, 28.0),
    );
    let mut x = row.left();
    let mut next = |w: f32| {
        let c = Rect::from_min_size(pos2(x, row.top()), vec2(w, row.height()));
        x += w + space::S;
        c
    };
    controls::toggle(ui, params, ParamId::MorphOn, next(78.0), "Morph", accent);
    let br = next(34.0);
    let lit = morph_on && morph <= 0.0;
    if button(ui, br, "go-a", "A", lit, has(0), accent).clicked()
        && let Some(c) = ctl
    {
        c.request_snapshot(0);
    }
    let fr = next(196.0);
    controls::fader(ui, params, st, ParamId::Morph, fr, accent, "A ↔ B");
    let br = next(34.0);
    let lit = morph_on && morph >= 100.0;
    if button(ui, br, "go-b", "B", lit, has(1), accent).clicked()
        && let Some(c) = ctl
    {
        c.request_snapshot(1);
    }
    x += space::S;
    let tr = Rect::from_min_size(pos2(x, row.top()), vec2(128.0, row.height()));
    controls::fader(ui, params, st, ParamId::MorphTime, tr, accent, "Time");
    let kr = Rect::from_min_max(pos2(tr.right() + space::S, row.top()), row.right_bottom());
    let keys = SnapshotKeys::from_index(params.get_plain(ParamId::SnapshotKeys));
    let name = if keys == SnapshotKeys::Off {
        "Keys off".to_owned()
    } else {
        format!("Keys {}", keys.name())
    };
    if controls::stepper(ui, params, ParamId::SnapshotKeys, kr, &name, accent) {
        st.keys_menu = !st.keys_menu;
    }
    st.keys_rect = kr;
}

/// The MIDI output: on/off, channel, what it plays.
pub(crate) fn midi_out_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &crate::View,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter().clone();
    panel(&p, r);
    let head_y = r.top() + 20.0;
    let t = text(
        &p,
        pos2(r.left() + space::L, head_y),
        Align2::LEFT_CENTER,
        "MIDI Out",
        size::LABEL,
        color::TEXT_DIM,
    );
    let on = params.get_plain(ParamId::MidiOut) >= 0.5;
    let notes = view.midi_out_notes;
    if on && notes != 0 {
        let low = notes.trailing_zeros();
        #[allow(clippy::cast_possible_truncation)]
        let mut s = crate::note_name(low as u8);
        if notes.count_ones() > 1 {
            s = format!("{s} (+{})", notes.count_ones() - 1);
        }
        value(
            &p,
            pos2(t.right() + space::M, head_y),
            Align2::LEFT_CENTER,
            &s,
            size::VALUE,
            accent,
        );
    }
    let tr = Rect::from_min_size(
        pos2(r.right() - space::L - 56.0, head_y - 11.0),
        vec2(56.0, 22.0),
    );
    controls::toggle(ui, params, ParamId::MidiOut, tr, "On", accent);

    let row_y = r.bottom() - space::M - 28.0;
    let cr = Rect::from_min_size(pos2(r.left() + space::L, row_y), vec2(84.0, 28.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ch = (params.get_plain(ParamId::MidiOutChannel).max(0.0) as usize).min(15);
    if controls::stepper(
        ui,
        params,
        ParamId::MidiOutChannel,
        cr,
        CHANNELS[ch],
        accent,
    ) {
        st.channel_menu = !st.channel_menu;
    }
    let sr = Rect::from_min_max(
        pos2(cr.right() + space::S, row_y),
        pos2(r.right() - space::L, row_y + 28.0),
    );
    let src = OutSource::from_index(params.get_plain(ParamId::MidiOutSource));
    if controls::stepper(ui, params, ParamId::MidiOutSource, sr, src.name(), accent) {
        st.source_menu = !st.source_menu;
    }
    st.channel_rect = cr;
    st.source_rect = sr;
}

/// Pop-up lists of the perform row, over everything else.
pub(crate) fn perform_menus(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    accent: Color32,
) {
    if st.keys_menu {
        st.keys_menu = controls::choice_menu(
            ui,
            params,
            ParamId::SnapshotKeys,
            st.keys_rect,
            &keys_labels(),
            color::QUACK,
        );
    }
    if st.channel_menu {
        let r = Rect::from_min_size(st.channel_rect.min, vec2(st.channel_rect.width(), 0.0));
        let r = r.translate(vec2(0.0, -26.0 * 16.0 - 12.0));
        st.channel_menu =
            controls::choice_menu(ui, params, ParamId::MidiOutChannel, r, &CHANNELS, accent);
    }
    if st.source_menu {
        st.source_menu = controls::choice_menu(
            ui,
            params,
            ParamId::MidiOutSource,
            st.source_rect,
            &source_labels(),
            accent,
        );
    }
}
