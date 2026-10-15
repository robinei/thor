//! The keymaps' screens, over the settings tab (see `core`'s `keymap_ui`): the picker (a scrolled list:
//! Default, then the keymaps, each with who uses it and an Edit button; New), a keymap's menu, deleting
//! one in use, the edit screen ("map from": the pad's controls drawn in two clusters at the sides, as they
//! sit beside the screen, each with a label of what it maps to, in blue where it differs from the pad),
//! what one input maps to ("map to": the targets that fit it, as rows of
//! buttons, the chosen one lit), a key chord typed on the keyboard, and waiting for a pad button.
//!
//! On black like the rest: the pad's parts are thin dim outlines, the only colour is blue for what a
//! keymap changes.

use std::fmt::Write;

use clamshell_core::keymap::{Binding, Button, Chord, Dir, Input, Keymap, Kind, MouseButton, Output, PadOut, SPEEDS, Side, THRESHOLDS, gyro_holds};
use clamshell_core::keymap_ui::Screen;
use clamshell_core::{Id, KeymapId, ListId, RowPart, Scope, Target};

use super::format::Fmt;
use super::scroll::Scrolled;
use super::pad::{Pad, Part};
use super::style::{self, Look, PRESSED, SURFACE, SURFACE_HI, Text, Tone};
use super::{Frame, editor};
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

/// An input's binding in keymap `k`, if it has one.
fn binding(k: Option<&Keymap>, input: Input) -> Option<Binding> {
    k.and_then(|k| k.get(input)).copied()
}

/// The edit screen: the labels' text, of the small text size; of the area's width, a cluster's width,
/// the room between it and its labels (where the lines turn), and the labels' column's width.
const LABEL: f32 = 0.66;
const CLUSTER_W: f32 = 0.18;
const GAP: f32 = 0.045;
const LABEL_COLUMN_W: f32 = 0.17;
/// The controls' outlines, and the lines in to the labels (grey, and blue for a mapping).
const PART_LINE: Rgb = Rgb(0x4a4a54);
const LEADER: Rgb = Rgb(0x2c2c34);
const LEADER_MAPPED: f32 = 0.45;
/// Line widths in logical pixels: the controls', the lines in.
const PART_W: f32 = 1.5;
const LEADER_W: f32 = 1.0;
/// The most controls in a cluster.
const CONTROLS: usize = 16;

/// A control's label beside its cluster: the control, what it maps to (if not as on the pad), and its
/// block's top and height once placed.
#[derive(Clone, Copy)]
struct Callout {
    input: Input,
    part: Part,
    mapped: Option<Fmt<48>>,
    top: f32,
    h: f32,
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

    /// Places the blocks as near level with their lines as they can be (the first line of each, `line`
    /// high, at its line's height), within `y0`..`y1`, `gap` apart or less if they don't fit.
    fn place(&mut self, y0: f32, y1: f32, gap: f32, line: f32) {
        let total: f32 = self.iter().map(|c| c.h).sum();
        let gap = gap.min((y1 - y0 - total) / self.len.saturating_sub(1).max(1) as f32).max(0.0);
        let mut next = y0;
        for c in self.items.iter_mut().take(self.len).flatten() {
            c.top = (c.exit() - line / 2.0).min(y1 - c.h).max(next);
            next = c.top + c.h + gap;
        }
        let mut last = y1 + gap;
        for c in self.items.iter_mut().take(self.len).rev().flatten() {
            c.top = c.top.min(last - gap - c.h);
            last = c.top;
        }
    }
}

/// The edit screen: the controls in two clusters at the sides (see [`super::pad`]), each a target, with
/// a line in to its label (its name, and what it maps to); View, Menu and the gyro in the middle, their
/// labels under them.
fn edit(f: &mut Frame, body: Rect, id: u16) {
    let s = f.s;
    let keymap = s.keymaps().get(id);
    let name = keymap.map_or("", |k| k.name.as_str());
    let area = header(f, body, name, None, "Press or touch an input · Home: back");
    let size = Text::Small.size(body.h) * LABEL;
    let line = size * LINE;
    let cw = (area.w * CLUSTER_W).floor();
    let gap = (area.w * GAP).floor();
    let lw = (area.w * LABEL_COLUMN_W).floor();
    let pad = Pad::new(area, cw);

    let (mut left, mut right) = (Column::new(), Column::new());
    for input in Input::all() {
        let Some(part) = pad.part(input) else {
            continue;
        };
        let mapped = binding(keymap, input).map(|b| Fmt::<48>::of(format_args!("{}", OutputLabel(b.output))));
        let h = line * if mapped.is_some() { 2.0 } else { 1.0 };
        let c = Callout { input, part, mapped, top: 0.0, h };
        if part.side == Side::Left { left.push(c) } else { right.push(c) }
    }
    for column in [&mut left, &mut right] {
        column.place(area.y, area.bottom(), size * 0.55, line);
    }

    // Lines in, from each control to its label.
    for (side, column) in [(Side::Left, &left), (Side::Right, &right)] {
        let cluster = pad.cluster(side);
        let (bus, end) = match side {
            Side::Left => (cluster.right() + gap * 0.3, cluster.right() + gap - size * 0.3),
            Side::Right => (cluster.x - gap * 0.3, cluster.x - gap + size * 0.3),
        };
        for c in column.iter() {
            leader(f, c, bus, end, line);
        }
    }

    // The controls (the clicks last: they sit on the sticks' caps).
    for side in Side::ALL {
        if let Some(well) = pad.well(side) {
            f.c.outline(well, well.w, PART_W, PART_LINE);
        }
    }
    let clicks = |c: &&Callout| matches!(c.input, Input::Button(Button::LS | Button::RS));
    let all = || left.iter().chain(right.iter());
    for c in all().filter(|c| !clicks(c)).chain(all().filter(clicks)) {
        control(f, c, size);
    }

    // The labels.
    for (side, column) in [(Side::Left, &left), (Side::Right, &right)] {
        let cluster = pad.cluster(side);
        let x = if side == Side::Left { cluster.right() + gap } else { cluster.x - gap - lw };
        for c in column.iter() {
            label(f, c.input, c.mapped, Rect::new(x, c.top, lw, c.h), if side == Side::Left { Align::Start } else { Align::End }, size);
        }
    }

    // View, Menu and the gyro, in the middle.
    let strip = Rect::new(area.x + cw + gap + lw, area.y, area.w - 2.0 * (cw + gap + lw), area.h);
    middle(f, strip, keymap, size);
}

/// A line from a control in to its label: across (or up to its lane, then across) to `bus`, then on to
/// `end`, level with the label's first line.
fn leader(f: &mut Frame, c: &Callout, bus: f32, end: f32, line: f32) {
    let (ax, ay) = c.part.anchor;
    let y = c.top + line / 2.0;
    let color = if c.mapped.is_some() { Tone::Fn.rgb().alpha(LEADER_MAPPED) } else { LEADER.into() };
    let across = c.exit();
    if let Some(lane) = c.part.lane {
        f.c.line((ax, ay), (ax, lane), LEADER_W, color);
    }
    f.c.line((ax, across), (bus, across), LEADER_W, color);
    f.c.line((bus, across), (end, y), LEADER_W, color);
}

/// A control's label: its name (dim if as on the pad), and under it what it maps to; a target for it.
fn label(f: &mut Frame, input: Input, mapped: Option<Fmt<48>>, r: Rect, align: Align, size: f32) {
    let name = Fmt::<24>::of(format_args!("{}", Short(input)));
    let line = size * LINE;
    let look = Look { radius: style::RADIUS_S, fill: None, inset: 0.0, ring: -size * 0.3 };
    let at = |w: f32| match align {
        Align::Start => r.x,
        Align::Center => r.x + (r.w - w) / 2.0,
        Align::End => r.right() - w,
    };
    // The highlight and focus ring go round the text.
    let wide = f.c.text_width(size, &name).max(mapped.map_or(0.0, |m| f.c.text_width(size, &m))).min(r.w);
    let shown = Rect::new(at(wide), r.y, wide, r.h);
    f.button_in(Id::Keymap(KeymapId::Input(input)), r, shown, look, |f| {
        let mut row = |text: &str, y: f32, tone: Tone| {
            let w = f.c.text_width(size, text).min(r.w);
            let x = at(w);
            f.c.text_fit(Rect::new(x, y, w + 1.0, line), size, tone, text, Keep::Start);
        };
        row(&name, r.y, if mapped.is_some() { Tone::Normal } else { Tone::Dim });
        if let Some(m) = mapped {
            row(&m, r.y + line, Tone::Fn);
        }
    });
}

/// View, Menu and the gyro, one under another in the middle of `strip`, each with its label under it;
/// the whole of each is its target.
fn middle(f: &mut Frame, strip: Rect, keymap: Option<&Keymap>, size: f32) {
    let line = size * LINE;
    let pill_w = (strip.w * 0.4).min(size * 2.6);
    let pill_h = pill_w * 0.45;
    let cell_h = pill_h * 1.6 + line * 2.0 + size * 0.6;
    let gap = size;
    let total = cell_h * 3.0 + gap * 2.0;
    let mut y = strip.y + (strip.h - total) / 2.0;
    for input in [Input::Button(Button::View), Input::Button(Button::Menu), Input::Gyro] {
        let cell = Rect::new(strip.x, y, strip.w, cell_h);
        y += cell_h + gap;
        let mapped = binding(keymap, input).map(|b| Fmt::<48>::of(format_args!("{}", OutputLabel(b.output))));
        let tone = if mapped.is_some() { Tone::Fn } else { Tone::Dim };
        let (cx, cy) = (cell.center().0, cell.y + pill_h * 0.8);
        let shown = if input == Input::Gyro {
            Rect::new(cx - pill_h * 0.8, cy - pill_h * 0.8, pill_h * 1.6, pill_h * 1.6)
        } else {
            Rect::new(cx - pill_w / 2.0, cy - pill_h / 2.0, pill_w, pill_h)
        };
        let look = Look { radius: shown.h / 2.0, fill: None, inset: 0.0, ring: -3.0 };
        let name = Fmt::<24>::of(format_args!("{}", Short(input)));
        f.button_in(Id::Keymap(KeymapId::Input(input)), cell, shown, look, |f| {
            f.c.outline(shown, shown.h / 2.0, PART_W, PART_LINE);
            let u = pill_h * 0.28;
            match input {
                Input::Button(Button::View) => {
                    f.c.outline(Rect::new(cx - u * 1.1, cy - u * 0.9, u * 1.4, u * 1.1), 1.0, 1.0, tone);
                    f.c.outline(Rect::new(cx - u * 0.3, cy - u * 0.2, u * 1.4, u * 1.1), 1.0, 1.0, tone);
                }
                Input::Button(_) => {
                    for k in [-1.0, 0.0, 1.0] {
                        f.c.line((cx - u, cy + k * u * 0.7), (cx + u, cy + k * u * 0.7), 1.0, tone);
                    }
                }
                _ => {
                    // The gyro: a ring round a dot.
                    f.c.outline(shown.centered(shown.w * 0.5, shown.h * 0.5), shown.w, 1.0, tone);
                    f.c.circle(cx, cy, shown.w * 0.08, tone);
                }
            }
            let text_y = cell.y + pill_h * 1.6;
            let w = f.c.text_width(size, &name).min(cell.w);
            f.c.text_fit(Rect::new(cx - w / 2.0, text_y, w + 1.0, line), size, if mapped.is_some() { Tone::Normal } else { Tone::Dim }, &name, Keep::Start);
            if let Some(m) = mapped {
                let w = f.c.text_width(size, &m).min(cell.w);
                f.c.text_fit(Rect::new(cx - w / 2.0, text_y + line, w + 1.0, line), size, Tone::Fn, &m, Keep::Start);
            }
        });
    }
}

/// A control in a cluster: its outline and its own mark, in blue if it is mapped; a target.
fn control(f: &mut Frame, c: &Callout, size: f32) {
    let p = c.part;
    let tone = if c.mapped.is_some() { Tone::Fn } else { Tone::Dim };
    let look = Look { radius: p.radius, fill: None, inset: 0.0, ring: -3.0 };
    let shown = p.shown;
    f.button_in(Id::Keymap(KeymapId::Input(c.input)), p.target, shown, look, |f| {
        let (cx, cy) = shown.center();
        let name = |f: &mut Frame, t: &str, r: Rect, size: f32| {
            f.c.text_in(r, size, tone, t, Align::Center);
        };
        match c.input {
            Input::Trigger(s) => {
                // Round on top, flat at the bottom.
                f.c.clip(Some(Rect::new(shown.x - PART_W, shown.y - PART_W, shown.w + 2.0 * PART_W, shown.h + PART_W / 2.0)));
                f.c.outline(Rect::new(shown.x, shown.y, shown.w, shown.h + shown.w), shown.w / 2.0, PART_W, PART_LINE);
                f.c.clip(None);
                f.c.line((shown.x, shown.bottom()), (shown.right(), shown.bottom()), PART_W, PART_LINE);
                let r = Rect::new(shown.x, shown.bottom() - shown.h * 0.55, shown.w, shown.h * 0.4);
                name(f, if s == Side::Left { "LT" } else { "RT" }, r, size.min(r.h * 0.7));
            }
            Input::Button(b @ (Button::LB | Button::RB | Button::L4 | Button::L5 | Button::R4 | Button::R5)) => {
                f.c.outline(shown, shown.h / 2.0, PART_W, PART_LINE);
                name(f, b.label(), shown, size.min(shown.h * 0.6));
            }
            Input::Button(b @ (Button::A | Button::B | Button::X | Button::Y)) => {
                f.c.outline(shown, shown.w, PART_W, PART_LINE);
                name(f, b.label(), shown, shown.h * 0.48);
            }
            Input::Button(b @ (Button::LS | Button::RS)) => {
                f.c.outline(shown, shown.w, PART_W, PART_LINE);
                name(f, b.label(), shown.inset(-shown.w), shown.h * 0.4);
            }
            Input::Stick(_) => f.c.outline(shown, shown.w, PART_W, PART_LINE),
            Input::Dpad(d) | Input::StickDir(_, d) => {
                if matches!(c.input, Input::Dpad(_)) {
                    f.c.outline(shown, p.radius, PART_W, PART_LINE);
                }
                let icon = match d {
                    Dir::Up => Icon::ArrowUp,
                    Dir::Down => Icon::ArrowDown,
                    Dir::Left => Icon::ArrowLeft,
                    Dir::Right => Icon::ArrowRight,
                };
                let side = if matches!(c.input, Input::Dpad(_)) { shown.w.min(shown.h) * 0.55 } else { shown.w * 0.8 };
                f.c.icon(icon, Rect::new(cx, cy, 0.0, 0.0).inset(-side / 2.0), tone);
            }
            _ => {}
        }
    });
}

/// An input's name on screen.
struct Label(Input);

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Input::Dpad(d) => f.write_str(d.label()),
            input => input.label(f),
        }
    }
}

/// An input's name in a label beside its cluster, where the side goes without saying.
struct Short(Input);

impl std::fmt::Display for Short {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Input::Stick(_) => f.write_str("Stick"),
            Input::StickDir(_, d) => write!(f, "Stick {}", d.name()),
            Input::Button(Button::LS | Button::RS) => f.write_str("Stick click"),
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

/// The "map to" screen: the targets that fit the input, as rows of chips; the chosen one lit.
fn map_to(f: &mut Frame, body: Rect, id: u16, input: Input) {
    let s = f.s;
    let b = binding(s.keymaps().get(id), input);
    let out = b.map(|b| b.output);
    let title = Fmt::<64>::of(format_args!("{} to", Label(input)));
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
            if out == Some(Output::Pointer) {
                speeds(f, &mut flow, b);
            }
        }
        Kind::Gyro => {
            flow.section(f, "Motion");
            flow.chip(f, target(Target::Pointer), "Mouse", out == Some(Output::Pointer));
            for side in Side::ALL {
                let label = if side == Side::Left { "Left stick" } else { "Right stick" };
                flow.chip(f, target(Target::Stick(side)), label, out == Some(Output::Stick(side)));
            }
            if b.is_some_and(|b| b.output != Output::Nothing) {
                speeds(f, &mut flow, b);
                flow.section(f, "Active");
                let hold = b.and_then(|b| b.hold);
                flow.chip(f, Id::Keymap(KeymapId::Hold(None)), "Always", hold.is_none());
                for h in gyro_holds() {
                    let label = Fmt::<16>::of(format_args!("{}", Label(h)));
                    flow.chip(f, Id::Keymap(KeymapId::Hold(Some(h))), &label, hold == Some(h));
                }
            }
        }
        Kind::Trigger => {
            flow.section(f, "Trigger");
            for side in Side::ALL {
                flow.chip(f, target(Target::Trigger(side)), if side == Side::Left { "LT" } else { "RT" }, out == Some(Output::Trigger(side)));
            }
            digital_targets(f, &mut flow, out);
            if out.is_some_and(|o| o.digital()) {
                flow.section(f, "Past");
                for t in THRESHOLDS {
                    let label = Fmt::<8>::of(format_args!("{t} %"));
                    flow.chip(f, Id::Keymap(KeymapId::Threshold(t)), &label, b.is_some_and(|b| b.threshold == t));
                }
            }
        }
        Kind::Digital => digital_targets(f, &mut flow, out),
    }
    flow.section(f, "");
    flow.chip(f, target(Target::Default), if kind == Kind::Gyro { "Off" } else { "As on the pad" }, out.is_none());
    if kind != Kind::Gyro {
        flow.chip(f, target(Target::Nothing), "Nothing", out == Some(Output::Nothing));
    }
}

/// The speed steps of motion to the pointer or a stick.
fn speeds(f: &mut Frame, flow: &mut Flow, b: Option<Binding>) {
    flow.section(f, "Speed");
    for n in SPEEDS {
        let label = Fmt::<4>::of(format_args!("{n}"));
        flow.chip(f, Id::Keymap(KeymapId::Speed(n)), &label, b.is_some_and(|b| b.speed == n));
    }
}

/// What a button may map to: the pad's buttons (or one pressed on the pad), a key chord, the mouse's
/// buttons, the wheel.
fn digital_targets(f: &mut Frame, flow: &mut Flow, out: Option<Output>) {
    flow.section(f, "Pad");
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
