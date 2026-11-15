//! Step 8: the preset bar, the macro strip and the right-click menu
//! every control shares (MIDI learn, macro depths). Then the perform
//! row: snapshots A/B with their morph, and the MIDI output.
//!
//! All of it goes through [`ParamAccess`]: parameter edits as host edits,
//! macros and MIDI learn through [`ParamAccess::control`] (absent in a
//! host without it: the menus then show nothing).

use std::collections::BTreeSet;

use crate::help::Hint;
use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use quack_dsp::ParamId;
use quack_dsp::control::{MACRO_COUNT, can_modulate};
use quack_dsp::latency::VoiceRange;
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
    /// Browser: shelf shown (the loaded preset's at first), search
    /// text, favorites (read when first needed).
    shelf: Option<Shelf>,
    search: String,
    favs: Option<BTreeSet<String>>,
    /// User preset whose delete button was clicked once (a second
    /// click deletes it).
    deleting: Option<String>,
}

impl PresetUi {
    /// Open the browser (on `shelf`, or the loaded preset's).
    pub fn open_browser(&mut self, shelf: Option<Shelf>) {
        self.menu = true;
        self.shelf = shelf;
    }

    fn favs(&mut self) -> &BTreeSet<String> {
        self.favs.get_or_insert_with(|| {
            library::user_dir()
                .map(|d| library::favorites(&d))
                .unwrap_or_default()
        })
    }

    /// Star or unstar a preset, and save the list.
    fn toggle_favorite(&mut self, key: &str, now: f64) {
        let favs = self.favs.get_or_insert_with(BTreeSet::new);
        if !favs.remove(key) {
            favs.insert(key.to_owned());
        }
        if let Some(dir) = library::user_dir()
            && let Err(e) = library::save_favorites(&dir, favs)
        {
            self.note = Some((format!("Could not save favorites: {e}"), now));
        }
    }

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
    let mut target = presets::merge(&pu.values, &preset.values);
    // A preset made to morph starts on A, ready to move.
    if preset.has_snapshots() {
        target.set(ParamId::Morph, 0.0);
        target.set(ParamId::MorphOn, 1.0);
    }
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
    crate::paint::fill(&p, bar, radius::CONTROL, color::PANEL);
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
            crate::paint::fill(&p, a.shrink(2.0), radius::CONTROL - 1, color::HOVER);
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
        crate::paint::fill(&p, mid.shrink(2.0), radius::CONTROL - 1, color::HOVER);
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

/// A shelf of the browser's side list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shelf {
    All,
    Favorites,
    User,
    /// A factory category ([`presets::STYLES`] or [`CATEGORIES`]).
    Category(&'static str),
}

impl Shelf {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Favorites => "Favorites",
            Self::User => "User",
            Self::Category(c) => c,
        }
    }
}

/// One preset in the browser.
#[derive(Debug, Clone)]
struct Item {
    name: String,
    category: String,
    factory: Option<usize>,
    path: Option<std::path::PathBuf>,
    note: String,
    /// Favorite key ([`library::factory_key`], [`library::user_key`]).
    key: String,
    /// Holds snapshots A and B.
    morph: bool,
}

impl Item {
    fn on(&self, shelf: Shelf, favs: &BTreeSet<String>) -> bool {
        match shelf {
            Shelf::All => true,
            Shelf::Favorites => favs.contains(&self.key),
            Shelf::User => self.factory.is_none(),
            Shelf::Category(c) => self.factory.is_some() && self.category == c,
        }
    }

    fn matches(&self, search: &str) -> bool {
        let s = search.trim().to_lowercase();
        s.is_empty()
            || [&self.name, &self.category, &self.note]
                .iter()
                .any(|t| t.to_lowercase().contains(&s))
    }
}

/// Every preset: the factory library in order, then the user's.
fn items(users: &[Entry]) -> Vec<Item> {
    let mut out: Vec<Item> = FACTORY
        .iter()
        .enumerate()
        .map(|(i, f)| Item {
            name: f.name.to_owned(),
            category: f.category.to_owned(),
            factory: Some(i),
            path: None,
            note: f.note.to_owned(),
            key: library::factory_key(f.name),
            morph: f.snapshots().is_some(),
        })
        .collect();
    out.extend(users.iter().map(|e| Item {
        name: e.name.clone(),
        category: "User".to_owned(),
        factory: None,
        path: Some(e.path.clone()),
        note: String::new(),
        key: library::user_key(&e.name),
        morph: false,
    }));
    out
}

/// The side list: the library shelves, then the styles, then the
/// modules.
fn shelves() -> Vec<(Option<&'static str>, Shelf)> {
    let mut v = vec![
        (Some("LIBRARY"), Shelf::All),
        (None, Shelf::Favorites),
        (None, Shelf::User),
    ];
    for (i, c) in presets::STYLES.iter().enumerate() {
        v.push(((i == 0).then_some("STYLES"), Shelf::Category(c)));
    }
    for (i, c) in CATEGORIES.iter().enumerate() {
        v.push(((i == 0).then_some("MODULES"), Shelf::Category(c)));
    }
    v
}

/// A five-pointed star, filled or outlined.
fn star(p: &egui::Painter, c: egui::Pos2, r: f32, on: bool, col: Color32) {
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|k| {
            #[allow(clippy::cast_precision_loss)]
            let a = std::f32::consts::PI * (k as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
            let rr = if k % 2 == 0 { r } else { r * 0.45 };
            c + vec2(a.cos(), a.sin()) * rr
        })
        .collect();
    if on {
        // Convex pieces: the inner pentagon and the five tips.
        let inner: Vec<egui::Pos2> = pts.iter().skip(1).step_by(2).copied().collect();
        p.add(egui::Shape::convex_polygon(inner, col, egui::Stroke::NONE));
        for k in (0..10).step_by(2) {
            let tip = vec![pts[(k + 9) % 10], pts[k], pts[k + 1]];
            p.add(egui::Shape::convex_polygon(tip, col, egui::Stroke::NONE));
        }
    } else {
        p.add(egui::Shape::closed_line(
            pts,
            egui::Stroke::new(1.0_f32, col),
        ));
    }
}

/// The preset browser: shelves on the left (library, styles, modules),
/// the presets of the shelf on the right with a search field and a
/// favorite star each, the hovered preset's description below. A
/// click loads the preset and leaves the browser open to try the next;
/// Escape or a click outside closes it. A user preset has a delete
/// button (x), which asks for a second click.
fn browser(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, anchor: Rect) {
    let users: Vec<Entry> = st.presets.users().to_vec();
    let all = items(&users);
    st.presets.favs();
    let current = params.control().and_then(|c| c.factory());
    let current_user = params
        .control()
        .map(|c| c.user_preset())
        .unwrap_or_default();
    let is_current = |it: &Item| {
        (it.factory.is_some() && it.factory == current)
            || (it.factory.is_none() && current.is_none() && it.name == current_user)
    };
    let shelf = *st.presets.shelf.get_or_insert_with(|| {
        all.iter()
            .find(|it| is_current(it))
            .map_or(Shelf::All, |it| match it.factory {
                Some(i) => Shelf::Category(FACTORY[i].category),
                None => Shelf::User,
            })
    });

    let size_v = vec2(660.0, 470.0);
    let screen = ui.ctx().content_rect();
    let x = (anchor.center().x - size_v.x / 2.0)
        .min(screen.right() - size_v.x - 8.0)
        .max(screen.left() + 8.0);
    let r = Rect::from_min_size(pos2(x, anchor.bottom() + 6.0), size_v);
    let mut pick: Option<Item> = None;
    let mut toggle: Option<String> = None;
    let mut delete: Option<Item> = None;
    let mut arm_delete: Option<Option<String>> = None;
    let deleting = st.presets.deleting.clone();
    let mut new_shelf = None;
    let mut hovered: Option<Item> = None;
    let mut close = false;
    let mut search = st.presets.search.clone();
    let favs = st.presets.favs.clone().unwrap_or_default();
    egui::Area::new(ui.id().with("quack-preset-browser"))
        .fixed_pos(r.min)
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let (r, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            let p = ui.painter().clone();
            rounded(&p, r, radius::PANEL, color::RAISED, color::BORDER);

            // Shelves.
            let side_w = 168.0;
            let mut y = r.top() + 10.0;
            for (si, (caption, s)) in shelves().into_iter().enumerate() {
                if let Some(cap) = caption {
                    if si > 0 {
                        y += 6.0;
                    }
                    text(
                        &p,
                        pos2(r.left() + 18.0, y + 8.0),
                        Align2::LEFT_CENTER,
                        cap,
                        size::CAPTION - 3.0,
                        color::TEXT_FAINT,
                    );
                    y += 18.0;
                }
                let row = Rect::from_min_size(pos2(r.left() + 8.0, y), vec2(side_w - 12.0, 22.0));
                let resp = ui.interact(row, ui.id().with(("quack-shelf", si)), Sense::click());
                let lit = s == shelf;
                if lit || resp.hovered() {
                    let fill = if lit {
                        color::QUACK.gamma_multiply(0.2)
                    } else {
                        color::BORDER
                    };
                    rounded(&p, row, radius::CONTROL - 1, fill, fill);
                }
                if resp.clicked() {
                    new_shelf = Some(s);
                }
                text(
                    &p,
                    pos2(row.left() + 10.0, row.center().y),
                    Align2::LEFT_CENTER,
                    s.label(),
                    size::BODY - 1.0,
                    if lit { color::QUACK } else { color::TEXT },
                );
                let n = all.iter().filter(|it| it.on(s, &favs)).count();
                text(
                    &p,
                    pos2(row.right() - 8.0, row.center().y),
                    Align2::RIGHT_CENTER,
                    &n.to_string(),
                    size::CAPTION - 3.0,
                    color::TEXT_FAINT,
                );
                y += 22.0;
            }
            let div = r.left() + side_w;
            p.line_segment(
                [pos2(div, r.top() + 10.0), pos2(div, r.bottom() - 10.0)],
                egui::Stroke::new(1.0_f32, color::BORDER),
            );

            // Search.
            let right = Rect::from_min_max(
                pos2(div + 10.0, r.top() + 10.0),
                pos2(r.right() - 10.0, r.bottom() - 10.0),
            );
            let field = Rect::from_min_size(right.min, vec2(right.width(), 24.0));
            ui.put(
                field,
                egui::TextEdit::singleline(&mut search)
                    .hint_text(format!("Search {}", shelf.label().to_lowercase()))
                    .char_limit(40),
            );

            // Presets of the shelf.
            let note_h = 46.0;
            let list = Rect::from_min_max(
                pos2(right.left(), field.bottom() + 8.0),
                pos2(right.right(), right.bottom() - note_h - 6.0),
            );
            let shown: Vec<&Item> = all
                .iter()
                .filter(|it| it.on(shelf, &favs) && it.matches(&search))
                .collect();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
            if shown.is_empty() {
                let msg = match shelf {
                    _ if !search.trim().is_empty() => "Nothing matches",
                    Shelf::Favorites => "Click a star to add a favorite",
                    Shelf::User => "Save one with Save",
                    _ => "Empty",
                };
                text(
                    &p,
                    pos2(list.left() + 10.0, list.top() + 14.0),
                    Align2::LEFT_CENTER,
                    msg,
                    size::CAPTION - 2.0,
                    color::TEXT_FAINT,
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("quack-preset-list")
                .auto_shrink([false, false])
                .show(&mut child, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let w = ui.available_width();
                    for it in &shown {
                        let (row, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
                        let star_r = Rect::from_center_size(
                            pos2(row.left() + 14.0, row.center().y),
                            vec2(22.0, 22.0),
                        );
                        let sresp = ui.interact(
                            star_r,
                            ui.id().with(("quack-fav", &it.key)),
                            Sense::click(),
                        );
                        let p = ui.painter();
                        let lit = is_current(it);
                        if resp.hovered() || sresp.hovered() || lit {
                            let fill = if lit {
                                color::QUACK.gamma_multiply(0.2)
                            } else {
                                color::BORDER
                            };
                            rounded(p, row, radius::CONTROL - 1, fill, fill);
                        }
                        if resp.hovered() || sresp.hovered() {
                            hovered = Some((*it).clone());
                        }
                        let fav = favs.contains(&it.key);
                        star(
                            p,
                            star_r.center(),
                            6.5,
                            fav,
                            if fav {
                                color::QUACK
                            } else if sresp.hovered() {
                                color::TEXT_DIM
                            } else {
                                color::TEXT_FAINT.gamma_multiply(0.6)
                            },
                        );
                        // User presets: a delete button, clicked twice.
                        let mut del_clicked = false;
                        if it.factory.is_none() {
                            let armed = deleting.as_deref() == Some(it.key.as_str());
                            let del_r = if armed {
                                Rect::from_min_size(
                                    pos2(row.right() - 112.0, row.top() + 2.0),
                                    vec2(108.0, row.height() - 4.0),
                                )
                            } else {
                                Rect::from_center_size(
                                    pos2(row.right() - 14.0, row.center().y),
                                    vec2(22.0, 22.0),
                                )
                            };
                            let dresp = ui.interact(
                                del_r,
                                ui.id().with(("quack-del", &it.key)),
                                Sense::click(),
                            );
                            let p = ui.painter();
                            if armed {
                                rounded(
                                    p,
                                    del_r,
                                    radius::CONTROL - 1,
                                    color::METER_OVER.gamma_multiply(0.25),
                                    color::METER_OVER,
                                );
                                text(
                                    p,
                                    del_r.center(),
                                    Align2::CENTER_CENTER,
                                    "Click to delete",
                                    size::CAPTION - 3.0,
                                    color::METER_OVER,
                                );
                            } else if resp.hovered() || dresp.hovered() {
                                let c = if dresp.hovered() {
                                    color::METER_OVER
                                } else {
                                    color::TEXT_FAINT
                                };
                                let m = del_r.center();
                                for (a, b) in [(-4.0, 4.0), (4.0, -4.0)] {
                                    p.line_segment(
                                        [m + vec2(-4.0, a), m + vec2(4.0, b)],
                                        egui::Stroke::new(1.5_f32, c),
                                    );
                                }
                            }
                            if dresp.hovered() {
                                hovered = Some((*it).clone());
                            }
                            if dresp.clicked() {
                                del_clicked = true;
                                if armed {
                                    delete = Some((*it).clone());
                                } else {
                                    arm_delete = Some(Some(it.key.clone()));
                                }
                            }
                        }
                        if sresp.clicked() {
                            toggle = Some(it.key.clone());
                        } else if resp.clicked() && !del_clicked {
                            pick = Some((*it).clone());
                        }
                        let t = text(
                            p,
                            pos2(row.left() + 30.0, row.center().y),
                            Align2::LEFT_CENTER,
                            &it.name,
                            size::BODY - 1.0,
                            if lit { color::QUACK } else { color::TEXT },
                        );
                        if it.morph {
                            let tag = Rect::from_min_size(
                                pos2(t.right() + 8.0, row.center().y - 7.0),
                                vec2(26.0, 14.0),
                            );
                            rounded(p, tag, 3, Color32::TRANSPARENT, color::BORDER);
                            text(
                                p,
                                tag.center(),
                                Align2::CENTER_CENTER,
                                "A/B",
                                size::CAPTION - 4.0,
                                color::TEXT_DIM,
                            );
                        }
                        // Program change number (factory) and, off its
                        // own shelf, the category.
                        let mut rx = row.right() - if it.factory.is_none() { 32.0 } else { 8.0 };
                        if let Some(i) = it.factory {
                            let pc = text(
                                p,
                                pos2(rx, row.center().y),
                                Align2::RIGHT_CENTER,
                                &format!("PC {i}"),
                                size::CAPTION - 3.0,
                                color::TEXT_FAINT.gamma_multiply(0.8),
                            );
                            rx = pc.left() - 10.0;
                        }
                        if !matches!(shelf, Shelf::Category(_) | Shelf::User) {
                            text(
                                p,
                                pos2(rx, row.center().y),
                                Align2::RIGHT_CENTER,
                                &it.category,
                                size::CAPTION - 2.0,
                                color::TEXT_FAINT,
                            );
                        }
                    }
                });

            // What the hovered (or loaded) preset is for.
            let info = hovered
                .clone()
                .or_else(|| all.iter().find(|it| is_current(it)).cloned());
            let foot = Rect::from_min_max(pos2(right.left(), right.bottom() - note_h), right.max);
            p.line_segment(
                [foot.left_top(), foot.right_top()],
                egui::Stroke::new(1.0_f32, color::BORDER),
            );
            if let Some(it) = info {
                let mut s = if it.note.is_empty() {
                    it.category.clone()
                } else {
                    it.note.clone()
                };
                if it.morph {
                    s.push_str("  Macro 1 or A/B: morph to B.");
                }
                let g = p.layout(
                    s,
                    crate::theme::label_font(size::CAPTION - 1.5),
                    color::TEXT_DIM,
                    foot.width() - 8.0,
                );
                p.galley(
                    pos2(foot.left() + 4.0, foot.top() + 7.0),
                    g,
                    color::TEXT_DIM,
                );
            }

            let out = ui.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|q| !r.contains(q) && !anchor.contains(q))
            });
            close = out || ui.input(|i| i.key_pressed(egui::Key::Escape));
        });
    st.presets.search = search;
    // A click anywhere else disarms the delete button.
    if let Some(a) = arm_delete {
        st.presets.deleting = a;
    } else if pick.is_some() || toggle.is_some() || new_shelf.is_some() {
        st.presets.deleting = None;
    }
    if let Some(it) = delete {
        st.presets.deleting = None;
        let now = ui.input(|i| i.time);
        if let Some(path) = &it.path {
            match library::delete(path) {
                Ok(()) => {
                    st.presets.note = Some((format!("Deleted \u{201c}{}\u{201d}", it.name), now));
                    if st.presets.favs().contains(&it.key) {
                        st.presets.toggle_favorite(&it.key, now);
                    }
                    // The sound stays; it is just no longer a preset.
                    if let Some(ctl) = params.control()
                        && ctl.factory().is_none()
                        && ctl.user_preset() == it.name
                    {
                        ctl.set_loaded(None, "");
                        st.presets.seen = Some(ctl.recall_count());
                        st.presets.baseline = None;
                    }
                }
                Err(e) => st.presets.note = Some((format!("Could not delete: {e}"), now)),
            }
        }
        st.presets.users = None;
    }
    if let Some(s) = new_shelf {
        st.presets.shelf = Some(s);
    }
    if let Some(key) = toggle {
        st.presets.toggle_favorite(&key, ui.input(|i| i.time));
    }
    if let Some(it) = pick {
        match (it.factory, it.path) {
            (Some(i), _) => recall(params, st, &FACTORY[i].to_preset(), Some(i)),
            (None, Some(path)) => {
                if let Some(p) = library::load(&path) {
                    recall(params, st, &p, None);
                }
            }
            _ => {}
        }
    } else if close {
        st.presets.menu = false;
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

/// Bottom panel: the four macros, then snapshots A and B with their
/// morph. What each macro moves shows on hover.
pub(crate) fn perform_panel(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, r: Rect) {
    let accent = color::QUACK;
    let p = ui.painter().clone();
    panel(&p, r);
    let title_y = r.top() + 20.0;
    crate::paint::section(
        &p,
        pos2(r.left() + space::L, title_y),
        "Macros",
        color::TEXT_FAINT,
    );
    let names = params.control().map_or_else(
        || quack_dsp::control::DEFAULT_MACRO_NAMES.map(str::to_owned),
        |c| c.names(),
    );
    let pct = |v: f64| format!("{v:.0}");
    let knob_y = r.top() + 20.0 + (r.height() - 20.0) / 2.0 - 6.0;
    let cell_w = 96.0;
    for m in 0..MACRO_COUNT {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + 40.0 + cell_w * m as f32;
        let resp = controls::knob_named(
            ui,
            params,
            st,
            MACRO_PARAMS[m],
            pos2(cx, knob_y),
            50.0,
            0.0,
            accent,
            &pct,
            &names[m],
        );
        let lines = targets_text(params, m);
        if lines.is_empty() {
            resp.hint("Right-click any control to assign it to this macro");
        } else {
            resp.hint(format!("{}\n{}", names[m], lines.join("\n")));
        }
    }

    // Snapshots.
    let x0 = r.left() + 436.0;
    p.line_segment(
        [
            pos2(x0 - space::L, r.top() + space::M),
            pos2(x0 - space::L, r.bottom() - space::M),
        ],
        egui::Stroke::new(1.0_f32, color::GRID_STRONG),
    );
    let t = crate::paint::section(&p, pos2(x0, title_y), "Snapshots", color::TEXT_FAINT);
    let morph_on = params.get_plain(ParamId::MorphOn) >= 0.5;
    let mr = Rect::from_min_size(pos2(t.right() + space::L, title_y - 11.0), vec2(96.0, 22.0));
    controls::switch(ui, params, ParamId::MorphOn, mr, "Morph", accent)
        .hint("On: A and B blend with the morph knob. Off: they recall at once");
    let ctl = params.control();
    let has = |s: usize| ctl.is_some_and(|c| c.has_snapshot(s));
    let morph = params.get_plain(ParamId::Morph);
    let bw = 52.0;
    let top = knob_y - 26.0;
    let values = st.presets.values;
    let snap = |ui: &egui::Ui, x: f32, i: usize, label: &str, lit: bool| {
        let br = Rect::from_min_size(pos2(x, top), vec2(bw, 30.0));
        let resp = button(ui, br, &format!("go-{i}"), label, lit, has(i), accent);
        if resp.clicked()
            && let Some(c) = ctl
        {
            c.request_snapshot(i);
        }
        resp.hint(format!("Recall snapshot {label}"));
        let sr = Rect::from_min_size(pos2(x, br.bottom() + 6.0), vec2(bw, 22.0));
        let sresp = ui.interact(sr, ui.id().with(("quack-btn", "save", i)), Sense::click());
        let p = ui.painter();
        let bg = if sresp.hovered() {
            color::HOVER
        } else {
            color::CONTROL
        };
        crate::paint::fill(p, sr, radius::CONTROL, bg);
        text(
            p,
            sr.center(),
            Align2::CENTER_CENTER,
            &format!("Save {label}"),
            size::CAPTION - 3.0,
            if ctl.is_some() {
                color::TEXT_DIM
            } else {
                color::TEXT_FAINT
            },
        );
        if sresp.clicked()
            && let Some(c) = ctl
        {
            c.store_snapshot(i, &values);
        }
        sresp.hint(format!(
            "Save the current sound as snapshot {label}{}",
            if has(i) { " (replaces it)" } else { "" }
        ));
    };
    snap(ui, x0, 0, "A", morph_on && morph <= 0.0);
    let morph_x = x0 + bw + space::M + 26.0;
    controls::knob_named(
        ui,
        params,
        st,
        ParamId::Morph,
        pos2(morph_x, knob_y),
        50.0,
        0.0,
        if morph_on {
            accent
        } else {
            accent.gamma_multiply(0.35)
        },
        &pct,
        "A ↔ B",
    );
    let b_x = morph_x + 26.0 + space::M;
    snap(ui, b_x, 1, "B", morph_on && morph >= 100.0);
    let ms = |v: f64| format!("{v:.0}");
    let time_x = b_x + bw + space::L + 26.0;
    controls::knob_named(
        ui,
        params,
        st,
        ParamId::MorphTime,
        pos2(time_x, knob_y),
        50.0,
        ParamId::MorphTime.spec().kind.min(),
        color::NEUTRAL,
        &ms,
        "Time",
    );
    // MIDI keys that recall A and B.
    let kx = time_x + 26.0 + space::XL;
    text(
        &p,
        pos2(kx, top + 4.0),
        Align2::LEFT_CENTER,
        "Recall from MIDI keys",
        size::CAPTION - 2.5,
        color::TEXT_FAINT,
    );
    let kr = Rect::from_min_size(
        pos2(kx, top + 16.0),
        vec2((r.right() - space::L - kx).min(170.0), 28.0),
    );
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

/// MIDI learn status, in the status bar (`at`: its left centre).
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
        Align2::LEFT_CENTER,
        &msg,
        size::CAPTION - 2.5,
        c,
    );
}

/// The settings button (a gear) in the header: opens the settings.
pub(crate) fn settings_button(ui: &egui::Ui, st: &mut UiState, r: Rect) {
    let resp = ui.interact(r, ui.id().with("quack-settings"), Sense::click());
    if resp.clicked() {
        st.settings = !st.settings;
    }
    let p = ui.painter();
    let bg = if st.settings || resp.hovered() {
        color::HOVER
    } else {
        color::RAISED
    };
    crate::paint::fill(p, r, radius::CONTROL, bg);
    crate::paint::gear_icon(p, r.center(), 7.5, color::TEXT_DIM);
    resp.hint("Settings: zoom, program change, MIDI out");
}

/// The settings, under the gear: zoom, program change, MIDI out, MIDI
/// learn. A click outside closes them.
pub(crate) fn settings_menu(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &crate::View,
    anchor: Rect,
) {
    if !st.settings {
        return;
    }
    let w = 320.0;
    let row = 44.0;
    let h = 36.0 + row + 68.0 + 84.0 + 58.0;
    let r = Rect::from_min_size(
        pos2(anchor.right() - w + 120.0, anchor.bottom() + 8.0),
        vec2(w, h),
    );
    let accent = color::QUACK;
    let mut close = false;
    egui::Area::new(ui.id().with("quack-settings-menu"))
        .fixed_pos(r.min)
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let (r, _) = ui.allocate_exact_size(r.size(), Sense::click());
            let p = ui.painter().clone();
            rounded(&p, r, radius::PANEL, color::RAISED, color::BORDER);
            let x = r.left() + space::L;
            let right = r.right() - space::L;
            crate::paint::section(&p, pos2(x, r.top() + 20.0), "Settings", color::TEXT_FAINT);
            let mut y = r.top() + 36.0;
            let divider = |p: &egui::Painter, y: f32| {
                p.line_segment(
                    [pos2(r.left() + 1.0, y), pos2(r.right() - 1.0, y)],
                    egui::Stroke::new(1.0_f32, color::BORDER.gamma_multiply(0.6)),
                );
            };

            // Zoom.
            divider(&p, y);
            let cy = y + row / 2.0;
            text(
                &p,
                pos2(x, cy),
                Align2::LEFT_CENTER,
                "Zoom",
                size::BODY,
                color::TEXT,
            );
            if let Some(ctl) = params.control() {
                let z = ctl.zoom();
                let at = ZOOMS.iter().position(|&s| s >= z).unwrap_or(1);
                let plus = Rect::from_min_size(pos2(right - 28.0, cy - 13.0), vec2(28.0, 26.0));
                let minus = plus.translate(vec2(-28.0 - 64.0, 0.0));
                for (i, (b, label, dir)) in [(minus, "−", -1i32), (plus, "+", 1)]
                    .into_iter()
                    .enumerate()
                {
                    let resp = ui.interact(b, ui.id().with(("quack-zoom", i)), Sense::click());
                    let bg = if resp.hovered() {
                        color::HOVER
                    } else {
                        color::CONTROL
                    };
                    crate::paint::fill(&p, b, radius::CONTROL, bg);
                    text(
                        &p,
                        b.center(),
                        Align2::CENTER_CENTER,
                        label,
                        size::BODY,
                        color::TEXT_DIM,
                    );
                    if resp.clicked() {
                        #[allow(
                            clippy::cast_possible_wrap,
                            clippy::cast_sign_loss,
                            clippy::cast_possible_truncation
                        )]
                        let n = (at as i32 + dir).clamp(0, ZOOMS.len() as i32 - 1) as usize;
                        ctl.set_zoom(ZOOMS[n]);
                    }
                }
                value(
                    &p,
                    pos2((minus.right() + plus.left()) / 2.0, cy),
                    Align2::CENTER_CENTER,
                    &format!("{z} %"),
                    size::VALUE,
                    color::TEXT,
                );
            }
            y += row;

            // Program change.
            divider(&p, y);
            let cy = y + 18.0;
            text(
                &p,
                pos2(x, cy),
                Align2::LEFT_CENTER,
                "Program Change",
                size::BODY,
                color::TEXT,
            );
            if let Some(ctl) = params.control() {
                let sr = Rect::from_min_size(pos2(right - 28.0, cy - 9.0), vec2(28.0, 18.0));
                let resp = ui.interact(sr, ui.id().with("quack-program-change"), Sense::click());
                if resp.clicked() {
                    ctl.set_program_change(!ctl.program_change());
                }
                controls::switch_look(&p, sr, "", ctl.program_change(), resp.hovered(), accent);
            }
            for (i, line) in [
                "A pedal or controller sends program n: factory",
                "preset n loads. To change sounds between songs.",
            ]
            .into_iter()
            .enumerate()
            {
                #[allow(clippy::cast_precision_loss)]
                text(
                    &p,
                    pos2(x, cy + 20.0 + 15.0 * i as f32),
                    Align2::LEFT_CENTER,
                    line,
                    size::CAPTION - 3.0,
                    color::TEXT_FAINT,
                );
            }
            y += 68.0;

            // MIDI out: switch, then channel and what it plays.
            divider(&p, y);
            let cy = y + 20.0;
            let t = text(
                &p,
                pos2(x, cy),
                Align2::LEFT_CENTER,
                "MIDI Out",
                size::BODY,
                color::TEXT,
            );
            let on = params.get_plain(ParamId::MidiOut) >= 0.5;
            let notes = view.midi_out_notes;
            if on && notes != 0 {
                #[allow(clippy::cast_possible_truncation)]
                let n = crate::note_name(notes.trailing_zeros() as u8);
                value(
                    &p,
                    pos2(t.right() + space::M, cy),
                    Align2::LEFT_CENTER,
                    &n,
                    size::VALUE - 2.0,
                    accent,
                );
            }
            let sw = Rect::from_min_size(pos2(right - 28.0, cy - 11.0), vec2(28.0, 22.0));
            controls::switch(ui, params, ParamId::MidiOut, sw, "", accent);
            let half = (right - x - space::S) / 2.0;
            let cr = Rect::from_min_size(pos2(x, cy + 18.0), vec2(half, 28.0));
            let sr = Rect::from_min_size(pos2(cr.right() + space::S, cr.top()), vec2(half, 28.0));
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
            let src = OutSource::from_index(params.get_plain(ParamId::MidiOutSource));
            if controls::stepper(ui, params, ParamId::MidiOutSource, sr, src.name(), accent) {
                st.source_menu = !st.source_menu;
            }
            st.channel_rect = cr;
            st.source_rect = sr;
            y += 84.0;

            // MIDI learn.
            divider(&p, y);
            let cy = y + 20.0;
            text(
                &p,
                pos2(x, cy),
                Align2::LEFT_CENTER,
                "MIDI Learn",
                size::BODY,
                color::TEXT,
            );
            if let Some(ctl) = params.control() {
                let n = ctl.bindings().len();
                let msg = match n {
                    0 => "Right-click a control to learn".to_owned(),
                    1 => "1 control learned".to_owned(),
                    n => format!("{n} controls learned"),
                };
                text(
                    &p,
                    pos2(x, cy + 18.0),
                    Align2::LEFT_CENTER,
                    &msg,
                    size::CAPTION - 3.0,
                    color::TEXT_FAINT,
                );
                if n > 0 {
                    let fr = Rect::from_min_size(pos2(right - 86.0, cy - 4.0), vec2(86.0, 26.0));
                    let resp = ui.interact(fr, ui.id().with("quack-forget-all"), Sense::click());
                    let bg = if resp.hovered() {
                        color::HOVER
                    } else {
                        color::CONTROL
                    };
                    crate::paint::fill(&p, fr, radius::CONTROL, bg);
                    text(
                        &p,
                        fr.center(),
                        Align2::CENTER_CENTER,
                        "Forget all",
                        size::CAPTION - 1.0,
                        color::TEXT_DIM,
                    );
                    if resp.clicked() {
                        ctl.forget_all();
                    }
                }
            }

            let lists = st.channel_menu || st.source_menu;
            let clicked_out = ui.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|q| !r.contains(q) && !anchor.contains(q))
            });
            if clicked_out && !lists {
                close = true;
            }
        });
    if close {
        st.settings = false;
    }
}

/// Right-click menu of a control: MIDI learn and macro depths; and, on
/// a macro knob, its name.
pub(crate) fn attach(ui: &egui::Ui, resp: &egui::Response, params: &dyn ParamAccess, id: ParamId) {
    crate::help::param(resp, id);
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
        crate::paint::fill(p, r, radius::CONTROL, color::CONTROL);
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
        // Above the stepper: the list is taller than the room below.
        st.channel_menu = controls::choice_menu_at(
            ui,
            params,
            ParamId::MidiOutChannel,
            st.channel_rect,
            &CHANNELS,
            accent,
            true,
        );
    }
    if st.voice_menu {
        let labels: Vec<&str> = VoiceRange::ALL.iter().map(|v| v.name()).collect();
        st.voice_menu = controls::choice_menu(
            ui,
            params,
            ParamId::Voice,
            st.voice_rect,
            &labels,
            color::QUACK,
        );
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
