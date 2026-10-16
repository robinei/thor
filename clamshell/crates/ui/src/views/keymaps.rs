//! The keymaps' screens, over the settings tab (see `core`'s `keymap_ui`): the picker (a scrolled list:
//! Default, then the keymaps, each with who uses it and an Edit button; New), a keymap's menu, deleting
//! one in use, the edit screen (one layer of the keymap, named at the top right, a button to the list of
//! layers; Close at the top left; the pad's controls drawn in two clusters at the sides, as they sit
//! beside the screen, each the layer changes with a label; between the label columns, the whole height, a
//! panel for the selected input: the ways to remap it as icons, its parameters), the layers' lists and a
//! layer's menu, the same while a control is picked on it, what holds the gyro on, a part of the mouse
//! on its drawing, and a key chord typed on the keyboard.
//!
//! On black like the rest: the pad's parts are thin dim outlines; blue is what a keymap maps to something
//! else, grey what it only tunes.

use std::fmt::Write;

use clamshell_core::keymap::{Actuation, Binding, Button, Change, Chord, Dir, Input, Keymap, Kind, Layer, MouseButton, Output, PadOut, Param, Side, change, params_of};
use clamshell_core::keymap_ui::{EditView, Screen, holds, picked, way_of, ways};
use clamshell_core::slider::{Control, Slider};
use clamshell_core::{Id, KeymapId, ListId, RowPart, Scope, Target};

use super::format::Fmt;
use super::scroll::Scrolled;
use super::pad::{Pad, Part};
use super::style::{self, Look, Text, Tone};
use super::widgets::{EditCell, HeaderRight, ListRow};
use super::{Frame, Track, editor, widgets};
use crate::draw::{Align, Column, Keep, LINE, Rect, Rgb, Span};
use crate::icons::Icon;

/// A list row's height and a button's, of the area's height.
const ROW_H: f32 = 0.1;
const BUTTON_H: f32 = 0.085;

pub(super) fn draw(f: &mut Frame, body: Rect, screen: Screen) {
    match screen {
        Screen::List => list(f, body),
        Screen::Menu(id) => menu(f, body, id),
        Screen::Delete(id) => delete(f, body, id),
        Screen::Edit(id) => edit(f, body, id, None),
        Screen::Pick { id, input } => edit(f, body, id, Some(Picking::Target(input))),
        Screen::Holds { id, input } => edit(f, body, id, Some(Picking::Hold(input))),
        Screen::Mouse { id, input } => mouse(f, body, id, input),
        Screen::Layers { id, input } => layers(f, body, id, input),
        Screen::LayerMenu { id, layer } => layer_menu(f, body, id, layer),
        Screen::LayerPicker(id) => layer_picker(f, body, id),
        Screen::Capture { input, keys, .. } => capture(f, body, input, keys),
    }
}

/// The keymap screens' header (see [`widgets::header`]): Back, the title, and on the right `right`.
fn header(f: &mut Frame, body: Rect, title: &str, right: HeaderRight) -> Rect {
    widgets::header(f, body, Some(Id::Keymap(KeymapId::Back)), title, false, right)
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
    widgets::header(f, body, None, title, true, HeaderRight::Action(Id::Keymap(KeymapId::Back), "Cancel"))
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
    let area = header(f, body, &title, HeaderRight::Action(Id::Keymap(KeymapId::New), "New"));
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
        // (A keymap that is gone counts as Default.)
        let mine = id == chosen || (id == 0 && s.keymaps().get(chosen).is_none());
        let who = if row.is_some() { users(f, id) } else { Fmt::of(format_args!("As on the pad")) };
        let at = |part| Id::Row { list: ListId::Keymaps, i: i as u16, part };
        let item = ListRow { label: row.map_or("Default", |k| k.name.as_str()), tone: Tone::high_if(mine), mark: mine, note: &who, edit: if row.is_some() { EditCell::Edit(at(RowPart::Edit)) } else { EditCell::Empty } };
        widgets::list_row(f, at(RowPart::Main), rows.row(i), pad, size, item);
    }
    rows.end(f, area.x + area.w + pad * 0.6);
    f.c.text_fit(Rect::new(area.x, area.y + area.h - hint * LINE, area.w, hint * LINE), hint, Tone::Dim, "Long press a keymap to rename, duplicate or delete it", Keep::Start);
}


fn menu(f: &mut Frame, body: Rect, id: u16) {
    let name = f.s.keymap_name(id);
    let area = header(f, body, name, HeaderRight::None);
    let k = |id| Id::Keymap(id);
    let items =
        [(k(KeymapId::Edit), "Edit", Tone::Fn), (k(KeymapId::Rename), "Rename", Tone::Fn), (k(KeymapId::Duplicate), "Duplicate", Tone::Fn), (k(KeymapId::Delete), "Delete", Tone::Accent)];
    widgets::menu(f, area, body, &items);
}

fn delete(f: &mut Frame, body: Rect, id: u16) {
    let name = f.s.keymap_name(id);
    let title = Fmt::<128>::of(format_args!("Delete {name}?"));
    let area = header(f, body, &title, HeaderRight::None);
    let who = users(f, id);
    let size = Text::Body.size(body.h);
    let text = Fmt::<160>::of(format_args!("In use ({}): deleting it puts them back on Default.", who.as_str()));
    let used = f.c.wrap(Span { x: area.x, y: area.y, w: area.w }, size, Tone::Normal, &text, 3);
    let pad = style::pad(body.h);
    let rest = Rect::new(area.x, area.y + used + pad, area.w, area.h - used - pad);
    widgets::menu(f, rest, body, &[(Id::Keymap(KeymapId::Back), "Keep it", Tone::Fn), (Id::Keymap(KeymapId::ConfirmDelete), "Delete", Tone::Accent)]);
}

/// An input's binding in keymap `k`'s `layer` itself, if it has one there.
/// A label's first line: its actuation, text and tone.
type FirstLine = (Actuation, Fmt<48>, Tone);

/// An input's label in a layer: its press's line (unless as on the pad in Base), then its hold's and
/// double's (the first of those its first line without a press).
fn callout_lines(keymap: Option<&Keymap>, layer: Layer, input: Input) -> (Option<FirstLine>, [Option<(Actuation, Fmt<48>)>; 2]) {
    let press = binding(keymap, layer, input).filter(|b| layer != Layer::BASE || change(input, Some(b)) != Change::None);
    let mut lines = press.map(|b| {
        let (text, tone) = described(input, &b, keymap);
        (Actuation::Press, text, tone)
    });
    let mut extra = [None, None];
    for (slot, act) in extra.iter_mut().zip(Actuation::of(input.kind()).filter(|a| *a != Actuation::Press)) {
        let Some(b) = keymap.and_then(|k| k.get_of(layer, input, act)) else {
            continue;
        };
        match lines {
            None => lines = Some((act, summary(input, b, keymap), Tone::Fn)),
            Some(_) => *slot = Some((act, summary(input, b, keymap))),
        }
    }
    (lines, extra)
}

fn binding(k: Option<&Keymap>, layer: Layer, input: Input) -> Option<Binding> {
    k.and_then(|k| k.get_in(layer, input)).copied()
}

/// The edit screen: the labels' text, of the small text size; of the area's width, a cluster's width,
/// the room between it and its labels (where the lines turn), and the labels' column's width; the
/// bottom row's height and the panel's rows', of the area's height.
const LABEL: f32 = 0.66;
const CLUSTER_W: f32 = 0.222;
const GAP: f32 = 0.016;
const LABEL_COLUMN_W: f32 = 0.115;
const PANEL_ROW_H: f32 = 0.075;
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
fn mapping(b: Binding, keymap: Option<&Keymap>) -> Fmt<48> {
    match b.output {
        // (A layer by its name in the keymap.)
        Output::Layer(l) => Fmt::of(format_args!("Layer: {}", keymap.and_then(|k| k.layer_name(l)).unwrap_or("?"))),
        out => Fmt::of(format_args!("{}", OutputLabel(out))),
    }
}

/// What an input's binding in a layer says, and its tone: blue where it maps the input to something
/// else, grey where it is the input as on the pad (tuned, or said so in the Back layer).
fn described(input: Input, b: &Binding, keymap: Option<&Keymap>) -> (Fmt<48>, Tone) {
    match change(input, Some(b)) {
        Change::Mapped => (summary(input, b, keymap), Tone::Fn),
        _ if input == Input::Gyro => (Fmt::of(format_args!("Off")), Tone::Normal),
        Change::Tuned => (summary(input, b, keymap), Tone::Normal),
        Change::None => (Fmt::of(format_args!("As on pad")), Tone::Normal),
    }
}

/// A binding in a few words: what it maps to, with its turbo and toggle (`Ctrl+X · 12/s`); one that only
/// repeats or toggles the input as on the pad says just that (`Turbo 12/s`), one only tuned otherwise
/// `Tuned`.
fn summary(input: Input, b: &Binding, keymap: Option<&Keymap>) -> Fmt<48> {
    let mut text = Fmt::<48>::new();
    let mapped = Some(b.output) != clamshell_core::keymap::as_on_the_pad(input);
    match (mapped, b.turbo, b.toggle) {
        (true, ..) => {
            let _ = write!(text, "{}", mapping(*b, keymap).as_str());
            if b.turbo > 0 {
                let _ = write!(text, " · {}/s", b.turbo);
            }
            if b.toggle {
                let _ = write!(text, " · toggle");
            }
        }
        (false, 0, false) => {
            let _ = write!(text, "Tuned");
        }
        (false, t, toggle) => {
            if t > 0 {
                let _ = write!(text, "Turbo {t}/s");
            }
            if toggle {
                let _ = write!(text, "{}Toggle", if t > 0 { " · " } else { "" });
            }
        }
    }
    text
}

/// A changed control's label beside its cluster: the control, its text and tone, and the label's top
/// once placed.
#[derive(Clone, Copy)]
struct Callout {
    part: Part,
    /// Its first line (a press's, else a hold's or a double's), and its tone.
    act: Actuation,
    text: Fmt<48>,
    tone: Tone,
    /// A hold's and a double's lines below it, smaller.
    extra: [Option<(Actuation, Fmt<48>)>; 2],
    top: f32,
    /// Its extra lines left out for room (the first says how many with "+N").
    collapsed: bool,
    /// Base's, shown faded in another layer that doesn't map the input.
    faded: bool,
}

impl Callout {
    /// The height its line reaches the labels' side at.
    fn exit(&self) -> f32 {
        self.part.lane.unwrap_or(self.part.anchor.1)
    }

    fn extras(&self) -> usize {
        self.extra.iter().flatten().count()
    }

    /// Its height: the first line's, and the extra lines' unless collapsed.
    fn height(&self, line: f32) -> f32 {
        line + if self.collapsed { 0.0 } else { self.extras() as f32 * line * EXTRA }
    }
}

/// An extra line of a label (a hold's, a double's), of a line's height and text size.
const EXTRA: f32 = 0.85;

/// The word before a hold's or a double's line on a label.
fn act_word(act: Actuation) -> &'static str {
    match act {
        Actuation::Press => "",
        Actuation::Hold => "hold",
        Actuation::Double => "2×",
        Actuation::FullPull => "full",
    }
}

/// One cluster's labels, in the order their lines arrive in (top to bottom).
struct Labels {
    items: [Option<Callout>; CONTROLS],
    len: usize,
}

impl Labels {
    fn new() -> Labels {
        Labels { items: [None; CONTROLS], len: 0 }
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

    /// The labels' heights together, with `gap` between them.
    fn total(&self, line: f32, gap: f32) -> f32 {
        self.iter().map(|c| c.height(line)).sum::<f32>() + gap * self.len.saturating_sub(1) as f32
    }

    /// Places the labels (their first lines `line` high) level with their lines where they can be,
    /// within `y0`..`y1`, at least `gap` apart (less if they don't fit; their extra lines left out first).
    fn place(&mut self, y0: f32, y1: f32, line: f32, gap: f32) {
        if self.total(line, gap) > y1 - y0 {
            for c in self.items.iter_mut().take(self.len).flatten() {
                c.collapsed = c.extras() > 0;
            }
        }
        let heights: f32 = self.iter().map(|c| c.height(line)).sum();
        let gap = gap.min((y1 - y0 - heights) / self.len.saturating_sub(1).max(1) as f32).max(0.0);
        let mut next = y0;
        for c in self.items.iter_mut().take(self.len).flatten() {
            let h = c.height(line);
            c.top = (c.exit() - line / 2.0).min(y1 - h).max(next);
            next = c.top + h + gap;
        }
        let mut last = y1 + gap;
        for c in self.items.iter_mut().take(self.len).rev().flatten() {
            c.top = c.top.min(last - gap - c.height(line));
            last = c.top;
        }
    }
}

/// One line of a label at `y`: a dim word first (`hold`, `2×`; none for a press), the text in `tone`
/// (cut to `lw`), and a dim note after it (`+2`); placed by `at` from its width (from the left or right).
#[allow(clippy::too_many_arguments)]
fn label_line(f: &mut Frame, at: impl Fn(f32) -> f32, y: f32, size: f32, lw: f32, word: &str, text: &str, tone: Tone, note: &str) {
    let space = size * 0.3;
    let word_w = if word.is_empty() { 0.0 } else { f.c.text_width(size, word) + space };
    let note_w = if note.is_empty() { 0.0 } else { f.c.text_width(size, note) };
    let text_w = f.c.text_width(size, text).min((lw - word_w - note_w).max(size));
    let x = at(word_w + text_w + note_w);
    let h = size * LINE;
    if !word.is_empty() {
        f.c.text_in(Rect::new(x, y, word_w, h), size, Tone::Dim, word, Align::Start);
    }
    f.c.text_fit(Rect::new(x + word_w, y, text_w + 1.0, h), size, tone, text, Keep::Start);
    if !note.is_empty() {
        f.c.text_in(Rect::new(x + word_w + text_w, y, note_w + 1.0, h), size, Tone::Dim, note, Align::Start);
    }
}

/// Where the edit screen's parts go in `body`: the clusters' area (below the corners), Close's corner (top
/// left) and the layer button's (top right), a cluster's width, the room between it and its labels, the
/// labels' column's width, and the strip between the label columns, the whole height (the panel's).
pub(super) struct EditLayout {
    pub area: Rect,
    pub close: Rect,
    pub layer: Rect,
    pub cw: f32,
    pub gap: f32,
    pub lw: f32,
    pub strip: Rect,
}

/// The corners' height (Close, the layer button), of the area's height.
const CORNER_H: f32 = 0.075;

/// The edit screen's margin at the sides, of the usual one (the drawing wants the room).
const EDIT_SIDE: f32 = 0.35;

pub(super) fn edit_layout(body: Rect) -> EditLayout {
    let pad = style::pad(body.h);
    let side = pad * EDIT_SIDE;
    let full = Rect::new(body.x + side, body.y, body.w - 2.0 * side, body.h - pad);
    let cw = (full.w * CLUSTER_W).floor();
    let gap = (full.w * GAP).floor();
    let lw = (full.w * LABEL_COLUMN_W).floor();
    let band = body.h * CORNER_H;
    let mut area = full;
    area.take_top(band);
    let strip = Rect::new(full.x + cw + gap + lw, full.y, full.w - 2.0 * (cw + gap + lw), full.h);
    let close = Rect::new(full.x, full.y, cw, band);
    let layer = Rect::new(full.right() - cw, full.y, cw, band);
    EditLayout { area, close, layer, cw, gap, lw, strip }
}

/// The edit screen's corners: Close and Undo at the top left, the layer shown at the top right, a button
/// to the list of layers (all faded while a control is picked).
fn edit_corners(f: &mut Frame, body: Rect, l: &EditLayout, keymap: Option<&Keymap>, layer: Layer, picking: bool) {
    let size = Text::Small.size(body.h);
    let name = keymap.and_then(|k| k.layer_name(layer)).unwrap_or("Base");
    let r = l.layer;
    let arrow = size * 0.7;
    let layer_button = |f: &mut Frame, tone: Tone| {
        let w = f.c.text_width(size, name).min(r.w - arrow * 1.6);
        let x = r.right() - arrow - size * 0.3 - w;
        f.c.text_fit(Rect::new(x, r.y, w + 1.0, r.h), size, tone, name, Keep::Start);
        f.c.icon(Icon::ArrowDown, Rect::new(r.right() - arrow, r.y + (r.h - arrow) / 2.0, arrow, arrow), tone);
    };
    // Close, and Undo beside it (faded with nothing to undo).
    let close_w = f.c.text_width(size, "Close") + size * 1.2;
    let close = Rect::new(l.close.x, l.close.y, close_w, l.close.h);
    let undo = Rect::new(close.right(), l.close.y, (l.close.w - close_w).max(1.0), l.close.h);
    // (Picking: the panel has Cancel; these wait, faded.)
    if picking {
        f.c.text_in(close, size, Tone::Dim, "Close", Align::Start);
        f.c.text_in(undo, size, Tone::Dim, "Undo", Align::Start);
        layer_button(f, Tone::Dim);
        return;
    }
    f.text_button(Id::Keymap(KeymapId::Back), close, size, Tone::Fn, "Close", Align::Start);
    if f.s.can_undo() {
        f.text_button(Id::Keymap(KeymapId::Undo), undo, size, Tone::Fn, "Undo", Align::Start);
    } else {
        f.c.text_in(undo, size, Tone::Dim, "Undo", Align::Start);
    }
    // The layer shown: Base in blue as any button; another in white, so it is plain which one is edited.
    let id = Id::Keymap(KeymapId::LayerPicker);
    let tone = if f.pressed(&id) || layer != Layer::BASE { Tone::High } else { Tone::Fn };
    f.button(id, r, Look::TEXT, |f| layer_button(f, tone));
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
    let layout = edit_layout(body);
    edit_corners(f, body, &layout, keymap, layer, picking.is_some());
    let EditLayout { area, cw, gap, lw, strip, .. } = layout;
    let size = Text::Small.size(body.h) * LABEL;
    let line = size * LINE;
    let pad = Pad::new(area, cw);

    let (mut left, mut right) = (Labels::new(), Labels::new());
    for input in Input::all() {
        let Some(part) = pad.part(input) else {
            continue;
        };
        // While a control is picked, those that can't be have no label.
        if picking.is_some_and(|p| !p.takes(input)) {
            continue;
        }
        // The layer's own; in another layer than Base, an input it doesn't map: Base's, faded.
        let (mut lines, mut extra) = callout_lines(keymap, layer, input);
        let mut faded = false;
        if lines.is_none() && layer != Layer::BASE {
            (lines, extra) = callout_lines(keymap, Layer::BASE, input);
            faded = true;
        }
        let Some((act, text, tone)) = lines else {
            continue;
        };
        let tone = if faded { Tone::Dim } else { tone };
        let c = Callout { part, act, text, tone, extra, top: 0.0, collapsed: false, faded };
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
            let extra = Actuation::of(input.kind()).filter(|a| *a != Actuation::Press).any(|a| keymap.is_some_and(|k| k.get_of(layer, input, a).is_some()));
            let tone = match binding(keymap, layer, input) {
                _ if extra => Tone::Fn,
                Some(b) => described(input, &b, keymap).1,
                None => Tone::Dim,
            };
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
            let more = if c.collapsed { Fmt::<8>::of(format_args!(" +{}", c.extras())) } else { Fmt::new() };
            let at = |w: f32| if side == Side::Left { cluster.right() + gap } else { cluster.x - gap - w };
            label_line(f, at, c.top, size, lw, act_word(c.act), &c.text, c.tone, &more);
            if c.collapsed {
                continue;
            }
            let mut y = c.top + line;
            for (act, text) in c.extra.iter().flatten() {
                label_line(f, at, y, size * EXTRA, lw, act_word(*act), text, if c.faded { Tone::Dim } else { Tone::Fn }, "");
                y += line * EXTRA;
            }
        }
    }

    // Home (ClamShell's, drawn dim), at the screen's bottom left corner.
    let fixed = if picking.is_some() { MUTED } else { FIXED };
    if let Some(r) = pad.foot(Side::Left) {
        f.c.outline(r, r.w, PART_W, fixed);
        let w = f.c.text_width(size, "Home");
        f.c.text_in(Rect::new(r.x - r.w * 0.6 - w, r.y, w, r.h), size, fixed, "Home", Align::Start);
    }

    // The panel: the whole strip between the label columns.
    let margin = (strip.w * 0.04).min(size);
    let panel_r = Rect::new(strip.x + margin, strip.y, (strip.w - 2.0 * margin).max(1.0), strip.h);
    match picking {
        Some(p) => picking_panel(f, body, panel_r, id, p),
        None => panel(f, body, panel_r, id, keymap, view),
    }
}

/// The panel while a control is picked: what for, what to do, and Cancel (for the gyro's hold, Always
/// beside it).
fn picking_panel(f: &mut Frame, body: Rect, r: Rect, id: u16, picking: Picking) {
    let small = Text::Small.size(body.h) * PANEL_TEXT;
    let heading = Text::Heading.size(body.h) * 0.9;
    let mut col = Column::new(r);
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
    if let Some(row) = col.next(heading * LINE * lines) {
        widgets::centred_lines(f, row, heading, Tone::High, &title);
    }
    if let Some(row) = col.next(small * LINE * 2.2) {
        widgets::centred_lines(f, row, small, Tone::Normal, what);
    }
    col.skip(style::pad(body.h) * 0.3);
    let Some(row) = col.next(body.h * BUTTON_H * 0.65) else {
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
        Target::Layers => Icon::Layers,
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
    let mut col = Column::new(r);
    let Some(input) = view.selected else {
        let y = r.y + r.h * 0.4;
        for (i, t) in ["Press or touch an input", "to see what it does"].into_iter().enumerate() {
            widgets::centred_text(f, Rect::new(r.x, y + i as f32 * small * LINE, r.w, small * LINE), small, Tone::Dim, t);
        }
        // In another layer than Base: which, and what the faded labels are.
        if view.layer != Layer::BASE {
            let name = keymap.and_then(|k| k.layer_name(view.layer)).unwrap_or("");
            let y = y + small * LINE * 3.0;
            widgets::centred_text(f, Rect::new(r.x, y, r.w, small * LINE), small, Tone::High, name);
            widgets::centred_text(f, Rect::new(r.x, y + small * LINE, r.w, small * LINE), small * 0.85, Tone::Dim, "Faded: as in Base");
        }
        return;
    };
    let layer = view.layer;
    let act = view.act;
    let own = keymap.and_then(|k| k.get_of(layer, input, act)).copied();
    // The name.
    let name = Fmt::<32>::of(format_args!("{}", Label(input)));
    let heading = Text::Heading.size(body.h) * 0.9;
    if let Some(row) = col.next(heading * LINE) {
        widgets::centred_text(f, row, heading, Tone::High, &name);
    }
    // Press, Hold, Double (a button's, a direction's), Pull and Full pull (a trigger's): the one shown in
    // white; a dot under one set.
    let tabs = Actuation::of(input.kind()).count();
    if tabs > 1
        && let Some(row) = col.next(small * LINE * 2.2)
    {
        let w = row.w / tabs as f32;
        for (i, a) in Actuation::of(input.kind()).enumerate() {
            let tab = Rect::new(row.x + i as f32 * w, row.y, w, row.h * 0.85);
            let tone = if a == act { Tone::High } else { Tone::Fn };
            f.text_button(Id::Keymap(KeymapId::Actuation(a)), tab, small, tone, a.label_for(input.kind()), Align::Center);
            if keymap.is_some_and(|k| k.get_of(layer, input, a).is_some()) {
                f.c.circle(tab.center().0, row.bottom() - small * 0.25, small * 0.1, tone);
            }
        }
    }
    // What it does: its binding of the actuation shown; in the Back layer without one, the main layer's.
    let unset = |input: Input| match act {
        Actuation::Press => as_on_pad(input),
        Actuation::Hold => Fmt::of(format_args!("No hold")),
        Actuation::Double => Fmt::of(format_args!("No double")),
        Actuation::FullPull => Fmt::of(format_args!("No full pull")),
    };
    let main = || keymap.and_then(|k| k.get_of(Layer::BASE, input, act)).map(|b| summary(input, b, keymap)).unwrap_or_else(|| unset(input));
    if let Some(row) = col.next(small * LINE * 2.2) {
        match (own, layer) {
            (None, l) if l != Layer::BASE => {
                let line = small * LINE;
                let top = row.y + (row.h - 2.0 * line) / 2.0;
                widgets::centred_text(f, Rect::new(row.x, top, row.w, line), small, Tone::Dim, "As in Base");
                widgets::centred_text(f, Rect::new(row.x, top + line, row.w, line), small, Tone::Dim, &main());
            }
            (None, _) => widgets::centred_lines(f, row, small, Tone::Dim, &unset(input)),
            // (The panel's own content in white: blue is for labels of what is remapped, and actions.)
            (Some(b), _) => {
                let tone = if input == Input::Gyro || change(input, Some(&b)) == Change::Mapped || act != Actuation::Press { Tone::High } else { described(input, &b, keymap).1 };
                widgets::centred_lines(f, row, small, tone, &summary(input, &b, keymap));
            }
        }
    }
    // A press that waits (the input has a hold or a double).
    if act == Actuation::Press
        && input.kind() == Kind::Digital
        && s.press_waits(id, input)
        && let Some(row) = col.next(small * LINE)
    {
        widgets::centred_text(f, row, small * 0.85, Tone::Dim, "A short press acts on letting go");
    }
    // The ways to remap it.
    let bh = body.h * BUTTON_H;
    col.skip(pad * 0.5);
    if let Some(row) = col.next(bh) {
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
    // (Acceleration's speed only with acceleration; the ratchet after the gyro's hold, with one.)
    let shown_params = params_of(input, base.output, act).filter(|p| match p {
        Param::AccelAt => base.accel > 0,
        // (Y shares X's row.)
        Param::Ratchet | Param::InvertY => false,
        _ => true,
    });
    let mut held = None;
    'rows: {
        for p in shown_params {
            let Some(row) = col.next(rh) else {
                break 'rows;
            };
            let v = p.get(&base);
            let changed = v != p.default_for(input, base.output);
            if p == Param::InvertX {
                invert_row(f, row, small, &base);
            } else if p.switch() {
                widgets::switch_row(f, Id::Keymap(KeymapId::Toggle(p)), row, small, p.label(), v > 0);
            } else {
                let value = shown(p, v);
                let slider = view.slider.filter(|sl| sl.control == Control::Param(p));
                // (The screen's width, as the bar's sliders: the track as long, over the drawing.)
                let track = Track { x0: body.x, w: body.w };
                // (A held slider is drawn last, its card over the rows below it.)
                if let Some(sl) = slider {
                    held = Some((row, track, sl, p));
                }
                f.button(Id::Control(Control::Param(p)), row, Look::ROW, |f| {
                    if slider.is_none() {
                        widgets::value_row(f, row, small, p.label(), &value, changed);
                    }
                });
                f.track(track);
            }
        }
        if input == Input::Gyro && matches!(base.output, Output::Pointer | Output::Stick(_)) {
            let Some(row) = col.next(rh) else {
                break 'rows;
            };
            let value = match base.hold {
                Some(h) => Fmt::<24>::of(format_args!("while {}", Label(h))),
                None => Fmt::of(format_args!("Always")),
            };
            f.button(Id::Keymap(KeymapId::Holds), row, Look::ROW, |f| widgets::value_row(f, row, small, "Active", &value, base.hold.is_some()));
            if base.hold.is_some()
                && let Some(row) = col.next(rh)
            {
                widgets::switch_row(f, Id::Keymap(KeymapId::Toggle(Param::Ratchet)), row, small, Param::Ratchet.label(), base.ratchet);
            }
        }

        if params_of(input, base.output, act).any(|p| !p.switch())
            && let Some(row) = col.next(small * LINE * 1.5)
        {
            widgets::centred_text(f, row.shifted(0.0, small * 0.3), small * 0.85, Tone::Dim, "Slide a value to set it");
        }
    }
    if let Some((row, track, sl, p)) = held {
        slider_row(f, row, track, sl, p, r);
    }
}

/// The inversions' row: "Invert", then a switch each for X and Y, each its own target.
fn invert_row(f: &mut Frame, row: Rect, size: f32, b: &Binding) {
    let inner = row.inset(size * 0.4);
    let (mut sw_w, mut letter) = (widgets::switch_w(size), size * 1.2);
    // (Narrow: no word, the two switches scaled into the row.)
    let label = inner.w >= 2.0 * (sw_w + letter + size * 0.6) + size * 3.0;
    if !label {
        let k = (inner.w / (2.0 * (sw_w + letter + size * 0.6))).min(1.0);
        (sw_w, letter) = (sw_w * k, letter * k);
    }
    let mut x = inner.right();
    for (p, name, on) in [(Param::InvertY, "Y", b.invert_y), (Param::InvertX, "X", b.invert_x)] {
        let sw = Rect::new(x - sw_w, row.y, sw_w, row.h);
        let target = Rect::new(sw.x - letter, row.y, sw_w + letter, row.h);
        f.button(Id::Keymap(KeymapId::Toggle(p)), target, Look::ROW, |f| {
            f.c.text_in(Rect::new(target.x, row.y, letter * 0.8, row.h), size, Tone::Dim, name, Align::Start);
            widgets::switch_shape(f.c, sw, on);
        });
        x = target.x - size * 0.6 * if label { 1.0 } else { sw_w / widgets::switch_w(size) };
    }
    if label {
        f.c.text_fit(Rect::new(inner.x, row.y, (x - inner.x).max(0.0), row.h), size, Tone::Dim, "Invert", Keep::Start);
    }
}


/// "As on the pad" in words, for an input (the gyro: off).
fn as_on_pad(input: Input) -> Fmt<48> {
    Fmt::of(format_args!("{}", if input == Input::Gyro { "Off" } else { "As on the pad" }))
}


/// A parameter's value in words (`40 %`, `Off`, `12/s`, `400 ms`).
fn shown(p: Param, v: u8) -> Fmt<16> {
    let mut text = Fmt::<16>::new();
    let _ = p.show(v, &mut text);
    text
}

/// A parameter's row while its slider is up (the bar's slider, within the panel): the name and value
/// above, the track along the row with the thumb under the finger.
fn slider_row(f: &mut Frame, row: Rect, track: Track, sl: Slider, p: Param, panel: Rect) {
    // As the bar's: the track along the screen through the row, the value on a card beside the finger.
    let text = Fmt::<32>::of(format_args!("{} {}", p.label(), shown(p, sl.value).as_str()));
    let within = Rect::new(track.x0, panel.y, track.w, panel.h);
    widgets::held_slider(f, &sl, track.x0, row.y + row.h / 2.0, row.h * 0.75, within, None, &text);
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
            Input::Button(b @ (Button::View | Button::Menu | Button::Back)) => {
                f.c.outline(shown, shown.w, lw, lc);
                // Its name on the outer side.
                let w = f.c.text_width(size, thor(b));
                let x = if p.side == Side::Left { shown.x - shown.w * 0.6 - w } else { shown.right() + shown.w * 0.6 };
                f.c.text_in(Rect::new(x, shown.y, w, shown.h), size, tone, thor(b), Align::Start);
            }
            Input::Stick(_) => f.c.outline(shown, shown.w, lw, lc),
            Input::Gyro => {
                f.c.outline(shown, shown.h / 2.0, lw, lc);
                name(f, "Gyro", shown, size.min(shown.h * 0.6));
            }
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

/// The layers for `input` to turn on: the keymap's, but Base (the one it maps to now marked), each with
/// what turns it on, and New layer… at the end (named, then mapped to). Chosen, it closes; Cancel gives
/// up.
fn layers(f: &mut Frame, body: Rect, id: u16, input: Input) {
    let now = binding(f.s.keymaps().get(id), f.s.keymap_edit().layer, input).map(|b| b.output);
    let area = choice_header(f, body, &remap_title(input));
    layer_list(f, body, area, id, |l| now == Some(Output::Layer(l)), false);
}

/// A keymap's layers on the scrolled list ([`ListId::Layers`]), the rows the state gives: each its name
/// (`current` marked), what turns it on, its Edit (`edit`), then New layer….
fn layer_list(f: &mut Frame, body: Rect, area: Rect, id: u16, current: impl Fn(Layer) -> bool, edit: bool) {
    let s = f.s;
    let keymap = s.keymaps().get(id);
    let pad = style::pad(body.h);
    let size = Text::Body.size(body.h);
    let view = Rect::new(area.x - pad * 0.5, area.y, area.w + pad, area.h);
    let rows = Scrolled::begin(f, ListId::Layers, view, body.h * ROW_H, s.list_len(ListId::Layers));
    for i in rows.shown() {
        let at = |part| Id::Row { list: ListId::Layers, i: i as u16, part };
        let layer = s.layer_row(i);
        let (name, what, mine) = match layer {
            Some(l) => (keymap.and_then(|k| k.layer_name(l)).unwrap_or(""), keymap.map_or(Fmt::new(), |k| turned_on_by(k, l)), current(l)),
            None => ("New layer…", Fmt::new(), false),
        };
        let tone = if layer.is_some() { Tone::high_if(mine) } else { Tone::Fn };
        let cell = match (edit, layer) {
            (false, _) => EditCell::None,
            (true, Some(_)) => EditCell::Edit(at(RowPart::Edit)),
            (true, None) => EditCell::Empty,
        };
        let item = ListRow { label: name, tone, mark: mine, note: &what, edit: cell };
        widgets::list_row(f, at(RowPart::Main), rows.row(i), pad, size, item);
    }
    rows.end(f, area.x + area.w + pad * 0.6);
}

/// A layer's menu: rename it, delete it (what maps to it too).
fn layer_menu(f: &mut Frame, body: Rect, id: u16, layer: Layer) {
    let name = f.s.keymaps().get(id).and_then(|k| k.layer_name(layer)).unwrap_or("");
    let area = header(f, body, name, HeaderRight::None);
    let k = |id| Id::Keymap(id);
    if f.s.keymap_edit().clearing {
        widgets::menu(f, area, body, &[(k(KeymapId::ConfirmClear), "Clear all its bindings", Tone::Accent), (k(KeymapId::Back), "Cancel", Tone::Fn)]);
    } else if layer == Layer::BASE {
        widgets::menu(f, area, body, &[(k(KeymapId::Clear), "Clear", Tone::Fn)]);
    } else {
        // Once: a switch, and what it means.
        let size = Text::Body.size(body.h);
        let small = Text::Small.size(body.h);
        let mut rest = area;
        let row = rest.take_top(body.h * BUTTON_H);
        let on = f.s.keymaps().get(id).is_some_and(|k| k.layer_once(layer));
        // (Its row reaches into the margin, so its label lines up with the actions below.)
        let inset = size * 0.4;
        let r = Rect::new(row.x - inset, row.y, widgets::menu_w(area, size) + 2.0 * inset, row.h);
        widgets::switch_row(f, k(KeymapId::LayerOnce), r, size, "Once", on);
        let note = rest.take_top(small * LINE * 1.6);
        f.c.text_fit(Rect::new(note.x, note.y, note.w, small * LINE), small, Tone::Dim, "Tapped: on until the next input it maps", Keep::Start);
        rest.take_top(style::pad(body.h) * 0.5);
        widgets::menu(f, rest, body, &[(k(KeymapId::RenameLayer), "Rename", Tone::Fn), (k(KeymapId::Clear), "Clear", Tone::Fn), (k(KeymapId::DeleteLayer), "Delete", Tone::Accent)]);
    }
}

/// The keymap's layers, to show one on the edit screen (the one shown marked), and New layer… at the
/// end; a layer's Edit (or a long press): its menu. Back returns to the edit screen.
fn layer_picker(f: &mut Frame, body: Rect, id: u16) {
    let shown = f.s.keymap_edit().layer;
    let area = choice_header(f, body, "Layers");
    layer_list(f, body, area, id, |l| l == shown, true);
}

/// What turns layer `l` on, in words: each binding to it (`Back`, `LB held`, `L1 twice`, toggles saying
/// so; a Once layer says so first); Base is beneath the others.
fn turned_on_by(k: &Keymap, l: Layer) -> Fmt<96> {
    let mut out = Fmt::new();
    if l == Layer::BASE {
        let _ = write!(out, "Beneath the others");
        return out;
    }
    if k.layer_once(l) {
        let _ = write!(out, "Once · ");
    }
    let mut any = false;
    for (from, _) in k.layers() {
        for act in Actuation::ALL {
            for (input, b) in k.layer_of(from, act) {
                if b.output != Output::Layer(l) {
                    continue;
                }
                let how = match (act, b.toggle) {
                    (_, true) => " toggles",
                    (Actuation::Press, _) => "",
                    (Actuation::Hold, _) => " held",
                    (Actuation::Double, _) => " twice",
                    (Actuation::FullPull, _) => " full",
                };
                let _ = write!(out, "{}{}{how}", if any { ", " } else { "" }, Label(*input));
                any = true;
            }
        }
    }
    if !any {
        let _ = write!(out, "Nothing turns it on");
    }
    out
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
