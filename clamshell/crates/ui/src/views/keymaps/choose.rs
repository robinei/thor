//! The screens that a way to remap opens, and the layer screens. These are the layer lists (to show a
//! layer, or to pick one as a target), a layer's menu, the mouse's drawing, and a key chord typed on the
//! keyboard.

use std::fmt::Write;

use clamshell_state::bottom::keyboard::Modifier;
use clamshell_state::keymap::{Chord, Dir, Input, Layer, MouseButton, Output};
use clamshell_state::{Id, KeymapId, ListId, RowPart, Target};

use crate::gfx::icons::Icon;
use crate::gfx::{Align, Keep, LINE, Rect};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Text, Tone};
use crate::toolkit::widgets::{EditCell, HeaderRight, ListArea, ListRow};
use crate::toolkit::{Frame, widgets};
use crate::views::editor;

use super::edit::binding;
use super::words::{Label, OutputLabel, turned_on_by};
use super::{BUTTON_H, PART_LINE, PART_W, ROW_H, SELECTED_W, choice_header, header, remap_title};

/// The layers that `input` can turn on: the keymap's layers except Base, each with what turns it on, and
/// the one it maps to now marked. New layer… at the end names a new layer and then maps to it. Choosing
/// one closes the screen; Cancel gives up.
pub(super) fn layers(f: &mut Frame, id: u16, input: Input) {
    let now = binding(f.s.keymapping().keymaps().get(id), f.s.keymapping().edit().layer, input).map(|b| b.output);
    let area = choice_header(f, &remap_title(input, &f.s.profile().control_layout.labels));
    layer_list(f, area, id, |l| now == Some(Output::Layer(l)), false);
}

/// A keymap's layers on the scrolled list ([`ListId::Layers`]), in the rows the state gives. Each row
/// has the layer's name, with `current` marked, what turns it on, and an Edit button if `edit` is set.
/// New layer… comes last.
pub(super) fn layer_list(f: &mut Frame, area: Rect, id: u16, current: impl Fn(Layer) -> bool, edit: bool) {
    let s = f.s;
    let keymap = s.keymapping().keymaps().get(id);
    let list = ListArea::new(f, area, f.body().h * ROW_H);
    let rows = list.scrolled(f, ListId::Layers, s.lists().len(ListId::Layers));
    for i in rows.shown() {
        let at = |part| Id::Row { list: ListId::Layers, i: i as u16, part };
        let layer = s.keymapping().layer_row(i);
        let (name, what, mine) = match layer {
            Some(l) => (
                keymap.and_then(|k| k.layer_name(l)).unwrap_or(""),
                keymap.map_or(Fmt::new(), |k| turned_on_by(k, l, &f.s.profile().control_layout.labels)),
                current(l),
            ),
            None => ("New layer…", Fmt::new(), false),
        };
        let tone = if layer.is_some() { Tone::high_if(mine) } else { Tone::Fn };
        let cell = match (edit, layer) {
            (false, _) => EditCell::None,
            (true, Some(_)) => EditCell::Edit(at(RowPart::Edit)),
            (true, None) => EditCell::Empty,
        };
        let item = ListRow { label: name, tone, mark: mine, note: &what, edit: cell };
        widgets::list_row(f, at(RowPart::Main), rows.row(i), list.pad, list.size, item);
    }
    list.end(f, rows);
}

/// A layer's menu: rename it, or delete it along with what maps to it.
pub(super) fn layer_menu(f: &mut Frame, id: u16, layer: Layer) {
    let name = f.s.keymapping().keymaps().get(id).and_then(|k| k.layer_name(layer)).unwrap_or("");
    let area = header(f, name, HeaderRight::None);
    let k = |id| Id::Keymap(id);
    if f.s.keymapping().edit().clearing {
        widgets::menu(f, area, [(k(KeymapId::ConfirmClear), "Clear all its bindings", Tone::Accent), (k(KeymapId::Back), "Cancel", Tone::Fn)]);
    } else if layer == Layer::BASE {
        widgets::menu(f, area, [(k(KeymapId::Clear), "Clear", Tone::Fn)]);
    } else {
        // Once: a switch, and a line saying what it means.
        let size = f.text(Text::Body);
        let small = f.text(Text::Small);
        let mut rest = area;
        let row = rest.take_top(f.body().h * BUTTON_H);
        let on = f.s.keymapping().keymaps().get(id).is_some_and(|k| k.layer_once(layer));
        // Its row reaches into the margin, so that its label lines up with the actions below.
        let r = Rect::new(row.x, row.y, widgets::menu_w(area, size), row.h).widened(size * 0.4);
        widgets::switch_row(f, k(KeymapId::LayerOnce), r, size, "Once", on);
        let note = rest.take_top(small * LINE * 1.6);
        f.c.text_fit(note.top_line(small), small, Tone::Dim, "Tapped: on until the next input it maps", Keep::Start);
        rest.take_top(f.pad() * 0.5);
        widgets::menu(
            f,
            rest,
            [(k(KeymapId::RenameLayer), "Rename", Tone::Fn), (k(KeymapId::Clear), "Clear", Tone::Fn), (k(KeymapId::DeleteLayer), "Delete", Tone::Accent)],
        );
    }
}

/// The keymap's layers, for choosing which one the edit screen shows, with the shown one marked and New
/// layer… at the end. A layer's Edit button or a long press opens its menu. Back returns to the edit
/// screen.
pub(super) fn layer_picker(f: &mut Frame, id: u16) {
    let shown = f.s.keymapping().edit().layer;
    let area = choice_header(f, "Layers");
    layer_list(f, area, id, |l| l == shown, true);
}

/// The mouse drawing's height as a fraction of the room under the header, and its width as a fraction of
/// its height.
pub(super) const MOUSE_H: f32 = 0.8;
pub(super) const MOUSE_ASPECT: f32 = 0.6;

/// Picks a part of the mouse for `input` on a drawing of a mouse. The parts are the left and right
/// buttons (the top halves of the body), the wheel with arrows for its turns (pressing it is the middle
/// button), and the two side buttons on the left, Forward above Back. Each part is a target, and the one
/// it maps to now is lit. Choosing one closes the screen; Cancel or Back gives up.
pub(super) fn mouse(f: &mut Frame, id: u16, input: Input) {
    let s = f.s;
    let out = binding(s.keymapping().keymaps().get(id), s.keymapping().edit().layer, input).map(|b| b.output);
    let area = choice_header(f, &remap_title(input, &f.s.profile().control_layout.labels));
    let small = f.text(Text::Small);
    let room = widgets::hint(f, area, "A button, the wheel (its middle button) or an arrow (a turn of it)");
    let h = (room.h * MOUSE_H).min(room.w * 0.5 / MOUSE_ASPECT);
    let w = h * MOUSE_ASPECT;
    let m = Rect::new(room.center().0 - w / 2.0, room.y + (room.h - h) / 2.0, w, h);
    let cx = m.center().0;
    let split = m.y + h * 0.42;
    let lit = |o: Output| out == Some(o);
    let line = |on: bool| if on { (SELECTED_W, Tone::High.rgb()) } else { (PART_W, PART_LINE) };
    let look = Look { radius: w * 0.1, fill: None, inset: 0.0, ring: -3.0, press_fill: false };
    let label = |f: &mut Frame, kid: &Id, on: bool| if on || f.pressed(kid) { Tone::High } else { Tone::Fn };

    // The wheel and the arrows for its turns. The line between the buttons stops short of them.
    let wheel = Rect::new(cx - w * 0.07, m.y + h * 0.11, w * 0.14, h * 0.14);
    let a = w * 0.14;
    // The body, the line between the buttons and the palm, and the line between the two buttons.
    f.c.outline(m, w * 0.45, PART_W, PART_LINE);
    f.c.line((m.x + PART_W, split), (m.right() - PART_W, split), PART_W, PART_LINE);
    f.c.line((cx, wheel.bottom() + a), (cx, split), PART_W, PART_LINE);
    // The buttons are the top halves of the body, with their names inside.
    for (b, half) in [(MouseButton::Left, Rect::new(m.x, m.y, w / 2.0, split - m.y)), (MouseButton::Right, Rect::new(cx, m.y, w / 2.0, split - m.y))] {
        let kid = Id::Keymap(KeymapId::Target(Target::To(Output::Mouse(b))));
        let on = lit(Output::Mouse(b));
        f.button(kid.clone(), half, look, |f| {
            let name = Rect::new(half.x, split - half.h * 0.22, half.w, half.h * 0.16);
            let tone = label(f, &kid, on);
            f.c.text_in(name, small * 0.9, tone, if b == MouseButton::Left { "Left click" } else { "Right click" }, Align::Center);
        });
    }
    // The wheel, which is also the middle button, and its turns.
    let turns = [
        (Dir::Up, Rect::new(cx - a / 2.0, wheel.y - a, a, a), Icon::ArrowUp),
        (Dir::Down, Rect::new(cx - a / 2.0, wheel.bottom(), a, a), Icon::ArrowDown),
        (Dir::Left, Rect::new(wheel.x - a * 1.05, wheel.center().1 - a / 2.0, a, a), Icon::ArrowLeft),
        (Dir::Right, Rect::new(wheel.right() + a * 0.05, wheel.center().1 - a / 2.0, a, a), Icon::ArrowRight),
    ];
    let kid = Id::Keymap(KeymapId::Target(Target::To(Output::Mouse(MouseButton::Middle))));
    let on = lit(Output::Mouse(MouseButton::Middle));
    f.button(kid, wheel, Look { radius: wheel.w / 2.0, ..look }, |f| {
        let (lw, lc) = line(on);
        f.c.outline(wheel, wheel.w / 2.0, lw, lc);
    });
    for (d, r, icon) in turns {
        let kid = Id::Keymap(KeymapId::Target(Target::To(Output::Wheel(d))));
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
        let kid = Id::Keymap(KeymapId::Target(Target::To(Output::Mouse(b))));
        let on = lit(Output::Mouse(b));
        f.button_in(kid.clone(), target, shown, Look { radius: sw / 2.0, ..look }, |f| {
            let (lw, lc) = line(on);
            f.c.outline(shown, sw / 2.0, lw, lc);
            let tone = label(f, &kid, on);
            f.c.text_in(Rect::new(target.x, y, tw, shown.h), small * 0.9, tone, name, Align::Start);
        });
    }
}

/// A key chord typed on the keyboard below, or a stick's four keys typed in turn.
pub(super) fn capture(f: &mut Frame, input: Input, keys: Option<(usize, [Chord; 4])>) {
    let kb = f.s.keyboard();
    let on = |m| kb.latch(m).on();
    let held = Chord { key: None, shift: on(Modifier::Shift), ctrl: on(Modifier::Ctrl), alt: on(Modifier::Alt), logo: on(Modifier::Super) };
    let mut text = Fmt::<96>::new();
    let title = match keys {
        None => {
            Fmt::<128>::of(format_args!("Remap {} to… modifiers, then a key (Done: the modifiers alone)", Label(input, &f.s.profile().control_layout.labels)))
        }
        Some((n, so_far)) => {
            for k in so_far.iter().take(n) {
                let _ = write!(text, "{}  ", OutputLabel(Output::Key(*k), &f.s.profile().control_layout.labels));
            }
            let dir = [Dir::Up, Dir::Left, Dir::Down, Dir::Right].get(n).copied().unwrap_or(Dir::Up);
            Fmt::<128>::of(format_args!("Remap {} to… the key for {} ({} of 4)", Label(input, &f.s.profile().control_layout.labels), dir.name(), n + 1))
        }
    };
    if !held.is_empty() {
        let _ = write!(text, "{}+", OutputLabel(Output::Key(held), &f.s.profile().control_layout.labels));
    }
    editor::field(f, &title, &text, (Id::Keymap(KeymapId::Back), "Cancel"), (Id::Keymap(KeymapId::Done), "Done"));
}
