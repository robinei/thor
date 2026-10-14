//! Follow mode under shelf: the selected game's settings and the Launch button.

use sysprog_core::{Id, Power};

use super::{DIM, Frame, GO, Look, SURFACE, TEXT, TEXT_HI, label};
use crate::draw::{Rect, rgb};

/// Padding, of the area's height.
const PAD: f32 = 0.05;
/// Vertical positions and sizes, of the area's height.
const TITLE: f32 = 0.07;
const SECTION_Y: f32 = 0.2;
const SECTION: f32 = 0.045;
const CHOICES_Y: f32 = 0.29;
const CHOICES_H: f32 = 0.16;
const LAUNCH_Y: f32 = 0.62;
const LAUNCH_H: f32 = 0.2;
/// Label size, of a button's height.
const BUTTON_TEXT: f32 = 0.3;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let s = f.s;
    let Some(game) = s.selected_game() else {
        return;
    };
    let pad = body.h * PAD;
    f.c.text(pad, body.y + pad, body.h * TITLE, rgb(TEXT_HI), &game.title);
    f.c.text(pad, body.y + body.h * SECTION_Y, body.h * SECTION, rgb(DIM), "Power");

    let row = Rect::new(pad, body.y + body.h * CHOICES_Y, body.w - 2.0 * pad, body.h * CHOICES_H);
    let n = Power::ALL.len() as f32;
    let button_w = (row.w - (n - 1.0) * pad) / n;
    let current = s.power_of(&game.id);
    let look = Look { radius: 8.0, fill: Some(SURFACE), inset: 0.0, ring_inset: -2.0 };
    for (i, power) in Power::ALL.into_iter().enumerate() {
        let r = Rect::new(row.x + i as f32 * (button_w + pad), row.y, button_w, row.h);
        let chosen = power == current;
        f.button(Id::Power(power), r, look, |c| {
            if chosen {
                c.outline(r, 8.0, 2.0, rgb(game.color));
            }
            label(c, r, r.h * BUTTON_TEXT, if chosen { TEXT_HI } else { TEXT }, power.label());
        });
    }

    let launch = Rect::new(pad, body.y + body.h * LAUNCH_Y, body.w - 2.0 * pad, body.h * LAUNCH_H);
    let look = Look { radius: 10.0, fill: None, inset: 0.0, ring_inset: -2.0 };
    f.button(Id::Launch, launch, look, |c| {
        c.outline(launch, 10.0, 2.0, rgb(GO));
        label(c, launch, launch.h * BUTTON_TEXT, GO, "Launch");
    });
}
