//! The keymaps' screens, over the settings tab (see `core`'s `keymap_ui`): the picker (a scrolled list:
//! Default, then the keymaps, each with who uses it and an Edit button; New), a keymap's menu, deleting
//! one in use, the edit screen ("map from": a drawing of the pad, each input with what it maps to, in blue
//! where it differs from the pad), what one input maps to ("map to": the targets that fit it, as rows of
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
use super::style::{self, Look, PRESSED, SURFACE, SURFACE_HI, Text, Tone};
use super::{Frame, editor};
use crate::draw::{Align, Keep, LINE, Rect, Span};
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

/// Where each input is drawn on the edit screen: its place in the drawing (fractions of the area).
const BUTTONS: [(Input, f32, f32, f32, f32); 18] = [
    (Input::Trigger(Side::Left), 0.03, 0.0, 0.12, 0.11),
    (Input::Button(Button::LB), 0.17, 0.0, 0.12, 0.11),
    (Input::Button(Button::RB), 0.71, 0.0, 0.12, 0.11),
    (Input::Trigger(Side::Right), 0.85, 0.0, 0.12, 0.11),
    (Input::Button(Button::View), 0.37, 0.2, 0.11, 0.1),
    (Input::Button(Button::Menu), 0.52, 0.2, 0.11, 0.1),
    (Input::Button(Button::Y), 0.775, 0.16, 0.09, 0.11),
    (Input::Button(Button::X), 0.68, 0.29, 0.09, 0.11),
    (Input::Button(Button::B), 0.87, 0.29, 0.09, 0.11),
    (Input::Button(Button::A), 0.775, 0.42, 0.09, 0.11),
    (Input::Button(Button::LS), 0.11, 0.55, 0.12, 0.09),
    (Input::Button(Button::RS), 0.80, 0.7, 0.1, 0.09),
    (Input::Dpad(Dir::Up), 0.29, 0.6, 0.08, 0.1),
    (Input::Dpad(Dir::Left), 0.2, 0.71, 0.08, 0.1),
    (Input::Dpad(Dir::Right), 0.38, 0.71, 0.08, 0.1),
    (Input::Dpad(Dir::Down), 0.29, 0.82, 0.08, 0.1),
    (Input::Gyro, 0.44, 0.86, 0.11, 0.11),
    (Input::Button(Button::L4), 0.02, 0.78, 0.08, 0.09),
];

/// The back buttons other than L4 (whose place is in [`BUTTONS`]).
const BACK: [(Input, f32, f32, f32, f32); 3] =
    [(Input::Button(Button::L5), 0.02, 0.89, 0.08, 0.09), (Input::Button(Button::R4), 0.9, 0.8, 0.08, 0.09), (Input::Button(Button::R5), 0.9, 0.9, 0.08, 0.09)];

/// The sticks' places: the area (fractions) the stick and its direction markers take.
const STICKS: [(Side, f32, f32, f32, f32); 2] = [(Side::Left, 0.06, 0.16, 0.22, 0.36), (Side::Right, 0.55, 0.56, 0.22, 0.38)];

fn place(area: Rect, (x, y, w, h): (f32, f32, f32, f32)) -> Rect {
    Rect::new(area.x + x * area.w, area.y + y * area.h, w * area.w, h * area.h)
}

/// An input's binding in keymap `k`, if it has one.
fn binding(k: Option<&Keymap>, input: Input) -> Option<Binding> {
    k.and_then(|k| k.get(input)).copied()
}

/// The edit screen: the pad drawn, each input a target with what it maps to.
fn edit(f: &mut Frame, body: Rect, id: u16) {
    let s = f.s;
    let keymap = s.keymaps().get(id);
    let name = keymap.map_or("", |k| k.name.as_str());
    let area = header(f, body, name, None, "Press or touch an input · Home: back");
    let size = Text::Small.size(body.h);
    for (input, x, y, w, h) in BUTTONS.into_iter().chain(BACK) {
        let r = place(area, (x, y, w, h));
        input_cell(f, r, input, binding(keymap, input), size);
    }
    for (side, x, y, w, h) in STICKS {
        stick(f, place(area, (x, y, w, h)), side, keymap, size);
    }
}

/// One input: an outline with its name, and below it what it maps to (blue), if not as on the pad.
fn input_cell(f: &mut Frame, r: Rect, input: Input, b: Option<Binding>, size: f32) {
    let label = Fmt::<24>::of(format_args!("{}", Label(input)));
    let mapped = b.map(|b| Fmt::<48>::of(format_args!("{}", OutputLabel(b.output))));
    f.button(Id::Keymap(KeymapId::Input(input)), r, Look::TILE, |f| {
        f.c.outline(r.inset(2.0), style::RADIUS, 1.5, Tone::Dim);
        let (top, bottom) = (Rect::new(r.x, r.y, r.w, r.h * 0.55), Rect::new(r.x + 4.0, r.y + r.h * 0.5, r.w - 8.0, r.h * 0.45));
        let label_size = (r.h * 0.3).min(size * 1.3);
        f.c.text_in(top, label_size, Tone::Normal, &label, Align::Center);
        if let Some(m) = mapped {
            let size = size.min(r.h * 0.25);
            let w = f.c.text_width(size, &m).min(bottom.w);
            f.c.text_fit(Rect::new(bottom.x + (bottom.w - w) / 2.0, bottom.y, w + 1.0, bottom.h), size, Tone::Fn, &m, Keep::Start);
        }
    });
}

/// A stick: the stick as a whole (a circle, its name and mapping inside), and a marker for each
/// direction around it (blue where that direction is mapped).
fn stick(f: &mut Frame, area: Rect, side: Side, keymap: Option<&Keymap>, size: f32) {
    let marker = area.w.min(area.h) * 0.16;
    let radius = (area.w.min(area.h) / 2.0 - marker * 1.2).max(4.0);
    let (cx, cy) = area.center();
    let whole = Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
    let input = Input::Stick(side);
    let label = if side == Side::Left { "Left stick" } else { "Right stick" };
    let mapped = binding(keymap, input).map(|b| Fmt::<48>::of(format_args!("{}", OutputLabel(b.output))));
    f.button(Id::Keymap(KeymapId::Input(input)), whole, Look { radius, ..Look::TILE }, |f| {
        f.c.outline(whole.inset(2.0), radius, 1.5, Tone::Dim);
        let text = (radius * 0.3).min(size * 1.2);
        f.c.text_in(Rect::new(whole.x, cy - text * 1.2, whole.w, text * LINE), text, Tone::Normal, label, Align::Center);
        if let Some(m) = mapped {
            let inner = Rect::new(whole.x + radius * 0.2, cy, whole.w - radius * 0.4, text * LINE);
            let size = size.min(text);
            let w = f.c.text_width(size, &m).min(inner.w);
            f.c.text_fit(Rect::new(inner.x + (inner.w - w) / 2.0, inner.y, w + 1.0, inner.h), size, Tone::Fn, &m, Keep::Start);
        }
    });
    let gap = radius + marker * 0.65;
    for (d, dx, dy, icon) in [(Dir::Up, 0.0, -1.0, Icon::ArrowUp), (Dir::Down, 0.0, 1.0, Icon::ArrowDown), (Dir::Left, -1.0, 0.0, Icon::ArrowLeft), (Dir::Right, 1.0, 0.0, Icon::ArrowRight)] {
        let r = Rect::new(cx + dx * gap - marker / 2.0, cy + dy * gap - marker / 2.0, marker, marker);
        let input = Input::StickDir(side, d);
        let tone = if binding(keymap, input).is_some() { Tone::Fn } else { Tone::Dim };
        f.button(Id::Keymap(KeymapId::Input(input)), r, Look::ICON, |f| f.c.icon(icon, r.square_center(marker * 0.7), tone));
    }
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
