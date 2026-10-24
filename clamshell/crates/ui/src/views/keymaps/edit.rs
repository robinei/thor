//! The keymap edit screen: the Thor's controls drawn in two clusters, with a label and a line in to it
//! for each changed control, the corners (Close, Undo, the layer button), and the layout they share.

use clamshell_state::profiles::picker::{Kind, ProfileId};
use std::fmt::Write;

use clamshell_state::keymap::{Actuation, Binding, Button, Change, Dir, Input, Keymap, Layer, Side, change};
use clamshell_state::{Id, KeymapId};

use crate::gfx::icons::Icon;
use crate::gfx::{Align, Keep, LINE, Rect};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Text, Tone};
use crate::toolkit::{Frame, style};

use super::pad::{Pad, Part};
use super::panel::{panel, picking_panel};
use super::words::{described, printed, summary};
use super::{
    CLUSTER_W, CONTROLS, FIXED, GAP, LABEL, LABEL_COLUMN_W, LABEL_GAP, LABEL_MIN, LEADER_ALPHA, LEADER_W, MUTED, PANEL_TEXT, PART_LINE, PART_W, Picking,
    SELECTED_W,
};

/// A label's first line: its actuation, text and tone.
pub(super) type FirstLine = (Actuation, Fmt<48>, Tone);

/// An input's label in a layer: the line for its press, unless that is as on the pad in Base, then the
/// lines for its hold and double. Without a press line, the first of those becomes the first line.
pub(super) fn callout_lines(
    keymap: Option<&Keymap>,
    layer: Layer,
    input: Input,
    labels: &clamshell_state::ControlLabels,
) -> (Option<FirstLine>, [Option<(Actuation, Fmt<48>)>; 2]) {
    let press = binding(keymap, layer, input).filter(|b| layer != Layer::BASE || change(input, Some(b)) != Change::None);
    let mut lines = press.map(|b| {
        let (text, tone) = described(input, &b, keymap, labels);
        (Actuation::Press, text, tone)
    });
    let mut extra = [None, None];
    for (slot, act) in extra.iter_mut().zip(Actuation::of(input.kind()).filter(|a| *a != Actuation::Press)) {
        let Some(b) = keymap.and_then(|k| k.get_of(layer, input, act)) else {
            continue;
        };
        match lines {
            None => lines = Some((act, summary(input, b, keymap, labels), Tone::Fn)),
            Some(_) => *slot = Some((act, summary(input, b, keymap, labels))),
        }
    }
    (lines, extra)
}

/// An input's binding in keymap `k`'s `layer` itself, if it has one there.
pub(super) fn binding(k: Option<&Keymap>, layer: Layer, input: Input) -> Option<Binding> {
    k.and_then(|k| k.get_in(layer, input)).copied()
}

/// A changed control's label beside its cluster: the control, its text and tone, and the label's top
/// once it has been placed.
#[derive(Clone, Copy)]
pub(super) struct Callout {
    part: Part,
    /// Its first line, which is the press's or else the hold's or double's, and its tone.
    act: Actuation,
    text: Fmt<48>,
    tone: Tone,
    /// A hold's and a double's lines below it, smaller.
    extra: [Option<(Actuation, Fmt<48>)>; 2],
    top: f32,
    /// Whether its extra lines are left out to save room. The first line then says how many with "+N".
    collapsed: bool,
    /// Whether this is Base's label, shown faded in another layer that doesn't map the input.
    faded: bool,
}

impl Callout {
    /// The height at which its line reaches the labels' side.
    fn exit(&self) -> f32 {
        self.part.lane.unwrap_or(self.part.anchor.1)
    }

    fn extras(&self) -> usize {
        self.extra.iter().flatten().count()
    }

    /// Its height: the first line, plus the extra lines unless collapsed.
    fn height(&self, line: f32) -> f32 {
        line + if self.collapsed { 0.0 } else { self.extras() as f32 * line * EXTRA }
    }
}

/// The height and text size of a label's extra lines (a hold's or a double's), as a fraction of the first
/// line's.
pub(super) const EXTRA: f32 = 0.85;

/// The word before a hold's, double's or full pull's line on a label.
pub(super) fn act_word(act: Actuation) -> &'static str {
    match act {
        Actuation::Press => "",
        Actuation::Hold => "hold",
        Actuation::Double => "2×",
        Actuation::FullPull => "full",
    }
}

/// One cluster's labels, in the order their lines arrive, from top to bottom.
pub(super) struct Labels {
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

    /// The labels' total height, with `gap` between them.
    fn total(&self, line: f32, gap: f32) -> f32 {
        self.iter().map(|c| c.height(line)).sum::<f32>() + gap * self.len.saturating_sub(1) as f32
    }

    /// Places the labels, whose first lines are `line` high, level with their lines where possible,
    /// within `y0`..`y1` and at least `gap` apart. If they don't fit, the extra lines are left out first,
    /// and then the gap shrinks.
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

/// What a label's line says: a dim word first (`hold`, `2×`, or none for a press), the text in `tone`,
/// and a dim note after it (`+2`).
pub(super) struct LabelLine<'a> {
    word: &'a str,
    text: &'a str,
    tone: Tone,
    note: &'a str,
}

/// One line of a label at `y` and `size`, with its text cut to `lw`. `at` gives its x from its width, so
/// that it can be placed from the left or the right.
pub(super) fn label_line(f: &mut Frame, at: impl Fn(f32) -> f32, y: f32, size: f32, lw: f32, line: LabelLine) {
    let LabelLine { word, text, tone, note } = line;
    let space = size * 0.3;
    let word_w = if word.is_empty() { 0.0 } else { f.c.text_width(size, word) + space };
    let note_w = if note.is_empty() { 0.0 } else { f.c.text_width(size, note) };
    let text_w = f.c.text_width(size, text).min((lw - word_w - note_w).max(size));
    // The word, text and note in a run, starting where `at` puts them.
    let mut x = at(word_w + text_w + note_w);
    let h = size * LINE;
    if !word.is_empty() {
        f.c.text_in(Rect::new(x, y, word_w, h), size, Tone::Dim, word, Align::Start);
    }
    x += word_w;
    f.c.text_fit(Rect::new(x, y, text_w + 1.0, h), size, tone, text, Keep::Start);
    x += text_w;
    if !note.is_empty() {
        f.c.text_in(Rect::new(x, y, note_w + 1.0, h), size, Tone::Dim, note, Align::Start);
    }
}

/// Where the edit screen's parts go in `body`: the clusters' area below the corners, Close's corner at
/// the top left and the layer button's at the top right, a cluster's width, the gap between it and its
/// labels, the labels' column width, and the panel's strip between the label columns at full height.
pub(crate) struct EditLayout {
    pub area: Rect,
    pub close: Rect,
    pub layer: Rect,
    pub cw: f32,
    pub gap: f32,
    pub lw: f32,
    pub strip: Rect,
}

/// The height of the corners (Close, the layer button), as a fraction of the area's height.
pub(super) const CORNER_H: f32 = 0.075;

/// The edit screen's side margin as a fraction of the usual one, since the drawing needs the room.
pub(super) const EDIT_SIDE: f32 = 0.35;

pub(crate) fn edit_layout(body: Rect) -> EditLayout {
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

/// The edit screen's corners: Close and Undo at the top left, and at the top right the layer shown, as a
/// button to the list of layers. All are faded while a control is being picked.
pub(super) fn edit_corners(f: &mut Frame, l: &EditLayout, keymap: Option<&Keymap>, layer: Layer, picking: bool) {
    let size = f.text(Text::Small);
    let name = keymap.and_then(|k| k.layer_name(layer)).unwrap_or("Base");
    let r = l.layer;
    let arrow = size * 0.7;
    let layer_button = |f: &mut Frame, tone: Tone| {
        let w = f.c.text_width(size, name).min(r.w - arrow * 1.6);
        let x = r.right() - arrow - size * 0.3 - w;
        f.c.text_fit(Rect::new(x, r.y, w + 1.0, r.h), size, tone, name, Keep::Start);
        let mut room = r;
        f.c.icon(Icon::ArrowDown, room.take_right(arrow).square_center(arrow), tone);
    };
    // Close, and Undo beside it, faded when there is nothing to undo.
    let close_w = f.c.text_width(size, "Close") + size * 1.2;
    let close = Rect::new(l.close.x, l.close.y, close_w, l.close.h);
    let undo = Rect::new(close.right(), l.close.y, (l.close.w - close_w).max(1.0), l.close.h);
    // While picking, the panel has Cancel, and these are faded and inactive.
    if picking {
        f.c.text_in(close, size, Tone::Dim, "Close", Align::Start);
        f.c.text_in(undo, size, Tone::Dim, "Undo", Align::Start);
        layer_button(f, Tone::Dim);
        return;
    }
    f.text_button(Id::Profile(Kind::Keymap, ProfileId::Back), close, size, Tone::Fn, "Close", Align::Start);
    if f.s.keymapping().can_undo() {
        f.text_button(Id::Keymap(KeymapId::Undo), undo, size, Tone::Fn, "Undo", Align::Start);
    } else {
        f.c.text_in(undo, size, Tone::Dim, "Undo", Align::Start);
    }
    // The layer shown: Base in blue like any button, another layer in white, so that it is clear which
    // one is being edited.
    let id = Id::Keymap(KeymapId::LayerPicker);
    let tone = if f.pressed(&id) || layer != Layer::BASE { Tone::High } else { Tone::Fn };
    f.button(id, r, Look::TEXT, |f| layer_button(f, tone));
}

/// The edit screen. The controls sit in two clusters at the sides (see [`super::pad`]), each one a
/// target. A control that the layer shown changes has a line to a label: blue when it is mapped to
/// something else, grey when it acts as on the pad with tuned parameters. Home is ClamShell's and drawn
/// dim. The layer button at the top switches the layer shown. Between the label columns is the panel,
/// with the selected input, what it does in the layer, the ways to remap it, and its parameters as
/// sliders or switches. Below are a hint and "No changes". While a control is being picked, either what
/// the selected input maps to or what holds the gyro on, the controls that can't be picked are dimmed,
/// and the panel and the hint say what to do.
pub(super) fn edit(f: &mut Frame, id: u16, picking: Option<Picking>) {
    let body = f.body();
    let s = f.s;
    let keymap = s.keymapping().keymaps().get(id);
    let view = s.keymapping().edit();
    let layer = view.layer;
    let layout = edit_layout(body);
    edit_corners(f, &layout, keymap, layer, picking.is_some());
    let EditLayout { area, cw, gap, lw, strip, .. } = layout;
    let size = f.text(Text::Small) * LABEL;
    let line = size * LINE;
    let pad = Pad::new(area, cw, s.profile().control_layout);

    let (mut left, mut right) = (Labels::new(), Labels::new());
    for input in Input::all() {
        let Some(part) = pad.part(input) else {
            continue;
        };
        // While a control is being picked, those that can't be picked have no label.
        if picking.is_some_and(|p| !p.takes(input)) {
            continue;
        }
        // The layer's own label. In a layer other than Base, an input the layer doesn't map shows Base's,
        // faded.
        let (mut lines, mut extra) = callout_lines(keymap, layer, input, &s.profile().control_layout.labels);
        let mut faded = false;
        if lines.is_none() && layer != Layer::BASE {
            (lines, extra) = callout_lines(keymap, Layer::BASE, input, &s.profile().control_layout.labels);
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

    // The controls, with the stick clicks last since they sit on the sticks' caps.
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
                Some(b) => described(input, &b, keymap, &s.profile().control_layout.labels).1,
                None => Tone::Dim,
            };
            let muted = picking.is_some_and(|p| !p.takes(input));
            // While picking the gyro's hold, the control holding it now is lit.
            let hold = match picking {
                Some(Picking::Hold(gyro)) => s.keymapping().param_base(id, gyro).and_then(|b| b.hold) == Some(input),
                _ => false,
            };
            control(f, input, part, tone, view.selected == Some(input) || hold, muted, size);
        }
    }

    // The labels. Where their column is too narrow for words, as in a narrow window, a dot goes at the
    // line's end instead, and the panel says what the selected one does.
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
            label_line(f, at, c.top, size, lw, LabelLine { word: act_word(c.act), text: &c.text, tone: c.tone, note: &more });
            if c.collapsed {
                continue;
            }
            let mut y = c.top + line;
            for (act, text) in c.extra.iter().flatten() {
                let tone = if c.faded { Tone::Dim } else { Tone::Fn };
                label_line(f, at, y, size * EXTRA, lw, LabelLine { word: act_word(*act), text, tone, note: "" });
                y += line * EXTRA;
            }
        }
    }

    // Home, which is ClamShell's and drawn dim, at the screen's bottom left corner.
    let fixed = if picking.is_some() { MUTED } else { FIXED };
    if let Some(r) = pad.foot(Side::Left) {
        f.c.outline(r, r.w, PART_W, fixed);
        let w = f.c.text_width(size, "Home");
        f.c.text_in(Rect::new(r.x - r.w * 0.6 - w, r.y, w, r.h), size, fixed, "Home", Align::Start);
    }

    // The panel fills the strip between the label columns, above the keymap line. That line says which
    // keymap is edited, a game's own or a named one with who uses it (its edits reach them all), and
    // offers Make own.
    let margin = (strip.w * 0.04).min(size);
    let mut panel_r = Rect::new(strip.x + margin, strip.y, (strip.w - 2.0 * margin).max(1.0), strip.h);
    if picking.is_none() {
        let small = f.text(Text::Small) * PANEL_TEXT;
        let row = panel_r.take_bottom(small * LINE * 1.6);
        // The line reaches into the label columns while their labels end above it. The labels of AYN and
        // Back, the controls beside it, reach down to it when they are mapped. Both sides widen alike, so
        // that it stays centred.
        let free = |column: &Labels| column.iter().all(|c| c.top + c.height(line) <= row.y);
        let into = if free(&left) && free(&right) { lw * 0.9 } else { 0.0 };
        keymap_line(f, Rect::new(strip.x - into, row.y, strip.w + 2.0 * into, row.h), id, small);
    }
    match picking {
        Some(p) => picking_panel(f, panel_r, id, p),
        None => panel(f, panel_r, id, keymap, view),
    }
}

/// Which keymap is edited: a game's own ("Own keymap of Hades"), or a named one with who uses it ("FPS,
/// used by all games, 2 games"). For a game without its own keymap, "Make own" follows, which makes a
/// copy for that game alone to edit from then on.
fn keymap_line(f: &mut Frame, mut row: Rect, id: u16, size: f32) {
    let own = f.s.keymapping().is_own(id);
    let text = match (own, f.s.panel().editing()) {
        (true, Some(entry)) => Fmt::<128>::of(format_args!("Own keymap of {}", entry.title)),
        (true, None) => Fmt::of(format_args!("Own keymap")),
        (false, _) => Fmt::of(format_args!("{}, used by {}", f.s.keymapping().name(id), Lower(super::users(f, id).as_str()))),
    };
    let label = if !own && f.s.panel().keymap_can_make_own() { "Make own" } else { "" };
    // The text and then the action, centred together.
    let gap = size;
    let action_w = if label.is_empty() { 0.0 } else { f.c.text_width(size, label) + gap };
    let w = (f.c.text_width(size, &text) + action_w).min(row.w);
    row = Rect::new(row.x + (row.w - w) / 2.0, row.y, w, row.h);
    if !label.is_empty() {
        let action = row.take_right(action_w);
        f.text_button(Id::Keymap(KeymapId::MakeOwn), action, size, Tone::Fn, label, Align::End);
    }
    f.c.text_fit(row, size, Tone::Dim, &text, Keep::Start);
}

/// Text with its first letter in lower case, as for "All games" after "used by".
struct Lower<'a>(&'a str);

impl std::fmt::Display for Lower<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut chars = self.0.chars();
        if let Some(c) = chars.next() {
            f.write_char(c.to_ascii_lowercase())?;
        }
        f.write_str(chars.as_str())
    }
}

/// A control in a cluster, as a target: its outline and its mark in `tone` (blue when mapped, grey when
/// tuned, dim when as on the pad). The outline is brighter when selected, and darker when `muted`
/// because it can't be picked.
pub(super) fn control(f: &mut Frame, input: Input, p: Part, tone: Tone, selected: bool, muted: bool, size: f32) {
    let look = Look { radius: p.radius, fill: None, inset: 0.0, ring: -3.0, press_fill: true };
    let shown = p.shown;
    let id = Id::Keymap(KeymapId::Input(input));
    let (lw, lc) = match (selected, muted) {
        (true, _) => (SELECTED_W, Tone::High.rgb()),
        (false, true) => (PART_W, MUTED),
        (false, false) => (PART_W, PART_LINE),
    };
    let tone = if muted { MUTED } else { tone.rgb() };
    // A stick also takes its group's corners as targets, added first so the cap's outline stays on top.
    for corner in p.corners.into_iter().flatten() {
        f.button_in(id.clone(), corner, shown, look, |_| {});
    }
    f.button_in(id, p.target, shown, look, |f| {
        let (cx, cy) = shown.center();
        // Centred by its ink, so that a letter in a round button looks centred whatever its shape.
        let name = |f: &mut Frame, t: &str, r: Rect, size: f32| {
            f.c.text_ink_centred(r, size, tone, t);
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
                name(f, printed(b, &f.s.profile().control_layout.labels), shown, size.min(shown.h * 0.6));
            }
            Input::Button(b @ (Button::A | Button::B | Button::X | Button::Y)) => {
                f.c.outline(shown, shown.w, lw, lc);
                name(f, printed(b, &f.s.profile().control_layout.labels), shown, shown.h * 0.48);
            }
            Input::Button(b @ (Button::LS | Button::RS)) => {
                f.c.outline(shown, shown.w, lw, lc);
                name(f, printed(b, &f.s.profile().control_layout.labels), shown.inset(-shown.w), shown.h * 0.4);
            }
            Input::Button(b @ (Button::View | Button::Menu | Button::Back)) => {
                f.c.outline(shown, shown.w, lw, lc);
                // Its name on the outer side.
                let w = f.c.text_width(size, printed(b, &f.s.profile().control_layout.labels));
                let x = if p.side == Side::Left { shown.x - shown.w * 0.6 - w } else { shown.right() + shown.w * 0.6 };
                f.c.text_in(Rect::new(x, shown.y, w, shown.h), size, tone, printed(b, &f.s.profile().control_layout.labels), Align::Start);
            }
            Input::Stick(_) => f.c.outline(shown, shown.w, lw, lc),
            Input::Button(Button::Ayn) => {
                f.c.outline(shown, shown.h / 2.0, lw, lc);
                name(f, "AYN", shown, size.min(shown.h * 0.6));
            }
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
