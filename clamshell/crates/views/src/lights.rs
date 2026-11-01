//! The stick lights' screens, drawn over the settings tab. Their rules are on
//! [`clamshell_state::lights::screens::Lighting`].
//!
//! - The picker and a set's menu, the profiles' picker ([`super::profiles`]).
//! - A set's edit screen: the two sticks drawn lit in their colours, each a button that picks the stick
//!   the sliders set while unlinked (its label brighter); Linked; a row for hue, saturation and
//!   brightness, whose held slider is the curve editor's; and "Charging shows the battery". The lights
//!   follow the sliders as they move. On a device without lights the header says so, and the rest is
//!   drawn as usual.

use clamshell_state::lights::screens::{LightsId, LightsScreen};
use clamshell_state::lights::{Colour, LightParam, Stick};
use clamshell_state::profiles::picker::{Kind, ProfileId};
use clamshell_state::{Control, Id};

use clamshell_ui::gfx::{Column, Rect, Rgb};
use clamshell_ui::toolkit::format::Fmt;
use clamshell_ui::toolkit::style::{Look, Text, Tone};
use clamshell_ui::toolkit::widgets::HeaderRight;
use clamshell_ui::toolkit::{Frame, Track, widgets};

/// A row's height, as a fraction of the area's height, and its text, as a fraction of the small size, as
/// on the curve's edit screen.
const ROW_H: f32 = 0.075;
const ROW_TEXT: f32 = 0.8;
/// A stick's radius, as a fraction of the picture's height.
const STICK: f32 = 0.2;
/// The least a lit stick is drawn at, as a share of its colour, so that a dim light still shows.
const LEAST: f32 = 0.25;

pub(super) fn draw(f: &mut Frame, screen: LightsScreen) {
    match screen {
        LightsScreen::Picker(p) => super::profiles::draw(f, Kind::Lights, p),
        LightsScreen::Edit(id) => edit(f, id),
    }
}

fn edit(f: &mut Frame, id: u16) {
    let s = f.s;
    let lighting = s.lighting();
    let lights = lighting.in_force(id);
    let title = Fmt::<96>::of(format_args!("Lights: {}", lighting.name(id)));
    let note = if s.profile().lights.is_none() { HeaderRight::Note("No stick lights on this device") } else { HeaderRight::None };
    let area = widgets::header(f, Some(Id::Profile(Kind::Lights, ProfileId::Back)), &title, false, note);
    let size = f.text(Text::Small) * ROW_TEXT;
    let rh = f.body().h * ROW_H;
    let rows_h = rh * (LightParam::ALL.len() + 2) as f32;
    let pad = f.pad();
    sticks(f, Rect::new(area.x, area.y, area.w, (area.h - rows_h - pad).max(0.0)), lights.linked, size, |stick| lights.colour(stick));
    let mut col = Column::new(Rect::new(area.x, area.bottom() - rows_h, area.w, rows_h));
    if let Some(row) = col.next(rh) {
        widgets::switch_row(f, Id::Lights(LightsId::Linked), row, size, "Linked", lights.linked);
    }
    // The track spans the screen's width, as the curve's sliders do.
    let body = f.body();
    let track = Track { x0: body.x, w: body.w };
    let colour = lights.colour(if lights.linked { Stick::Left } else { lighting.stick() });
    let mut held = None;
    for p in LightParam::ALL {
        let Some(row) = col.next(rh) else {
            break;
        };
        // A colour has no default to differ from, so its values are all as set.
        let row = widgets::slider_row(f, row, size, (Control::Light(p), track), p.label(), shown(p, p.get(&colour)).as_str(), Tone::Normal);
        held = held.or(row.map(|h| (h, p)));
    }
    if let Some(row) = col.next(rh) {
        widgets::switch_row(f, Id::Lights(LightsId::Battery), row, size, "Charging shows the battery", lights.battery);
    }
    // A held slider is drawn last, so its card covers the rows below it.
    if let Some((held, p)) = held {
        let text = Fmt::<32>::of(format_args!("{} {}", p.label(), shown(p, p.value(held.slider.value)).as_str()));
        widgets::held_row(f, held, area, &text);
    }
}

/// The two sticks in `r`, each a ring lit in its colour with its name below, and a button that picks it.
/// Unlinked, the picked stick's name is brighter.
fn sticks(f: &mut Frame, r: Rect, linked: bool, size: f32, colour: impl Fn(Stick) -> Colour) {
    let radius = (r.h * STICK).min(r.w * 0.12);
    if radius < size {
        return;
    }
    let picked = f.s.lighting().stick();
    for (i, stick) in Stick::BOTH.into_iter().enumerate() {
        let cx = r.x + r.w * if i == 0 { 0.3 } else { 0.7 };
        let cy = r.y + r.h * 0.45;
        let cell = Rect::new(cx - radius * 1.6, cy - radius * 1.4, radius * 3.2, radius * 2.8 + size * 1.6);
        let c = colour(stick);
        let tone = if !linked && stick == picked { Tone::High } else { Tone::Dim };
        f.button(Id::Lights(LightsId::Stick(stick)), cell, Look::ROW, |f| {
            if c.brightness > 0 {
                f.c.circle(cx, cy, radius, glow(c));
            }
            f.c.outline(Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0), radius, 2.0, tone.rgb());
            f.c.text_in(Rect::new(cell.x, cy + radius * 1.15, cell.w, size * 1.4), size, tone, stick.label(), clamshell_ui::gfx::Align::Center);
        });
    }
}

/// A light's colour as drawn: its colour at its brightness, no darker than [`LEAST`] of it.
fn glow(c: Colour) -> Rgb {
    let share = LEAST + (1.0 - LEAST) * f32::from(c.brightness.min(100)) / 100.0;
    let [r, g, b] = c.rgb().map(|v| (f32::from(v) * share).round() as u32);
    Rgb((r << 16) | (g << 8) | b)
}

/// A part's value with its unit.
fn shown(p: LightParam, v: u16) -> Fmt<16> {
    match p {
        LightParam::Hue => Fmt::of(format_args!("{v}°")),
        LightParam::Saturation | LightParam::Brightness => Fmt::of(format_args!("{v} %")),
    }
}
