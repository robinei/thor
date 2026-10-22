//! The edit screen's panel: the selected input's name, actuation tabs, what it does, the ways to remap it
//! and its parameters (rows, switches, held sliders); and the panel while a control is picked.

use clamshell_core::keymap::screens::{EditView, way_of, ways};
use clamshell_core::keymap::{Actuation, Binding, Change, Input, Keymap, Kind, Layer, Output, Param, change, params_of};
use clamshell_core::levels::slider::{Control, Slider};
use clamshell_core::{Id, KeymapId, Target};

use crate::draw::{Align, Column, Keep, LINE, Rect};
use crate::icons::Icon;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Text, Tone};
use crate::toolkit::widgets::HeldSlider;
use crate::toolkit::{Frame, Track, widgets};

use super::words::{Label, as_on_pad, described, shown, summary};
use super::{BUTTON_H, PANEL_ROW_H, PANEL_TEXT, Picking, remap_title};

/// The panel while a control is picked: what for, what to do, and Cancel (for the gyro's hold, Always
/// beside it).
pub(super) fn picking_panel(f: &mut Frame, r: Rect, id: u16, picking: Picking) {
    let small = f.text(Text::Small) * PANEL_TEXT;
    let heading = f.text(Text::Heading) * 0.9;
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
    col.skip(f.pad() * 0.3);
    let Some(row) = col.next(f.body().h * BUTTON_H * 0.65) else {
        return;
    };
    match picking {
        Picking::Hold(gyro) => {
            let always = f.s.param_base(id, gyro).is_some_and(|b| b.hold.is_none());
            let (a, c) = (row.cell(2, 1, 0.0, 0), row.cell(2, 1, 0.0, 1));
            f.text_button(Id::Keymap(KeymapId::Hold(None)), a, small, if always { Tone::High } else { Tone::Fn }, "Always", Align::Center);
            f.text_button(Id::Keymap(KeymapId::Back), c, small, Tone::Fn, "Cancel", Align::Center);
        }
        Picking::Target(_) => f.text_button(Id::Keymap(KeymapId::Back), row, small, Tone::Fn, "Cancel", Align::Center),
    }
}

/// The icon for a way to remap an input.
pub(super) fn way_icon(way: Target) -> Icon {
    match way {
        Target::Pick => Icon::Gamepad,
        Target::Key | Target::Keys => Icon::Keyboard,
        Target::MouseParts | Target::To(Output::Pointer) => Icon::Mouse,
        Target::Layers => Icon::Layers,
        Target::To(Output::Nothing) => Icon::Off,
        _ => Icon::Reset,
    }
}

/// The panel: the selected input's name; what it does in the layer shown; the ways to remap it, as icons
/// (the way it is mapped now in white); its parameters, a row each (one whose slider is up shows it in its
/// place).
pub(super) fn panel(f: &mut Frame, r: Rect, id: u16, keymap: Option<&Keymap>, view: EditView) {
    let body = f.body();
    let s = f.s;
    let pad = f.pad();
    let small = f.text(Text::Small) * PANEL_TEXT;
    let mut col = Column::new(r);
    let Some(input) = view.selected else {
        let mut lines = Column::new(r);
        lines.skip(r.h * 0.4);
        widgets::text_lines(f, &mut lines, &[(small, Tone::Dim, "Press or touch an input"), (small, Tone::Dim, "to see what it does")]);
        // In another layer than Base: which, and what the faded labels are.
        if view.layer != Layer::BASE {
            let name = keymap.and_then(|k| k.layer_name(view.layer)).unwrap_or("");
            lines.skip(small * LINE);
            widgets::text_lines(f, &mut lines, &[(small, Tone::High, name), (small * 0.85, Tone::Dim, "Faded: as in Base")]);
        }
        return;
    };
    let layer = view.layer;
    let act = view.act;
    let own = keymap.and_then(|k| k.get_of(layer, input, act)).copied();
    // The name.
    let name = Fmt::<32>::of(format_args!("{}", Label(input)));
    let heading = f.text(Text::Heading) * 0.9;
    if let Some(row) = col.next(heading * LINE) {
        widgets::centred_text(f, row, heading, Tone::High, &name);
    }
    // Press, Hold, Double (a button's, a direction's), Pull and Full pull (a trigger's): the one shown in
    // white; a dot under one set.
    let tabs = Actuation::of(input.kind()).count();
    if tabs > 1
        && let Some(row) = col.next(small * LINE * 2.2)
    {
        for (i, a) in Actuation::of(input.kind()).enumerate() {
            let cell = row.cell(tabs, 1, 0.0, i);
            let tab = Rect::new(cell.x, cell.y, cell.w, cell.h * 0.85);
            let tone = if a == act { Tone::High } else { Tone::Fn };
            f.text_button(Id::Keymap(KeymapId::Actuation(a)), tab, small, tone, a.label_for(input.kind()), Align::Center);
            if keymap.is_some_and(|k| k.get_of(layer, input, a).is_some()) {
                f.c.circle(tab.center().0, row.bottom() - small * 0.25, small * 0.1, tone);
            }
        }
    }
    // What it does: its binding for the actuation shown, or without one in a layer above Base, Base's.
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
                let mut lines = Column::new(row.centered(row.w, 2.0 * small * LINE));
                widgets::text_lines(f, &mut lines, &[(small, Tone::Dim, "As in Base"), (small, Tone::Dim, &main())]);
            }
            (None, _) => widgets::centred_lines(f, row, small, Tone::Dim, &unset(input)),
            // (The panel's own content in white: blue is for labels of what is remapped, and actions.)
            (Some(b), _) => {
                let tone = if input == Input::Gyro || change(input, Some(&b)) == Change::Mapped || act != Actuation::Press {
                    Tone::High
                } else {
                    described(input, &b, keymap).1
                };
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
    let bh = f.body().h * BUTTON_H;
    col.skip(pad * 0.5);
    if let Some(row) = col.next(bh) {
        let ways = ways(input.kind());
        let now = way_of(own.map(|b| b.output));
        for (i, way) in ways.iter().enumerate() {
            let r = row.cell(ways.len(), 1, 0.0, i);
            let kid = Id::Keymap(KeymapId::Target(*way));
            let tone = if *way == now || f.pressed(&kid) { Tone::High } else { Tone::Fn };
            f.button(kid, r, Look::TEXT, |f| f.c.icon(way_icon(*way), r.square_center(row.h * 0.5), tone));
        }
    }
    col.skip(pad * 0.4);
    // The parameters: the binding's, else the layers' below, else as on the pad.
    let Some(base) = s.param_base(id, input) else {
        return;
    };
    let rh = f.body().h * PANEL_ROW_H;
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
pub(super) fn invert_row(f: &mut Frame, row: Rect, size: f32, b: &Binding) {
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

/// A parameter's row while its slider is up (the bar's slider, within the panel): the name and value
/// above, the track along the row with the thumb under the finger.
pub(super) fn slider_row(f: &mut Frame, row: Rect, track: Track, sl: Slider, p: Param, panel: Rect) {
    // As the bar's: the track along the screen through the row, the value on a card beside the finger.
    let text = Fmt::<32>::of(format_args!("{} {}", p.label(), shown(p, sl.value).as_str()));
    let within = Rect::new(track.x0, panel.y, track.w, panel.h);
    widgets::held_slider(f, HeldSlider { slider: &sl, x0: track.x0, cy: row.y + row.h / 2.0, h: row.h * 0.75, within, icon: None, text: &text });
}
