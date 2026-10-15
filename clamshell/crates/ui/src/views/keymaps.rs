//! The keymaps' screens, over the settings tab (see `core`'s `keymap_ui`): the picker (a scrolled list:
//! Default, then the keymaps, each with who uses it and an Edit button; New), a keymap's menu, deleting
//! one in use, the edit screen (one layer of the keymap, Main or Back held: the pad's controls drawn in
//! two clusters at the sides, as they sit beside the screen, each the layer changes with a label; between
//! the label columns a panel for the selected input: the ways to remap it as icons, its parameters; "No
//! changes" below), the same while a control is picked on it, what holds the gyro on, a part of the mouse
//! on its drawing, and a key chord typed on the keyboard.
//!
//! On black like the rest: the pad's parts are thin dim outlines; blue is what a keymap maps to something
//! else, grey what it only tunes.

use std::fmt::Write;

use clamshell_core::keymap::{Binding, Button, Change, Chord, Dir, Input, Keymap, Kind, Layer, MouseButton, Output, PadOut, Side, change, params};
use clamshell_core::keymap_ui::{EditView, Screen, holds, picked, way_of, ways};
use clamshell_core::slider::{Control, Slider};
use clamshell_core::{Id, KeymapId, ListId, RowPart, Scope, Target};

use super::format::Fmt;
use super::scroll::Scrolled;
use super::pad::{Pad, Part};
use super::style::{self, Look, PRESSED, SURFACE_HI, Text, Tone};
use super::{Frame, Track, editor, widgets};
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

pub(super) fn draw(f: &mut Frame, body: Rect, screen: Screen) {
    match screen {
        Screen::List => list(f, body),
        Screen::Menu(id) => menu(f, body, id),
        Screen::Delete(id) => delete(f, body, id),
        Screen::Edit(id) => edit(f, body, id, None),
        Screen::Pick { id, input } => edit(f, body, id, Some(Picking::Target(input))),
        Screen::Holds { id, input } => edit(f, body, id, Some(Picking::Hold(input))),
        Screen::Mouse { id, input } => mouse(f, body, id, input),
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
        f.text_button(id, r, size * 0.85, Tone::Fn, label, Align::Center);
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

/// What is picked on the edit screen's drawing: what an input maps to, or what holds the gyro on.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Picking {
    Target(Input),
    Hold(Input),
}

impl Picking {
    /// Whether `input` can be picked.
    fn takes(self, input: Input) -> bool {
        match self {
            Picking::Target(from) => from == input || picked(from, input).is_some(),
            Picking::Hold(_) => holds(input),
        }
    }
}

/// A choice's header (what an input is remapped to, what holds the gyro on): the title centred and
/// Cancel on the right; no back arrow, since choosing something closes it and Cancel gives up. Returns
/// the area below it.
fn choice_header(f: &mut Frame, body: Rect, title: &str) -> Rect {
    let pad = style::pad(body.h);
    let mut area = style::content(body);
    let bar = area.take_top(body.h * HEADER_H);
    let size = Text::Heading.size(body.h);
    let cancel_w = f.c.text_width(size * 0.85, "Cancel") + pad * 2.0;
    let cancel = Rect::new(bar.right() - cancel_w, bar.y + bar.h * 0.1, cancel_w, bar.h * 0.8);
    f.text_button(Id::Keymap(KeymapId::Back), cancel, size * 0.85, Tone::Fn, "Cancel", Align::Center);
    centred(f, Rect::new(bar.x + cancel_w, bar.y, (bar.w - 2.0 * cancel_w).max(1.0), bar.h), size, Tone::High, title);
    area.y += pad * 0.4;
    area.h -= pad * 0.4;
    area
}

/// "Remap A to…".
fn remap_title(input: Input) -> Fmt<64> {
    Fmt::of(format_args!("Remap {} to…", Label(input)))
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
                f.c.text_in(e, size * 0.85, if f.pressed(&Id::Row { list: ListId::Keymaps, i: i as u16, part: RowPart::Edit }) { Tone::High } else { Tone::Fn }, "Edit", Align::Center);
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
        f.text_button(id.clone(), r, size, *tone, label, Align::Start);
    }
}

fn menu(f: &mut Frame, body: Rect, id: u16) {
    let name = f.s.keymap_name(id);
    let area = header(f, body, name, None, "");
    let k = |id| Id::Keymap(id);
    let items =
        [(k(KeymapId::Edit), "Edit", Tone::Fn), (k(KeymapId::Rename), "Rename", Tone::Fn), (k(KeymapId::Duplicate), "Duplicate", Tone::Fn), (k(KeymapId::Delete), "Delete", Tone::Accent)];
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
    buttons(f, rest, body, &[(Id::Keymap(KeymapId::Back), "Keep it", Tone::Fn), (Id::Keymap(KeymapId::ConfirmDelete), "Delete", Tone::Accent)]);
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
const GAP: f32 = 0.03;
const LABEL_COLUMN_W: f32 = 0.1;
const FOOT_ROW_H: f32 = 0.065;
const PANEL_ROW_H: f32 = 0.055;
/// The panel's and the bottom row's text, of the small text size.
const PANEL_TEXT: f32 = 0.8;
/// The controls' outlines, and the lines in to the labels (blue, dimmed).
const PART_LINE: Rgb = Rgb(0x4a4a54);
/// The buttons drawn that can't be remapped (Home, the AYN button and its neighbours).
const FIXED: Rgb = Rgb(0x2a2a30);
/// While a control is picked, those that can't be: barely there.
const MUTED: Rgb = Rgb(0x1c1c21);
const LEADER_ALPHA: f32 = 0.45;
/// Line widths in logical pixels: the controls', the lines in, a selected control's.
const PART_W: f32 = 1.5;
const LEADER_W: f32 = 1.0;
const SELECTED_W: f32 = 2.5;
/// The least room between labels, and the narrowest column (of the labels' text size) that has words.
const LABEL_GAP: f32 = 1.2;
const LABEL_MIN: f32 = 4.5;
/// The most controls in a cluster.
const CONTROLS: usize = 16;

/// What a binding maps to, in words; the pad's buttons by the Thor's printed names, as for inputs (games
/// get them by position: the printed B is the Xbox pad's A).
fn mapping(b: Binding) -> Fmt<48> {
    Fmt::of(format_args!("{}", OutputLabel(b.output)))
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
/// held, a segmented switch; faded while a control is picked, when it stays as it is).
fn edit_header(f: &mut Frame, body: Rect, title: &str, layer: Layer, faded: bool) {
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
        let tone = match (faded, on) {
            (true, true) => Tone::Normal,
            (true, false) => Tone::Dim,
            (false, true) => Tone::High,
            (false, false) => Tone::Fn,
        };
        f.text_button(Id::Keymap(KeymapId::Layer(l)), r, size, tone, l.label(), Align::Center);
    }
    let title_w = (right - pad * 0.5 - bar.x - pad * 0.4).max(0.0);
    f.c.text_fit(Rect::new(bar.x + pad * 0.4, bar.y, title_w, bar.h), Text::Heading.size(body.h), Tone::High, title, Keep::Start);
}

/// The edit screen: the controls in two clusters at the sides (see [`super::pad`]), each a target; one
/// the layer shown changes has a line in to a label (blue: mapped to something else; grey: as on the
/// pad, tuned). Home and the AYN button are drawn dim, not remappable (their codes are a device test, see
/// docs/thor-gameplan.md); Back is the layer key, a target that switches the layer shown, lit while it is
/// the Back layer. Between the label columns, the panel: the selected input, what it does in the layer,
/// the ways to remap it, and its parameters (a value slid for the slider, a switch tapped). The gyro's
/// button sits at the bottom, over the AYN button. Below, a hint and "No changes". While a control is
/// picked (what the selected input maps to, or what holds the gyro on), the controls that can't be are
/// dimmed, and the panel and the hint say what to do.
fn edit(f: &mut Frame, body: Rect, id: u16, picking: Option<Picking>) {
    let s = f.s;
    let keymap = s.keymaps().get(id);
    let view = s.keymap_edit();
    let layer = view.layer;
    let name = keymap.map_or("", |k| k.name.as_str());
    edit_header(f, body, name, layer, picking.is_some());
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
        // While a control is picked, those that can't be have no label.
        if picking.is_some_and(|p| !p.takes(input)) {
            continue;
        }
        let (text, tone) = described(input, &b);
        let c = Callout { part, text, tone, top: 0.0 };
        if part.side == Side::Left { left.push(c) } else { right.push(c) }
    }
    for column in [&mut left, &mut right] {
        column.place(area.y, area.bottom(), line, size * LABEL_GAP);
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
            let muted = picking.is_some_and(|p| !p.takes(Input::Stick(side)));
            f.c.outline(well, well.w, PART_W, if muted { MUTED } else { PART_LINE });
        }
    }
    let click = |i: &Input| matches!(i, Input::Button(Button::LS | Button::RS));
    for input in Input::all().filter(|i| !click(i)).chain(Input::all().filter(click)) {
        if let Some(part) = pad.part(input) {
            let tone = binding(keymap, layer, input).map_or(Tone::Dim, |b| described(input, &b).1);
            let muted = picking.is_some_and(|p| !p.takes(input));
            // (Picking the gyro's hold, the one holding it now is lit.)
            let hold = match picking {
                Some(Picking::Hold(gyro)) => s.param_base(id, gyro).and_then(|b| b.hold) == Some(input),
                _ => false,
            };
            control(f, input, part, tone, view.selected == Some(input) || hold, muted, size);
        }
    }

    // The labels; where their column is too narrow for words (a narrow window), a dot at the line's end
    // (the panel says what the selected one does).
    let words = lw >= size * LABEL_MIN;
    for (side, column) in [(Side::Left, &left), (Side::Right, &right)] {
        let cluster = pad.cluster(side);
        for c in column.iter() {
            if !words {
                let x = if side == Side::Left { cluster.right() + gap - size * 0.3 } else { cluster.x - gap + size * 0.3 };
                f.c.circle(x, c.top + line / 2.0, size * 0.15, c.tone);
                continue;
            }
            let w = f.c.text_width(size, &c.text).min(lw);
            let x = if side == Side::Left { cluster.right() + gap } else { cluster.x - gap - w };
            f.c.text_fit(Rect::new(x, c.top, w + 1.0, line), size, c.tone, &c.text, Keep::Start);
        }
    }

    // Home (ClamShell's, drawn dim) and Back (the layer key), at the screen's bottom corners.
    let fixed = if picking.is_some() { MUTED } else { FIXED };
    if let Some(r) = pad.foot(Side::Left) {
        f.c.outline(r, r.w, PART_W, fixed);
        let w = f.c.text_width(size, "Home");
        f.c.text_in(Rect::new(r.x - r.w * 0.6 - w, r.y, w, r.h), size, fixed, "Home", Align::Start);
    }
    if let Some(r) = pad.foot(Side::Right) {
        let held = layer == Layer::Back;
        // (Faded while a control is picked: the layer stays as it is.)
        let faded = picking.is_some();
        let name = if held { "Back held" } else { "Back" };
        let w = f.c.text_width(size, name);
        let row = pad.foot_row(Side::Right).unwrap_or(r);
        let target = Rect::new(r.x - r.w, r.y - r.w, r.w * 2.6 + w, r.h + 2.0 * r.w).intersect(row);
        let look = Look { radius: r.w / 2.0, fill: None, inset: 0.0, ring: -3.0, press_fill: true };
        f.button_in(Id::Keymap(KeymapId::Layer(layer.other())), target, r, look, |f| {
            if held {
                f.c.circle(r.center().0, r.center().1, r.w / 2.0, PRESSED);
            }
            let line = if faded { MUTED } else if held { Tone::High.rgb() } else { PART_LINE };
            f.c.outline(r, r.w, PART_W, line);
            let tone = if faded { MUTED } else { Tone::high_if(held).rgb() };
            f.c.text_in(Rect::new(r.right() + r.w * 0.6, r.y, w, r.h), size, tone, name, Align::Start);
        });
    }

    // The middle: the gyro and the AYN button at the bottom, the panel above.
    let foot = pad.foot(Side::Left).map_or(area.bottom() - line, |r| r.center().1);
    let mark = if view.selected == Some(Input::Gyro) {
        Mark::Selected
    } else if picking.is_some_and(|p| !p.takes(Input::Gyro)) {
        Mark::Muted
    } else {
        Mark::Plain
    };
    let gyro_top = middle(f, strip, foot, keymap, layer, mark, size);
    let margin = (strip.w * 0.04).min(size);
    let panel_r = Rect::new(strip.x + margin, area.y, (strip.w - 2.0 * margin).max(1.0), (gyro_top - area.y - size).max(1.0));
    match picking {
        Some(p) => picking_panel(f, body, panel_r, id, p),
        None => panel(f, body, panel_r, id, keymap, view),
    }
    foot_actions(f, body, foot_row, keymap, view, picking);

}

/// The panel while a control is picked: what for, what to do, and Cancel (for the gyro's hold, Always
/// beside it).
fn picking_panel(f: &mut Frame, body: Rect, r: Rect, id: u16, picking: Picking) {
    let small = Text::Small.size(body.h) * PANEL_TEXT;
    let heading = Text::Heading.size(body.h) * 0.9;
    let mut col = Column1::new(r);
    let (title, what) = match picking {
        Picking::Target(input) => (
            remap_title(input),
            match input.kind() {
                Kind::Stick => "Tap or move a stick, or tap the D-pad",
                Kind::Gyro => "Tap or move a stick",
                Kind::Trigger => "Tap or press a trigger or a button",
                Kind::Digital => "Tap or press a button",
            },
        ),
        Picking::Hold(_) => (Fmt::of(format_args!("Gyro active while…")), "Tap or press a button or a trigger"),
    };
    // (On two lines if it doesn't fit on one.)
    let lines = if f.c.text_width(heading, &title) > r.w { 2.0 } else { 1.0 };
    if let Some(row) = col.take(heading * LINE * lines) {
        centred_lines(f, row, heading, Tone::High, &title);
    }
    if let Some(row) = col.take(small * LINE * 2.2) {
        centred_lines(f, row, small, Tone::Normal, what);
    }
    col.skip(style::pad(body.h) * 0.3);
    let Some(row) = col.take(body.h * BUTTON_H * 0.65) else {
        return;
    };
    match picking {
        Picking::Hold(gyro) => {
            let always = f.s.param_base(id, gyro).is_some_and(|b| b.hold.is_none());
            let (a, c) = (Rect::new(row.x, row.y, row.w / 2.0, row.h), Rect::new(row.x + row.w / 2.0, row.y, row.w / 2.0, row.h));
            f.text_button(Id::Keymap(KeymapId::Hold(None)), a, small, if always { Tone::High } else { Tone::Fn }, "Always", Align::Center);
            f.text_button(Id::Keymap(KeymapId::Back), c, small, Tone::Fn, "Cancel", Align::Center);
        }
        Picking::Target(_) => f.text_button(Id::Keymap(KeymapId::Back), row, small, Tone::Fn, "Cancel", Align::Center),
    }
}

/// The icon for a way to remap an input.
fn way_icon(way: Target) -> Icon {
    match way {
        Target::Pick => Icon::Gamepad,
        Target::Key | Target::Keys => Icon::Keyboard,
        Target::MouseParts | Target::Pointer => Icon::Mouse,
        Target::Nothing => Icon::Off,
        _ => Icon::Reset,
    }
}

/// The panel: the selected input's name; what it does in the layer shown; the ways to remap it, as icons
/// (the way it is mapped now in white); its parameters, a row each (one whose slider is up shows it in its
/// place).
fn panel(f: &mut Frame, body: Rect, r: Rect, id: u16, keymap: Option<&Keymap>, view: EditView) {
    let s = f.s;
    let pad = style::pad(body.h);
    let small = Text::Small.size(body.h) * PANEL_TEXT;
    let mut col = Column1::new(r);
    let Some(input) = view.selected else {
        let y = r.y + r.h * 0.4;
        for (i, t) in ["Press or touch an input", "to see what it does"].into_iter().enumerate() {
            centred(f, Rect::new(r.x, y + i as f32 * small * LINE, r.w, small * LINE), small, Tone::Dim, t);
        }
        return;
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
        // (Under its heading the gyro needs no "Gyro ·".)
        (Some(b), _) if input == Input::Gyro && change(input, Some(&b)) == Change::Mapped => (mapping(b), Tone::Fn),
        (Some(b), _) => described(input, &b),
        (None, Layer::Back) => {
            let main = keymap.and_then(|k| k.get(input)).map(|b| described(input, b).0).unwrap_or_else(|| as_on_pad(input));
            (Fmt::of(format_args!("As in Main: {}", main.as_str())), Tone::Dim)
        }
        (None, Layer::Main) => (as_on_pad(input), Tone::Dim),
    };
    if let Some(row) = col.take(small * LINE * 2.2) {
        match (own, layer) {
            (None, Layer::Back) => {
                let main = keymap.and_then(|k| k.get(input)).map(|b| described(input, b).0).unwrap_or_else(|| as_on_pad(input));
                let line = small * LINE;
                let top = row.y + (row.h - 2.0 * line) / 2.0;
                centred(f, Rect::new(row.x, top, row.w, line), small, Tone::Dim, "As in Main");
                centred(f, Rect::new(row.x, top + line, row.w, line), small, Tone::Dim, &main);
            }
            // (The panel's own content in white: blue is for labels of what is remapped, and actions.)
            _ => centred_lines(f, row, small, if tone == Tone::Fn { Tone::High } else { tone }, &what),
        }
    }
    // The ways to remap it.
    let bh = body.h * BUTTON_H * 0.75;
    col.skip(pad * 0.3);
    if let Some(row) = col.take(bh) {
        let ways = ways(input.kind());
        let now = way_of(own.map(|b| b.output));
        let w = row.w / ways.len().max(1) as f32;
        for (i, way) in ways.iter().enumerate() {
            let r = Rect::new(row.x + i as f32 * w, row.y, w, row.h);
            let kid = Id::Keymap(KeymapId::Target(*way));
            let tone = if *way == now || f.pressed(&kid) { Tone::High } else { Tone::Fn };
            f.button(kid, r, Look::TEXT, |f| f.c.icon(way_icon(*way), r.square_center(row.h * 0.5), tone));
        }
    }
    col.skip(pad * 0.4);
    // The parameters (from the main layer's in the Back layer without its own, else as on the pad).
    let Some(base) = s.param_base(id, input) else {
        return;
    };
    let rh = body.h * PANEL_ROW_H;
    for p in params(input, base.output) {
        let Some(row) = col.take(rh) else {
            return;
        };
        let v = p.get(&base);
        let changed = v != p.default_for(input, base.output);
        if p.switch() {
            f.button(Id::Keymap(KeymapId::Toggle(p)), row, Look::ROW, |f| {
                let inner = row.inset(small * 0.4);
                let sw = Rect::new(inner.right() - row.h * 0.9, row.y, row.h * 0.9, row.h);
                f.c.text_fit(Rect::new(inner.x, row.y, sw.x - inner.x, row.h), small, Tone::Dim, p.label(), Keep::Start);
                widgets::switch_shape(f.c, sw, v > 0);
            });
        } else {
            let value = Fmt::<16>::of(format_args!("{v} %"));
            let slider = view.slider.filter(|sl| sl.control == Control::Param(p));
            let track = Track { x0: r.x, w: r.w };
            f.button(Id::Control(Control::Param(p)), row, Look::ROW, |f| match slider {
                Some(sl) => slider_row(f, row, track, sl, small, p.label()),
                None => param_row(f, row, small, p.label(), &value, changed),
            });
            f.track(track);
        }
    }
    if input == Input::Gyro && matches!(base.output, Output::Pointer | Output::Stick(_)) {
        let Some(row) = col.take(rh) else {
            return;
        };
        let value = match base.hold {
            Some(h) => Fmt::<24>::of(format_args!("while {}", Label(h))),
            None => Fmt::of(format_args!("Always")),
        };
        f.button(Id::Keymap(KeymapId::Holds), row, Look::ROW, |f| param_row(f, row, small, "Active", &value, base.hold.is_some()));
    }
    if params(input, base.output).next().is_some()
        && let Some(row) = col.take(small * LINE * 1.5)
    {
        centred(f, row.shifted(0.0, small * 0.3), small * 0.85, Tone::Dim, "Slide a value to set it");
    }
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

/// A parameter's row while its slider is up (the bar's slider, within the panel): the name and value
/// above, the track along the row with the thumb under the finger.
fn slider_row(f: &mut Frame, row: Rect, track: Track, sl: Slider, size: f32, label: &str) {
    f.c.rounded(row, row.h * 0.2, SURFACE_HI);
    let text = Fmt::<32>::of(format_args!("{label} {} %", sl.value));
    centred(f, Rect::new(row.x, row.y, row.w, row.h * 0.55), size, Tone::High, &text);
    let t = Track { x0: track.x0 + sl.track_start(), w: sl.length };
    widgets::track(f.c, t, row.y + row.h * 0.75, row.h * 0.07, row.h * 0.15, Some(sl.value), Tone::High);
}

/// Text centred in a row, on two lines (split at a space near the middle) if it doesn't fit on one.
fn centred_lines(f: &mut Frame, row: Rect, size: f32, tone: Tone, text: &str) {
    let line = size * LINE;
    if f.c.text_width(size, text) <= row.w {
        centred(f, Rect::new(row.x, row.y + (row.h - line) / 2.0, row.w, line), size, tone, text);
        return;
    }
    let middle = text.len() / 2;
    let split = text.match_indices(' ').map(|(i, _)| i).min_by_key(|i| i.abs_diff(middle));
    let (a, b) = match split {
        Some(i) => (text.get(..i).unwrap_or(text), text.get(i + 1..).unwrap_or("")),
        None => (text, ""),
    };
    let top = row.y + (row.h - 2.0 * line) / 2.0;
    centred(f, Rect::new(row.x, top, row.w, line), size, tone, a);
    centred(f, Rect::new(row.x, top + line, row.w, line), size, tone, b);
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

/// The bottom row: what the pad does here, and "No changes" (the layer shown cleared, once confirmed);
/// while a control is picked, only what the pad does.
fn foot_actions(f: &mut Frame, body: Rect, row: Rect, keymap: Option<&Keymap>, view: EditView, picking: Option<Picking>) {
    let pad = style::pad(body.h);
    let small = Text::Small.size(body.h) * PANEL_TEXT;
    let h = row.h * 0.7;
    let y = row.y + (row.h - h) / 2.0;
    let empty = keymap.is_none_or(|k| k.layer(view.layer).is_empty());
    let chip = |f: &mut Frame, kid: KeymapId, x: f32, label: &str, tone: Tone| -> f32 {
        let w = (f.c.text_width(small, label) + pad * 1.2).min(row.w * 0.3);
        let r = Rect::new(x - w, y, w, h);
        f.text_button(Id::Keymap(kid), r, small, tone, label, Align::Center);
        r.x - pad * 0.4
    };
    let mut x = row.right();
    if let Some(p) = picking {
        let hint = match p {
            Picking::Target(_) => "Tap or press the control it should be · Back: cancel",
            Picking::Hold(_) => "Tap or press what turns the gyro on while held · Back: cancel",
        };
        f.c.text_fit(row, small * 0.9, Tone::Dim, hint, Keep::Start);
    } else if view.clearing {
        x = chip(f, KeymapId::ConfirmClear, x, "Clear", Tone::Accent);
        x = chip(f, KeymapId::Back, x, "Keep", Tone::Fn);
        let q = Fmt::<48>::of(format_args!("Clear the {} layer?", view.layer.label()));
        f.c.text_fit(Rect::new(row.x, row.y, (x - row.x).max(0.0), row.h), small, Tone::Normal, &q, Keep::Start);
    } else {
        x = chip(f, KeymapId::Clear, x, "No changes", if empty { Tone::Dim } else { Tone::Fn });
        let hint = "Press an input to select it · Back: the other layer · Home: leave";
        f.c.text_fit(Rect::new(row.x, row.y, (x - row.x - pad).max(0.0), row.h), small * 0.9, Tone::Dim, hint, Keep::Start);
    }
}

/// The middle, at the bottom: the AYN button and the small round buttons beside it (drawn, not
/// remappable), level with Home and Back at `y`; above them the gyro's button ("Gyro · Mouse", "Gyro ·
/// off"). Returns the top of the gyro's button.
/// How a control is drawn besides its tone: as it is, selected, or faded (it can't be picked).
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Plain,
    Selected,
    Muted,
}

fn middle(f: &mut Frame, strip: Rect, y: f32, keymap: Option<&Keymap>, layer: Layer, mark: Mark, size: f32) -> f32 {
    let line = size * LINE;
    // The AYN button's row.
    let pill_w = (strip.w * 0.36).min(size * 2.4);
    let pill_h = pill_w * 0.38;
    let cx = strip.center().0;
    let ayn = Rect::new(cx - pill_w / 2.0, y - pill_h / 2.0, pill_w, pill_h);
    let fixed = if mark == Mark::Muted { MUTED } else { FIXED };
    f.c.outline(ayn, pill_h / 2.0, PART_W, fixed);
    for k in [-1.0, 1.0] {
        let r = pill_h * 0.4;
        let x = cx + k * (pill_w / 2.0 + r * 2.4);
        f.c.outline(Rect::new(x - r, y - r, 2.0 * r, 2.0 * r), r, PART_W, fixed);
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
    let look = Look { radius: h / 2.0, fill: None, inset: 0.0, ring: -3.0, press_fill: true };
    // (A finger's room around it.)
    let target = Rect::new(r.x - h * 0.3, r.y - h * 0.3, r.w + h * 0.6, h * 1.6).intersect(strip);
    f.button_in(Id::Keymap(KeymapId::Input(Input::Gyro)), target, r, look, |f| {
        let (lw, lc) = match mark {
            Mark::Selected => (SELECTED_W, Tone::High.rgb()),
            Mark::Muted => (PART_W, MUTED),
            Mark::Plain => (PART_W, PART_LINE),
        };
        f.c.outline(r, h / 2.0, lw, lc);
        if mark == Mark::Muted {
            f.c.text_in(r, size, MUTED, &text, Align::Center);
        } else {
            centred(f, r, size, tone, &text);
        }
    });
    target.y
}

/// A control in a cluster: its outline and its own mark, in `tone` (blue mapped, grey tuned, dim as on
/// the pad); outlined brighter when selected, darker when `muted` (it can't be picked); a target.
fn control(f: &mut Frame, input: Input, p: Part, tone: Tone, selected: bool, muted: bool, size: f32) {
    let look = Look { radius: p.radius, fill: None, inset: 0.0, ring: -3.0, press_fill: true };
    let shown = p.shown;
    let id = Id::Keymap(KeymapId::Input(input));
    let (lw, lc) = match (selected, muted) {
        (true, _) => (SELECTED_W, Tone::High.rgb()),
        (false, true) => (PART_W, MUTED),
        (false, false) => (PART_W, PART_LINE),
    };
    let tone = if muted { MUTED } else { tone.rgb() };
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
        match self.0 {
            Output::Pad(PadOut::Button(b)) => f.write_str(thor(b)),
            Output::Pad(PadOut::Trigger(s)) | Output::Trigger(s) => f.write_str(if s == Side::Left { "L2" } else { "R2" }),
            out => out.label(f),
        }
    }
}

/// The mouse's drawing, of the room under the header: its height, and its width of its height.
const MOUSE_H: f32 = 0.8;
const MOUSE_ASPECT: f32 = 0.6;

/// A part of the mouse for `input`, on a drawing of one: the left and right buttons (the body's top
/// halves), the wheel (pressed: the middle button) with arrows for its turns, the two side buttons on the
/// left (Forward above Back). Each part is a target; the one it maps to now is lit. Chosen, it closes;
/// Cancel (or Back) gives up.
fn mouse(f: &mut Frame, body: Rect, id: u16, input: Input) {
    let s = f.s;
    let out = binding(s.keymaps().get(id), s.keymap_edit().layer, input).map(|b| b.output);
    let area = choice_header(f, body, &remap_title(input));
    let small = Text::Small.size(body.h);
    let hint_h = small * LINE * 1.5;
    let room = Rect::new(area.x, area.y, area.w, area.h - hint_h);
    let h = (room.h * MOUSE_H).min(room.w * 0.5 / MOUSE_ASPECT);
    let w = h * MOUSE_ASPECT;
    let m = Rect::new(room.center().0 - w / 2.0, room.y + (room.h - h) / 2.0, w, h);
    let cx = m.center().0;
    let split = m.y + h * 0.42;
    let lit = |o: Output| out == Some(o);
    let line = |on: bool| if on { (SELECTED_W, Tone::High.rgb()) } else { (PART_W, PART_LINE) };
    let look = Look { radius: w * 0.1, fill: None, inset: 0.0, ring: -3.0, press_fill: false };
    let label = |f: &mut Frame, kid: &Id, on: bool| if on || f.pressed(kid) { Tone::High } else { Tone::Fn };

    // The wheel and its turns' arrows (the line between the buttons stops short of them).
    let wheel = Rect::new(cx - w * 0.07, m.y + h * 0.11, w * 0.14, h * 0.14);
    let a = w * 0.14;
    // The body, the line between the buttons and the palm, and between the buttons.
    f.c.outline(m, w * 0.45, PART_W, PART_LINE);
    f.c.line((m.x + PART_W, split), (m.right() - PART_W, split), PART_W, PART_LINE);
    f.c.line((cx, wheel.bottom() + a), (cx, split), PART_W, PART_LINE);
    // The buttons: the body's top halves, named in them.
    for (b, half) in [(MouseButton::Left, Rect::new(m.x, m.y, w / 2.0, split - m.y)), (MouseButton::Right, Rect::new(cx, m.y, w / 2.0, split - m.y))] {
        let kid = Id::Keymap(KeymapId::Target(Target::Mouse(b)));
        let on = lit(Output::Mouse(b));
        f.button(kid.clone(), half, look, |f| {
            let name = Rect::new(half.x, split - half.h * 0.22, half.w, half.h * 0.16);
            let tone = label(f, &kid, on);
            f.c.text_in(name, small * 0.9, tone, if b == MouseButton::Left { "Left click" } else { "Right click" }, Align::Center);
        });
    }
    // The wheel (the middle button) and its turns.
    let turns = [
        (Dir::Up, Rect::new(cx - a / 2.0, wheel.y - a, a, a), Icon::ArrowUp),
        (Dir::Down, Rect::new(cx - a / 2.0, wheel.bottom(), a, a), Icon::ArrowDown),
        (Dir::Left, Rect::new(wheel.x - a * 1.05, wheel.center().1 - a / 2.0, a, a), Icon::ArrowLeft),
        (Dir::Right, Rect::new(wheel.right() + a * 0.05, wheel.center().1 - a / 2.0, a, a), Icon::ArrowRight),
    ];
    let kid = Id::Keymap(KeymapId::Target(Target::Mouse(MouseButton::Middle)));
    let on = lit(Output::Mouse(MouseButton::Middle));
    f.button(kid, wheel, Look { radius: wheel.w / 2.0, ..look }, |f| {
        let (lw, lc) = line(on);
        f.c.outline(wheel, wheel.w / 2.0, lw, lc);
    });
    for (d, r, icon) in turns {
        let kid = Id::Keymap(KeymapId::Target(Target::Wheel(d)));
        let on = lit(Output::Wheel(d));
        f.button(kid.clone(), r, look, |f| {
            let tone = label(f, &kid, on);
            f.c.icon(icon, r.square_center(a * 0.6), tone);
        });
    }
    // The side buttons, on the left edge, named beside them.
    let sw = w * 0.07;
    for (b, y, name) in [(MouseButton::Forward, m.y + h * 0.47, "Forward"), (MouseButton::Back, m.y + h * 0.6, "Back")] {
        let shown = Rect::new(m.x - sw * 0.5, y, sw, h * 0.1);
        let tw = f.c.text_width(small * 0.9, name);
        let target = Rect::new(shown.x - tw - sw * 2.0, y - h * 0.015, tw + sw * 3.0, h * 0.13);
        let kid = Id::Keymap(KeymapId::Target(Target::Mouse(b)));
        let on = lit(Output::Mouse(b));
        f.button_in(kid.clone(), target, shown, Look { radius: sw / 2.0, ..look }, |f| {
            let (lw, lc) = line(on);
            f.c.outline(shown, sw / 2.0, lw, lc);
            let tone = label(f, &kid, on);
            f.c.text_in(Rect::new(target.x, y, tw, shown.h), small * 0.9, tone, name, Align::Start);
        });
    }
    let hint = "A button, the wheel (its middle button) or an arrow (a turn of it)";
    f.c.text_fit(Rect::new(area.x, area.bottom() - hint_h, area.w, hint_h), small * 0.9, Tone::Dim, hint, Keep::Start);
}

/// A key chord (or a stick's four keys, in turn) typed on the keyboard below.
fn capture(f: &mut Frame, body: Rect, input: Input, keys: Option<(usize, [Chord; 4])>) {
    let kb = f.s.keyboard();
    let held = Chord { key: None, shift: kb.shift.on(), ctrl: kb.ctrl.on(), alt: kb.alt.on(), logo: kb.logo.on() };
    let mut text = Fmt::<96>::new();
    let title = match keys {
        None => Fmt::<128>::of(format_args!("Remap {} to… modifiers, then a key (Done: the modifiers alone)", Label(input))),
        Some((n, so_far)) => {
            for k in so_far.iter().take(n) {
                let _ = write!(text, "{}  ", OutputLabel(Output::Key(*k)));
            }
            let dir = [Dir::Up, Dir::Left, Dir::Down, Dir::Right].get(n).copied().unwrap_or(Dir::Up);
            Fmt::<128>::of(format_args!("Remap {} to… the key for {} ({} of 4)", Label(input), dir.name(), n + 1))
        }
    };
    if !held.is_empty() {
        let _ = write!(text, "{}+", OutputLabel(Output::Key(held)));
    }
    editor::field(f, body, &title, &text, (Id::Keymap(KeymapId::Back), "Cancel"), (Id::Keymap(KeymapId::Done), "Done"));
}
