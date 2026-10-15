//! The keymaps' screens, over the settings tab (see `core`'s `keymap_ui`): the picker (a scrolled list:
//! Default, then the keymaps, each with who uses it and an Edit button; New), a keymap's menu, deleting
//! one in use, the edit screen (one layer of the keymap, Main or Back held: the pad's controls drawn in
//! two clusters at the sides, as they sit beside the screen, each the layer changes with a label; between
//! the label columns a panel for the selected input, with its parameters; "No changes" below), what one
//! input maps to ("map to": the targets that fit it, as rows of buttons, the chosen one lit), what holds
//! the gyro on, a key chord typed on the keyboard, and waiting for a pad button.
//!
//! On black like the rest: the pad's parts are thin dim outlines; blue is what a keymap maps to something
//! else, grey what it only tunes.

use std::fmt::Write;

use clamshell_core::keymap::{Binding, Button, Change, Chord, Dir, Input, Keymap, Kind, Layer, MouseButton, Output, PadOut, Param, Side, as_on_the_pad, change, gyro_holds, params};
use clamshell_core::keymap_ui::{EditView, Screen};
use clamshell_core::slider::Control;
use clamshell_core::{Id, KeymapId, ListId, RowPart, Scope, Target};

use super::format::Fmt;
use super::scroll::Scrolled;
use super::pad::{Pad, Part};
use super::style::{self, Look, PRESSED, SURFACE, SURFACE_HI, Text, Tone};
use super::{Frame, Track, editor};
use crate::draw::{Align, Keep, LINE, Rect, Rgb, Span};
use crate::icons::Icon;

/// The header's height, a list row's and a button's, of the area's height.
const HEADER_H: f32 = 0.1;
const ROW_H: f32 = 0.1;
const BUTTON_H: f32 = 0.085;
/// A menu's buttons' width, of the area's width.
const MENU_W: f32 = 0.45;
/// A row's Edit button and who uses it, of the row's width.
const EDIT_W: f32 = 0.14;
const USERS_W: f32 = 0.3;
/// A chip's text, of its height; the section labels' column, of the area's width.
const CHIP_TEXT: f32 = 0.42;
const LABELS_W: f32 = 0.16;

pub(super) fn draw(f: &mut Frame, body: Rect, screen: Screen) {
    match screen {
        Screen::List => list(f, body),
        Screen::Menu(id) => menu(f, body, id),
        Screen::Delete(id) => delete(f, body, id),
        Screen::Edit(id) => edit(f, body, id),
        Screen::MapTo { id, input } => map_to(f, body, id, input),
        Screen::Holds { id, input } => holds(f, body, id, input),
        Screen::PressPad { id, input } => press_pad(f, body, id, input),
        Screen::Capture { input, keys, .. } => capture(f, body, input, keys),
    }
}

/// The header: Back, the title, and on the right a note or a button; returns the area below it.
fn header(f: &mut Frame, body: Rect, title: &str, right: Option<(Id, &str)>, note: &str) -> Rect {
    let pad = style::pad(body.h);
    let mut area = style::content(body);
    let mut bar = area.take_top(body.h * HEADER_H);
    let back = bar.take_left(bar.h);
    f.button(Id::Keymap(KeymapId::Back), back, Look::ICON, |f| {
        f.c.icon(Icon::ArrowLeft, back.square_center(back.h * 0.45), Tone::Normal);
    });
    let size = Text::Heading.size(body.h);
    if let Some((id, label)) = right {
        let w = f.c.text_width(size, label) + pad * 2.0;
        let r = bar.take_right(w.min(bar.w * 0.4));
        let r = r.centered(r.w, r.h * 0.8);
        f.button(id, r, Look { fill: Some(SURFACE_HI), ..Look::CARD }, |f| {
            f.c.text_in(r, size * 0.85, Tone::High, label, Align::Center);
        });
    } else if !note.is_empty() {
        let small = Text::Small.size(body.h);
        let w = f.c.text_width(small, note);
        if w < bar.w * 0.6 {
            let r = bar.take_right(w);
            f.c.text_in(r, small, Tone::Dim, note, Align::End);
        }
    }
    f.c.text_fit(Rect::new(bar.x + pad * 0.4, bar.y, bar.w - pad, bar.h), size, Tone::High, title, Keep::Start);
    area.y += pad * 0.4;
    area.h -= pad * 0.4;
    area
}

/// Who uses a keymap, in words.
fn users(f: &Frame, id: u16) -> Fmt<48> {
    let u = f.s.keymap_users(id);
    match (u.all_games, u.games) {
        (true, 0) => Fmt::of(format_args!("All games")),
        (true, n) => Fmt::of(format_args!("All games, {n} {}", if n == 1 { "game" } else { "games" })),
        (false, 0) => Fmt::of(format_args!("Not used")),
        (false, n) => Fmt::of(format_args!("{n} {}", if n == 1 { "game" } else { "games" })),
    }
}

/// The picker.
fn list(f: &mut Frame, body: Rect) {
    let s = f.s;
    let scope = match (s.bottom().scope(), s.editing()) {
        (Scope::Game, Some(game)) => game.title.as_str(),
        _ => "all games",
    };
    let title = Fmt::<128>::of(format_args!("Keymap for {scope}"));
    let area = header(f, body, &title, Some((Id::Keymap(KeymapId::New), "New")), "");
    let pad = style::pad(body.h);
    let size = Text::Body.size(body.h);
    let hint = Text::Small.size(body.h);
    let view = Rect::new(area.x - pad * 0.5, area.y, area.w + pad, area.h - hint * LINE * 1.5);
    let chosen = s.keymap_chosen();
    let rows = Scrolled::begin(f, ListId::Keymaps, view, body.h * ROW_H, s.list_len(ListId::Keymaps));
    for i in rows.shown() {
        let Some(row) = s.keymap_row(i) else {
            continue;
        };
        let id = row.map_or(0, |k| k.id);
        let mut r = rows.row(i);
        let edit = row.map(|_| r.take_right(r.w * EDIT_W));
        // (A keymap that is gone counts as Default.)
        let mine = id == chosen || (id == 0 && s.keymaps().get(chosen).is_none());
        let name = row.map_or("Default", |k| k.name.as_str());
        let who = if row.is_some() { users(f, id) } else { Fmt::of(format_args!("As on the pad")) };
        f.button(Id::Row { list: ListId::Keymaps, i: i as u16, part: RowPart::Main }, r, Look::ROW, |f| {
            let dot = r.h * 0.06;
            if mine {
                f.c.circle(r.x + pad * 0.5 + dot, r.y + r.h / 2.0, dot, Tone::Normal);
            }
            let x = r.x + pad * 0.5 + dot * 4.0;
            let users_w = r.w * USERS_W;
            f.c.text_fit(Rect::new(x, r.y, r.x + r.w - users_w - x - pad * 0.5, r.h), size, Tone::high_if(mine), name, Keep::Start);
            let who_r = Rect::new(r.x + r.w - users_w, r.y, users_w - pad * 0.5, r.h);
            f.c.text_fit(who_r, size * 0.8, Tone::Dim, &who, Keep::Start);
        });
        if let Some(e) = edit {
            f.button(Id::Row { list: ListId::Keymaps, i: i as u16, part: RowPart::Edit }, e, Look::ROW, |f| {
                f.c.text_in(e, size * 0.85, Tone::Normal, "Edit", Align::Center);
            });
        }
    }
    rows.end(f, area.x + area.w + pad * 0.6);
    f.c.text_fit(Rect::new(area.x, area.y + area.h - hint * LINE, area.w, hint * LINE), hint, Tone::Dim, "Long press a keymap to rename, duplicate or delete it", Keep::Start);
}

/// A column of wide buttons, from the top of `area`.
fn buttons(f: &mut Frame, area: Rect, body: Rect, items: &[(Id, &str, Tone)]) {
    let pad = style::pad(body.h);
    let h = body.h * BUTTON_H;
    let size = Text::Body.size(body.h);
    for (i, (id, label, tone)) in items.iter().enumerate() {
        let r = Rect::new(area.x, area.y + i as f32 * (h + pad * 0.5), (area.w * MENU_W).max(size * 6.0).min(area.w), h);
        if r.bottom() > area.bottom() {
            break;
        }
        f.button(id.clone(), r, Look { fill: Some(SURFACE), ..Look::CARD }, |f| {
            f.c.text_in(r, size, *tone, label, Align::Center);
        });
    }
}

fn menu(f: &mut Frame, body: Rect, id: u16) {
    let name = f.s.keymap_name(id);
    let area = header(f, body, name, None, "");
    let k = |id| Id::Keymap(id);
    let items =
        [(k(KeymapId::Edit), "Edit", Tone::High), (k(KeymapId::Rename), "Rename", Tone::Normal), (k(KeymapId::Duplicate), "Duplicate", Tone::Normal), (k(KeymapId::Delete), "Delete", Tone::Accent)];
    buttons(f, area, body, &items);
}

fn delete(f: &mut Frame, body: Rect, id: u16) {
    let name = f.s.keymap_name(id);
    let title = Fmt::<128>::of(format_args!("Delete {name}?"));
    let area = header(f, body, &title, None, "");
    let who = users(f, id);
    let size = Text::Body.size(body.h);
    let text = Fmt::<160>::of(format_args!("In use ({}): deleting it puts them back on Default.", who.as_str()));
    let used = f.c.wrap(Span { x: area.x, y: area.y, w: area.w }, size, Tone::Normal, &text, 3);
    let pad = style::pad(body.h);
    let rest = Rect::new(area.x, area.y + used + pad, area.w, area.h - used - pad);
    buttons(f, rest, body, &[(Id::Keymap(KeymapId::Back), "Keep it", Tone::Normal), (Id::Keymap(KeymapId::ConfirmDelete), "Delete", Tone::Accent)]);
}

/// An input's binding in keymap `k`'s `layer` itself, if it has one there.
fn binding(k: Option<&Keymap>, layer: Layer, input: Input) -> Option<Binding> {
    k.and_then(|k| k.get_in(layer, input)).copied()
}

/// The edit screen: the labels' text, of the small text size; of the area's width, a cluster's width,
/// the room between it and its labels (where the lines turn), and the labels' column's width; the
/// bottom row's height and the panel's rows', of the area's height.
const LABEL: f32 = 0.66;
const CLUSTER_W: f32 = 0.225;
const GAP: f32 = 0.04;
const LABEL_COLUMN_W: f32 = 0.125;
const FOOT_ROW_H: f32 = 0.065;
const PANEL_ROW_H: f32 = 0.055;
/// The panel's and the bottom row's text, of the small text size.
const PANEL_TEXT: f32 = 0.8;
/// The controls' outlines, and the lines in to the labels (blue, dimmed).
const PART_LINE: Rgb = Rgb(0x4a4a54);
/// The buttons drawn that can't be remapped (Home, the AYN button and its neighbours).
const FIXED: Rgb = Rgb(0x2a2a30);
const LEADER_ALPHA: f32 = 0.45;
/// Line widths in logical pixels: the controls', the lines in, a selected control's.
const PART_W: f32 = 1.5;
const LEADER_W: f32 = 1.0;
const SELECTED_W: f32 = 2.5;
/// The most controls in a cluster.
const CONTROLS: usize = 16;

/// What a binding maps to, in words; the game pad's buttons and triggers as the Xbox pad's that games
/// see.
fn mapping(b: Binding) -> Fmt<48> {
    match b.output {
        Output::Pad(PadOut::Button(_) | PadOut::Trigger(_)) | Output::Trigger(_) => Fmt::of(format_args!("Xbox {}", OutputLabel(b.output))),
        out => Fmt::of(format_args!("{}", OutputLabel(out))),
    }
}

/// What an input's binding in a layer says, and its tone: blue where it maps the input to something
/// else, grey where it is the input as on the pad (tuned, or said so in the Back layer).
fn described(input: Input, b: &Binding) -> (Fmt<48>, Tone) {
    match change(input, Some(b)) {
        Change::Mapped if input == Input::Gyro => (Fmt::of(format_args!("Gyro · {}", mapping(*b).as_str())), Tone::Fn),
        Change::Mapped => (mapping(*b), Tone::Fn),
        _ if input == Input::Gyro => (Fmt::of(format_args!("Gyro · off")), Tone::Normal),
        Change::Tuned => (Fmt::of(format_args!("Tuned")), Tone::Normal),
        Change::None => (Fmt::of(format_args!("As on pad")), Tone::Normal),
    }
}

/// A changed control's label beside its cluster: the control, its text and tone, and the label's top
/// once placed.
#[derive(Clone, Copy)]
struct Callout {
    part: Part,
    text: Fmt<48>,
    tone: Tone,
    top: f32,
}

impl Callout {
    /// The height its line reaches the labels' side at.
    fn exit(&self) -> f32 {
        self.part.lane.unwrap_or(self.part.anchor.1)
    }
}

/// One cluster's labels, in the order their lines arrive in (top to bottom).
struct Column {
    items: [Option<Callout>; CONTROLS],
    len: usize,
}

impl Column {
    fn new() -> Column {
        Column { items: [None; CONTROLS], len: 0 }
    }

    fn iter(&self) -> impl Iterator<Item = &Callout> {
        self.items.iter().take(self.len).flatten()
    }

    fn push(&mut self, c: Callout) {
        let Some(slot) = self.items.get_mut(self.len) else {
            return;
        };
        *slot = Some(c);
        let y = |c: &Option<Callout>| c.map_or(f32::MAX, |c| c.exit());
        let mut i = self.len;
        while i > 0 && self.items.get(i - 1).map(y) > self.items.get(i).map(y) {
            self.items.swap(i - 1, i);
            i -= 1;
        }
        self.len += 1;
    }

    /// Places the labels (`h` high) level with their lines where they can be, within `y0`..`y1`, at
    /// least `h + gap` apart (less if they don't fit).
    fn place(&mut self, y0: f32, y1: f32, h: f32, gap: f32) {
        let step = (h + gap).min((y1 - y0 - h) / self.len.saturating_sub(1).max(1) as f32).max(0.0);
        let mut next = y0;
        for c in self.items.iter_mut().take(self.len).flatten() {
            c.top = (c.exit() - h / 2.0).min(y1 - h).max(next);
            next = c.top + step;
        }
        let mut last = y1 - h + step;
        for c in self.items.iter_mut().take(self.len).rev().flatten() {
            c.top = c.top.min(last - step);
            last = c.top;
        }
    }
}

/// Where the edit screen's parts go in `body`: the pad's area (below the header), the bottom row, a
/// cluster's width, the room between it and its labels, the labels' column's width, and the strip
/// between the label columns (the panel's and the gyro's).
pub(super) struct EditLayout {
    pub area: Rect,
    pub foot_row: Rect,
    pub cw: f32,
    pub gap: f32,
    pub lw: f32,
    pub strip: Rect,
}

pub(super) fn edit_layout(body: Rect) -> EditLayout {
    let pad = style::pad(body.h);
    let mut area = style::content(body);
    area.take_top(body.h * HEADER_H);
    area.y += pad * 0.4;
    area.h -= pad * 0.4;
    let foot_row = area.take_bottom(body.h * FOOT_ROW_H);
    let cw = (area.w * CLUSTER_W).floor();
    let gap = (area.w * GAP).floor();
    let lw = (area.w * LABEL_COLUMN_W).floor();
    let strip = Rect::new(area.x + cw + gap + lw, area.y, area.w - 2.0 * (cw + gap + lw), area.h);
    EditLayout { area, foot_row, cw, gap, lw, strip }
}

/// The edit screen's header: Back, the keymap's name, and on the right the layer shown (Main or Back
/// held, a segmented switch).
fn edit_header(f: &mut Frame, body: Rect, title: &str, layer: Layer) {
    let pad = style::pad(body.h);
    let mut area = style::content(body);
    let mut bar = area.take_top(body.h * HEADER_H);
    let back = bar.take_left(bar.h);
    f.button(Id::Keymap(KeymapId::Back), back, Look::ICON, |f| {
        f.c.icon(Icon::ArrowLeft, back.square_center(back.h * 0.45), Tone::Normal);
    });
    let size = Text::Small.size(body.h) * 0.8;
    let seg_h = bar.h * 0.62;
    // Right to left: Back held, Main.
    let mut right = bar.right();
    for l in [Layer::Back, Layer::Main] {
        let w = (f.c.text_width(size, l.label()) + pad * 1.4).min(bar.w * 0.22);
        let r = Rect::new(right - w, bar.y + (bar.h - seg_h) / 2.0, w, seg_h);
        right -= w + pad * 0.2;
        let on = l == layer;
        let look = Look { fill: Some(if on { PRESSED } else { SURFACE }), ..Look::CARD };
        f.button(Id::Keymap(KeymapId::Layer(l)), r, look, |f| {
            f.c.text_fit(r.inset(pad * 0.3), size, Tone::high_if(on), l.label(), Keep::Start);
        });
    }
    let title_w = (right - pad * 0.5 - bar.x - pad * 0.4).max(0.0);
    f.c.text_fit(Rect::new(bar.x + pad * 0.4, bar.y, title_w, bar.h), Text::Heading.size(body.h), Tone::High, title, Keep::Start);
}

/// The edit screen: the controls in two clusters at the sides (see [`super::pad`]), each a target; one
/// the layer shown changes has a line in to a label (blue: mapped to something else; grey: as on the
/// pad, tuned). Home and the AYN button are drawn dim, not remappable (their codes are a device test, see
/// docs/thor-gameplan.md); Back is the layer key, a target that switches the layer shown, lit while it is
/// the Back layer. Between the label columns, the panel: the selected input, what it does in the layer,
/// Change and Reset, and its parameters (a value is held or slid for the slider, a switch tapped). The
/// gyro's button sits at the bottom, over the AYN button. Below, a hint and "No changes".
fn edit(f: &mut Frame, body: Rect, id: u16) {
    let s = f.s;
    let keymap = s.keymaps().get(id);
    let view = s.keymap_edit();
    let layer = view.layer;
    let name = keymap.map_or("", |k| k.name.as_str());
    edit_header(f, body, name, layer);
    let EditLayout { area, foot_row, cw, gap, lw, strip } = edit_layout(body);
    let size = Text::Small.size(body.h) * LABEL;
    let line = size * LINE;
    let pad = Pad::new(area, cw);

    let (mut left, mut right) = (Column::new(), Column::new());
    for input in Input::all() {
        let (Some(part), Some(b)) = (pad.part(input), binding(keymap, layer, input)) else {
            continue;
        };
        if layer == Layer::Main && change(input, Some(&b)) == Change::None {
            continue;
        }
        let (text, tone) = described(input, &b);
        let c = Callout { part, text, tone, top: 0.0 };
        if part.side == Side::Left { left.push(c) } else { right.push(c) }
    }
    for column in [&mut left, &mut right] {
        column.place(area.y, area.bottom(), line, size * 0.4);
    }

    // Lines in, from each changed control to its label.
    for (side, column) in [(Side::Left, &left), (Side::Right, &right)] {
        let cluster = pad.cluster(side);
        let (bus, end) = match side {
            Side::Left => (cluster.right() + gap * 0.3, cluster.right() + gap - size * 0.3),
            Side::Right => (cluster.x - gap * 0.3, cluster.x - gap + size * 0.3),
        };
        for c in column.iter() {
            let color = c.tone.rgb().alpha(LEADER_ALPHA);
            let (ax, ay) = c.part.anchor;
            let (across, y) = (c.exit(), c.top + line / 2.0);
            if c.part.lane.is_some() {
                f.c.line((ax, ay), (ax, across), LEADER_W, color);
            }
            if (y - across).abs() < 0.5 {
                f.c.line((ax, across), (end, y), LEADER_W, color);
            } else {
                f.c.line((ax, across), (bus, across), LEADER_W, color);
                f.c.line((bus, across), (end, y), LEADER_W, color);
            }
        }
    }

    // The controls (the clicks last: they sit on the sticks' caps).
    for side in Side::ALL {
        if let Some(well) = pad.well(side) {
            f.c.outline(well, well.w, PART_W, PART_LINE);
        }
    }
    let click = |i: &Input| matches!(i, Input::Button(Button::LS | Button::RS));
    for input in Input::all().filter(|i| !click(i)).chain(Input::all().filter(click)) {
        if let Some(part) = pad.part(input) {
            let tone = binding(keymap, layer, input).map_or(Tone::Dim, |b| described(input, &b).1);
            control(f, input, part, tone, view.selected == Some(input), size);
        }
    }

    // The labels.
    for (side, column) in [(Side::Left, &left), (Side::Right, &right)] {
        let cluster = pad.cluster(side);
        for c in column.iter() {
            let w = f.c.text_width(size, &c.text).min(lw);
            let x = if side == Side::Left { cluster.right() + gap } else { cluster.x - gap - w };
            f.c.text_fit(Rect::new(x, c.top, w + 1.0, line), size, c.tone, &c.text, Keep::Start);
        }
    }

    // Home (ClamShell's, drawn dim) and Back (the layer key), at the screen's bottom corners.
    if let Some(r) = pad.foot(Side::Left) {
        f.c.outline(r, r.w, PART_W, FIXED);
        let w = f.c.text_width(size, "Home");
        f.c.text_in(Rect::new(r.x - r.w * 0.6 - w, r.y, w, r.h), size, FIXED, "Home", Align::Start);
    }
    if let Some(r) = pad.foot(Side::Right) {
        let held = layer == Layer::Back;
        let name = if held { "Back held" } else { "Back" };
        let w = f.c.text_width(size, name);
        let row = pad.foot_row(Side::Right).unwrap_or(r);
        let target = Rect::new(r.x - r.w, r.y - r.w, r.w * 2.6 + w, r.h + 2.0 * r.w).intersect(row);
        let look = Look { radius: r.w / 2.0, fill: None, inset: 0.0, ring: -3.0 };
        f.button_in(Id::Keymap(KeymapId::Layer(layer.other())), target, r, look, |f| {
            if held {
                f.c.circle(r.center().0, r.center().1, r.w / 2.0, PRESSED);
            }
            f.c.outline(r, r.w, PART_W, if held { Tone::High.rgb() } else { PART_LINE });
            f.c.text_in(Rect::new(r.right() + r.w * 0.6, r.y, w, r.h), size, Tone::high_if(held), name, Align::Start);
        });
    }

    // The middle: the gyro and the AYN button at the bottom, the panel above.
    let foot = pad.foot(Side::Left).map_or(area.bottom() - line, |r| r.center().1);
    let gyro_top = middle(f, strip, foot, keymap, layer, view.selected == Some(Input::Gyro), size);
    let margin = (strip.w * 0.04).min(size);
    let panel_r = Rect::new(strip.x + margin, area.y, (strip.w - 2.0 * margin).max(1.0), (gyro_top - area.y - size).max(1.0));
    let sliding_row = panel(f, body, panel_r, id, keymap, view);
    foot_actions(f, body, foot_row, keymap, view);

    // A parameter's slider over everything, on its row's height.
    if let Some(slider) = view.slider {
        let cy = sliding_row.unwrap_or(panel_r.center().1);
        super::bar::draw_slider(f, slider, body.w, cy, body.y.max(20.0));
    }
}

/// The panel: the selected input's name; what it does in the layer shown; Change and Reset; its
/// parameters, a row each. Returns the middle of the row whose slider is up, if one is.
fn panel(f: &mut Frame, body: Rect, r: Rect, id: u16, keymap: Option<&Keymap>, view: EditView) -> Option<f32> {
    let s = f.s;
    let pad = style::pad(body.h);
    let small = Text::Small.size(body.h) * PANEL_TEXT;
    let mut col = Column1::new(r);
    let Some(input) = view.selected else {
        let y = r.y + r.h * 0.4;
        for (i, t) in ["Press or touch an input", "to see what it does"].into_iter().enumerate() {
            centred(f, Rect::new(r.x, y + i as f32 * small * LINE, r.w, small * LINE), small, Tone::Dim, t);
        }
        return None;
    };
    let layer = view.layer;
    let own = binding(keymap, layer, input);
    // The name.
    let name = Fmt::<32>::of(format_args!("{}", Label(input)));
    let heading = Text::Heading.size(body.h) * 0.9;
    if let Some(row) = col.take(heading * LINE) {
        centred(f, row, heading, Tone::High, &name);
    }
    // What it does.
    let (what, tone) = match (own, layer) {
        (Some(b), _) => described(input, &b),
        (None, Layer::Back) => {
            let main = keymap.and_then(|k| k.get(input)).map(|b| described(input, b).0).unwrap_or_else(|| as_on_pad(input));
            (Fmt::of(format_args!("As in Main: {}", main.as_str())), Tone::Dim)
        }
        (None, Layer::Main) => (as_on_pad(input), Tone::Dim),
    };
    if let Some(row) = col.take(small * LINE * 1.4) {
        centred(f, row, small, tone, &what);
    }
    // Change, Reset.
    let bh = body.h * BUTTON_H * 0.65;
    col.skip(pad * 0.3);
    if let Some(row) = col.take(bh) {
        let w = (row.w - pad * 0.4) / 2.0;
        let reset = if layer == Layer::Back { "Use Main" } else { "Reset" };
        for (i, (kid, label, on)) in [(KeymapId::Change, "Change", true), (KeymapId::Reset, reset, own.is_some())].into_iter().enumerate() {
            let b = Rect::new(row.x + i as f32 * (w + pad * 0.4), row.y, w, row.h);
            f.button(Id::Keymap(kid), b, Look { fill: Some(SURFACE), ..Look::CARD }, |f| {
                centred(f, b.inset(pad * 0.15), small, if on { Tone::Normal } else { Tone::Dim }, label);
            });
        }
    }
    col.skip(pad * 0.4);
    // The parameters (from the main layer's in the Back layer without its own, else as on the pad).
    let base = s.param_base(id, input)?;
    let mut sliding_row = None;
    let rh = body.h * PANEL_ROW_H;
    for p in params(input, base.output) {
        let Some(row) = col.take(rh) else {
            return sliding_row;
        };
        let v = p.get(&base);
        let value = match p {
            _ if p.switch() => Fmt::<16>::of(format_args!("{}", if v > 0 { "On" } else { "Off" })),
            Param::Speed | Param::Curve => Fmt::of(format_args!("{v}")),
            _ => Fmt::of(format_args!("{v} %")),
        };
        let changed = v != p.default_value();
        if p.switch() {
            f.button(Id::Keymap(KeymapId::Toggle(p)), row, Look::ROW, |f| param_row(f, row, small, p.label(), &value, changed));
        } else {
            let sliding = view.slider.is_some_and(|sl| sl.control == Control::Param(p));
            if sliding {
                sliding_row = Some(row.center().1);
            }
            f.button(Id::Control(Control::Param(p)), row, Look::ROW, |f| param_row(f, row, small, p.label(), &value, changed));
            f.track(Track { x0: body.x, w: body.w });
        }
    }
    if input == Input::Gyro && matches!(base.output, Output::Pointer | Output::Stick(_)) {
        let Some(row) = col.take(rh) else {
            return sliding_row;
        };
        let value = match base.hold {
            Some(h) => Fmt::<24>::of(format_args!("while {}", Label(h))),
            None => Fmt::of(format_args!("Always")),
        };
        f.button(Id::Keymap(KeymapId::Holds), row, Look::ROW, |f| param_row(f, row, small, "Active", &value, base.hold.is_some()));
    }
    if let Some(row) = col.take(small * LINE * 1.5) {
        let hint = if params(input, base.output).next().is_some() { "Hold a value to slide it" } else { "Press it again to change it" };
        centred(f, row.shifted(0.0, small * 0.3), small * 0.85, Tone::Dim, hint);
    }
    sliding_row
}

/// "As on the pad" in words, for an input (the gyro: off).
fn as_on_pad(input: Input) -> Fmt<48> {
    Fmt::of(format_args!("{}", if input == Input::Gyro { "Off" } else { "As on the pad" }))
}

/// A parameter's row: its name on the left, its value on the right (brighter when not the default).
fn param_row(f: &mut Frame, row: Rect, size: f32, label: &str, value: &str, changed: bool) {
    let inner = row.inset(size * 0.4);
    let vw = f.c.text_width(size, value).min(inner.w * 0.5);
    f.c.text_fit(Rect::new(inner.x, row.y, inner.w - vw - size * 0.3, row.h), size, Tone::Dim, label, Keep::Start);
    f.c.text_fit(Rect::new(inner.right() - vw, row.y, vw + 1.0, row.h), size, Tone::high_if(changed), value, Keep::Start);
}

/// Text centred in a row (cut to fit).
fn centred(f: &mut Frame, row: Rect, size: f32, tone: Tone, text: &str) {
    let w = f.c.text_width(size, text).min(row.w);
    f.c.text_fit(Rect::new(row.x + (row.w - w) / 2.0, row.y, w + 1.0, row.h), size, tone, text, Keep::Start);
}

/// Rows taken down a column, while they fit.
struct Column1 {
    r: Rect,
    y: f32,
}

impl Column1 {
    fn new(r: Rect) -> Column1 {
        Column1 { r, y: r.y }
    }

    fn take(&mut self, h: f32) -> Option<Rect> {
        if self.y + h > self.r.bottom() + 0.01 {
            return None;
        }
        let row = Rect::new(self.r.x, self.y, self.r.w, h);
        self.y += h;
        Some(row)
    }

    fn skip(&mut self, h: f32) {
        self.y += h;
    }
}

/// The bottom row: what the pad does here, and "No changes" (the layer shown cleared, once confirmed).
fn foot_actions(f: &mut Frame, body: Rect, row: Rect, keymap: Option<&Keymap>, view: EditView) {
    let pad = style::pad(body.h);
    let small = Text::Small.size(body.h) * PANEL_TEXT;
    let h = row.h * 0.7;
    let y = row.y + (row.h - h) / 2.0;
    let empty = keymap.is_none_or(|k| k.layer(view.layer).is_empty());
    let chip = |f: &mut Frame, kid: KeymapId, x: f32, label: &str, tone: Tone| -> f32 {
        let w = (f.c.text_width(small, label) + pad * 1.2).min(row.w * 0.3);
        let r = Rect::new(x - w, y, w, h);
        f.button(Id::Keymap(kid), r, Look { fill: Some(SURFACE), ..Look::CARD }, |f| {
            f.c.text_fit(r.inset(pad * 0.4), small, tone, label, Keep::Start);
        });
        r.x - pad * 0.4
    };
    let mut x = row.right();
    if view.clearing {
        x = chip(f, KeymapId::ConfirmClear, x, "Clear", Tone::Accent);
        x = chip(f, KeymapId::Back, x, "Keep", Tone::Normal);
        let q = Fmt::<48>::of(format_args!("Clear the {} layer?", view.layer.label()));
        f.c.text_fit(Rect::new(row.x, row.y, (x - row.x).max(0.0), row.h), small, Tone::Normal, &q, Keep::Start);
    } else {
        x = chip(f, KeymapId::Clear, x, "No changes", if empty { Tone::Dim } else { Tone::Normal });
        let hint = "Press an input to select it · Back: the other layer · Home: leave";
        f.c.text_fit(Rect::new(row.x, row.y, (x - row.x - pad).max(0.0), row.h), small * 0.9, Tone::Dim, hint, Keep::Start);
    }
}

/// The middle, at the bottom: the AYN button and the small round buttons beside it (drawn, not
/// remappable), level with Home and Back at `y`; above them the gyro's button ("Gyro · Mouse", "Gyro ·
/// off"). Returns the top of the gyro's button.
fn middle(f: &mut Frame, strip: Rect, y: f32, keymap: Option<&Keymap>, layer: Layer, selected: bool, size: f32) -> f32 {
    let line = size * LINE;
    // The AYN button's row.
    let pill_w = (strip.w * 0.36).min(size * 2.4);
    let pill_h = pill_w * 0.38;
    let cx = strip.center().0;
    let ayn = Rect::new(cx - pill_w / 2.0, y - pill_h / 2.0, pill_w, pill_h);
    f.c.outline(ayn, pill_h / 2.0, PART_W, FIXED);
    for k in [-1.0, 1.0] {
        let r = pill_h * 0.4;
        let x = cx + k * (pill_w / 2.0 + r * 2.4);
        f.c.outline(Rect::new(x - r, y - r, 2.0 * r, 2.0 * r), r, PART_W, FIXED);
    }
    // The gyro's button, above it.
    let own = binding(keymap, layer, Input::Gyro);
    let (text, tone) = match own {
        Some(b) => described(Input::Gyro, &b),
        None => match keymap.and_then(|k| k.get(Input::Gyro)).filter(|_| layer == Layer::Back) {
            Some(_) => (Fmt::of(format_args!("Gyro · as in Main")), Tone::Dim),
            None => (Fmt::of(format_args!("Gyro · off")), Tone::Dim),
        },
    };
    let h = line * 1.6;
    let w = (f.c.text_width(size, &text) + h).min(strip.w);
    let r = Rect::new(cx - w / 2.0, ayn.y - size * 1.2 - h, w, h);
    let look = Look { radius: h / 2.0, fill: None, inset: 0.0, ring: -3.0 };
    // (A finger's room around it.)
    let target = Rect::new(r.x - h * 0.3, r.y - h * 0.3, r.w + h * 0.6, h * 1.6).intersect(strip);
    f.button_in(Id::Keymap(KeymapId::Input(Input::Gyro)), target, r, look, |f| {
        f.c.outline(r, h / 2.0, if selected { SELECTED_W } else { PART_W }, if selected { Tone::High.rgb() } else { PART_LINE });
        centred(f, r, size, tone, &text);
    });
    target.y
}

/// A control in a cluster: its outline and its own mark, in `tone` (blue mapped, grey tuned, dim as on
/// the pad); outlined brighter when selected; a target.
fn control(f: &mut Frame, input: Input, p: Part, tone: Tone, selected: bool, size: f32) {
    let look = Look { radius: p.radius, fill: None, inset: 0.0, ring: -3.0 };
    let shown = p.shown;
    let id = Id::Keymap(KeymapId::Input(input));
    let (lw, lc) = if selected { (SELECTED_W, Tone::High.rgb()) } else { (PART_W, PART_LINE) };
    // A stick takes its group's corners as well (drawn first, so the cap's outline stays on top).
    for corner in p.corners.into_iter().flatten() {
        f.button_in(id.clone(), corner, shown, look, |_| {});
    }
    f.button_in(id, p.target, shown, look, |f| {
        let (cx, cy) = shown.center();
        let name = |f: &mut Frame, t: &str, r: Rect, size: f32| {
            f.c.text_in(r, size, tone, t, Align::Center);
        };
        match input {
            Input::Trigger(s) => {
                // Round on top, flat at the bottom.
                f.c.clip(Some(Rect::new(shown.x - lw, shown.y - lw, shown.w + 2.0 * lw, shown.h + lw / 2.0)));
                f.c.outline(Rect::new(shown.x, shown.y, shown.w, shown.h + shown.w), shown.w / 2.0, lw, lc);
                f.c.clip(None);
                f.c.line((shown.x, shown.bottom()), (shown.right(), shown.bottom()), lw, lc);
                let r = Rect::new(shown.x, shown.bottom() - shown.h * 0.55, shown.w, shown.h * 0.4);
                name(f, if s == Side::Left { "L2" } else { "R2" }, r, size.min(r.h * 0.7));
            }
            Input::Button(b @ (Button::LB | Button::RB)) => {
                f.c.outline(shown, shown.h / 2.0, lw, lc);
                name(f, thor(b), shown, size.min(shown.h * 0.6));
            }
            Input::Button(b @ (Button::A | Button::B | Button::X | Button::Y)) => {
                f.c.outline(shown, shown.w, lw, lc);
                name(f, thor(b), shown, shown.h * 0.48);
            }
            Input::Button(b @ (Button::LS | Button::RS)) => {
                f.c.outline(shown, shown.w, lw, lc);
                name(f, thor(b), shown.inset(-shown.w), shown.h * 0.4);
            }
            Input::Button(b @ (Button::View | Button::Menu)) => {
                f.c.outline(shown, shown.w, lw, lc);
                // Its name on the outer side.
                let w = f.c.text_width(size, thor(b));
                let x = if p.side == Side::Left { shown.x - shown.w * 0.6 - w } else { shown.right() + shown.w * 0.6 };
                f.c.text_in(Rect::new(x, shown.y, w, shown.h), size, tone, thor(b), Align::Start);
            }
            Input::Stick(_) => f.c.outline(shown, shown.w, lw, lc),
            Input::Dpad(d) | Input::StickDir(_, d) => {
                if matches!(input, Input::Dpad(_)) || selected {
                    f.c.outline(shown, p.radius, lw, lc);
                }
                let icon = match d {
                    Dir::Up => Icon::ArrowUp,
                    Dir::Down => Icon::ArrowDown,
                    Dir::Left => Icon::ArrowLeft,
                    Dir::Right => Icon::ArrowRight,
                };
                let side = if matches!(input, Input::Dpad(_)) { shown.w.min(shown.h) * 0.55 } else { shown.w * 0.8 };
                f.c.icon(icon, Rect::new(cx, cy, 0.0, 0.0).inset(-side / 2.0), tone);
            }
            _ => {}
        }
    });
}

/// A button's name as printed on the Thor. Its face buttons are lettered Nintendo-style by position: the
/// bottom one (Xbox A, BTN_SOUTH) is B, the right one A, the top one X, the left one Y.
fn thor(b: Button) -> &'static str {
    match b {
        Button::A => "B",
        Button::B => "A",
        Button::X => "Y",
        Button::Y => "X",
        Button::LB => "L1",
        Button::RB => "R1",
        Button::LS => "L3",
        Button::RS => "R3",
        Button::View => "Select",
        Button::Menu => "Start",
        b => b.label(),
    }
}

/// An input's name on screen: the Thor's.
struct Label(Input);

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Input::Button(b) => f.write_str(thor(b)),
            Input::Trigger(s) => f.write_str(if s == Side::Left { "L2" } else { "R2" }),
            input => input.label(f),
        }
    }
}

struct OutputLabel(Output);

impl std::fmt::Display for OutputLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.0.label(f)
    }
}

/// Chips flowing in lines: a section's label on the left, its chips to the right, wrapping.
struct Flow {
    area: Rect,
    labels_w: f32,
    line_h: f32,
    gap: f32,
    x: f32,
    y: f32,
}

impl Flow {
    fn new(area: Rect, body: Rect) -> Flow {
        let line_h = body.h * BUTTON_H * 0.8;
        let gap = style::pad(body.h) * 0.35;
        Flow { area, labels_w: area.w * LABELS_W, line_h, gap, x: area.x, y: area.y - line_h - gap }
    }

    /// A new section, its label at the start of a new line.
    fn section(&mut self, f: &mut Frame, label: &str) {
        self.y += self.line_h + self.gap;
        self.x = self.area.x + self.labels_w;
        if self.fits() {
            let size = self.line_h * CHIP_TEXT * 0.8;
            f.c.text_fit(Rect::new(self.area.x, self.y, self.labels_w - self.gap, self.line_h), size, Tone::Dim, label, Keep::Start);
        }
    }

    fn fits(&self) -> bool {
        self.y + self.line_h <= self.area.bottom()
    }

    /// A chip: lit if `on`; left out if there is no room.
    fn chip(&mut self, f: &mut Frame, id: Id, label: &str, on: bool) {
        let size = self.line_h * CHIP_TEXT;
        let w = f.c.text_width(size, label) + self.line_h * 0.6;
        if self.x + w > self.area.right() && self.x > self.area.x + self.labels_w {
            self.y += self.line_h + self.gap;
            self.x = self.area.x + self.labels_w;
        }
        if !self.fits() {
            return;
        }
        let r = Rect::new(self.x, self.y, w.min(self.area.right() - self.x), self.line_h);
        self.x += w + self.gap;
        let look = Look { fill: Some(if on { PRESSED } else { SURFACE }), ..Look::CARD };
        f.button(id, r, look, |f| {
            f.c.text_fit(Rect::new(r.x + self.line_h * 0.3, r.y, r.w - self.line_h * 0.3, r.h), size, Tone::high_if(on), label, Keep::Start);
        });
    }
}

fn target(t: Target) -> Id {
    Id::Keymap(KeymapId::Target(t))
}

/// The "map to" screen: the targets that fit the input, as rows of chips; the chosen one lit. In the
/// Back layer, "As in Main" (no binding of its own) and "As on the pad" are both there.
fn map_to(f: &mut Frame, body: Rect, id: u16, input: Input) {
    let s = f.s;
    let layer = s.keymap_edit().layer;
    let b = binding(s.keymaps().get(id), layer, input);
    let out = b.map(|b| b.output);
    let title = match layer {
        Layer::Main => Fmt::<64>::of(format_args!("{} to", Label(input))),
        Layer::Back => Fmt::<64>::of(format_args!("{} to, Back held", Label(input))),
    };
    let area = header(f, body, &title, None, "");
    let mut flow = Flow::new(area, body);
    let kind = input.kind();
    match kind {
        Kind::Stick => {
            flow.section(f, "Motion");
            for side in Side::ALL {
                let label = if side == Side::Left { "Left stick" } else { "Right stick" };
                flow.chip(f, target(Target::Stick(side)), label, out == Some(Output::Stick(side)));
            }
            flow.chip(f, target(Target::Pointer), "Mouse", out == Some(Output::Pointer));
            flow.chip(f, target(Target::Dpad), "D-pad", out == Some(Output::Dpad));
            let keys = match out {
                Some(Output::Keys(k)) => Fmt::<48>::of(format_args!("{}", OutputLabel(Output::Keys(k)))),
                _ => Fmt::of(format_args!("Four keys…")),
            };
            flow.chip(f, target(Target::Keys), &keys, matches!(out, Some(Output::Keys(_))));
        }
        Kind::Gyro => {
            flow.section(f, "Motion");
            flow.chip(f, target(Target::Pointer), "Mouse", out == Some(Output::Pointer));
            for side in Side::ALL {
                let label = if side == Side::Left { "Left stick" } else { "Right stick" };
                flow.chip(f, target(Target::Stick(side)), label, out == Some(Output::Stick(side)));
            }
        }
        Kind::Trigger => {
            flow.section(f, "Trigger");
            for side in Side::ALL {
                flow.chip(f, target(Target::Trigger(side)), if side == Side::Left { "LT" } else { "RT" }, out == Some(Output::Trigger(side)));
            }
            digital_targets(f, &mut flow, out);
        }
        Kind::Digital => digital_targets(f, &mut flow, out),
    }
    flow.section(f, "");
    let own = if kind == Kind::Gyro { "Off" } else { "As on the pad" };
    match layer {
        Layer::Main => flow.chip(f, target(Target::Default), own, out.is_none()),
        Layer::Back => {
            flow.chip(f, target(Target::Default), "As in Main", out.is_none());
            if let Some(o) = as_on_the_pad(input) {
                flow.chip(f, target(Target::Own), own, out == Some(o));
            }
        }
    }
    if kind != Kind::Gyro {
        flow.chip(f, target(Target::Nothing), "Nothing", out == Some(Output::Nothing));
    }
}

/// What holds the gyro on: always, or while a button or trigger is held.
fn holds(f: &mut Frame, body: Rect, id: u16, input: Input) {
    let s = f.s;
    let b = s.param_base(id, input);
    let area = header(f, body, "Gyro active", None, "");
    let mut flow = Flow::new(area, body);
    flow.section(f, "Active");
    let hold = b.and_then(|b| b.hold);
    flow.chip(f, Id::Keymap(KeymapId::Hold(None)), "Always", hold.is_none());
    // (The Thor has no back buttons.)
    for h in gyro_holds().filter(|h| !matches!(h, Input::Button(b) if b.is_back())) {
        let label = Fmt::<16>::of(format_args!("while {}", Label(h)));
        flow.chip(f, Id::Keymap(KeymapId::Hold(Some(h))), &label, hold == Some(h));
    }
}

/// What a button may map to: the pad's buttons (or one pressed on the pad), a key chord, the mouse's
/// buttons, the wheel.
fn digital_targets(f: &mut Frame, flow: &mut Flow, out: Option<Output>) {
    flow.section(f, "Xbox pad");
    for p in PadOut::all() {
        let label = Fmt::<16>::of(format_args!("{}", OutputLabel(Output::Pad(p))));
        flow.chip(f, target(Target::Pad(p)), &label, out == Some(Output::Pad(p)));
    }
    flow.chip(f, target(Target::PressPad), "Press…", false);
    flow.section(f, "Key");
    let key = match out {
        Some(Output::Key(c)) => Fmt::<48>::of(format_args!("{}", OutputLabel(Output::Key(c)))),
        _ => Fmt::of(format_args!("Type a key…")),
    };
    flow.chip(f, target(Target::Key), &key, matches!(out, Some(Output::Key(_))));
    flow.section(f, "Mouse");
    for m in MouseButton::ALL {
        flow.chip(f, target(Target::Mouse(m)), m.label(), out == Some(Output::Mouse(m)));
    }
    flow.section(f, "Wheel");
    for d in Dir::ALL {
        flow.chip(f, target(Target::Wheel(d)), d.label(), out == Some(Output::Wheel(d)));
    }
}

/// Waiting for a pad button.
fn press_pad(f: &mut Frame, body: Rect, _id: u16, input: Input) {
    let title = Fmt::<64>::of(format_args!("{} to", Label(input)));
    let area = header(f, body, &title, None, "Home: back");
    let size = Text::Heading.size(body.h);
    f.c.text_in(area, size, Tone::Normal, "Press a button on the pad", Align::Center);
}

/// A key chord (or a stick's four keys, in turn) typed on the keyboard below.
fn capture(f: &mut Frame, body: Rect, input: Input, keys: Option<(usize, [Chord; 4])>) {
    let kb = f.s.keyboard();
    let held = Chord { key: None, shift: kb.shift.on(), ctrl: kb.ctrl.on(), alt: kb.alt.on(), logo: kb.logo.on() };
    let mut text = Fmt::<96>::new();
    let title = match keys {
        None => Fmt::<128>::of(format_args!("Key for {}: tap modifiers, then the key (Done: the modifiers alone)", Label(input))),
        Some((n, so_far)) => {
            for k in so_far.iter().take(n) {
                let _ = write!(text, "{}  ", OutputLabel(Output::Key(*k)));
            }
            let dir = [Dir::Up, Dir::Left, Dir::Down, Dir::Right].get(n).copied().unwrap_or(Dir::Up);
            Fmt::<128>::of(format_args!("Keys for {}, one at a time: {} ({} of 4)", Label(input), dir.name(), n + 1))
        }
    };
    if !held.is_empty() {
        let _ = write!(text, "{}+", OutputLabel(Output::Key(held)));
    }
    editor::field(f, body, &title, &text, (Id::Keymap(KeymapId::Back), "Cancel"), (Id::Keymap(KeymapId::Done), "Done"));
}
