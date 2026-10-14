//! Game settings: for the game in context (the running one, else shelf's selection) its power profile
//! and frame rate cap, saved per game; applied at once while it runs, else at its launch. A Launch
//! button while it isn't running.

use sysprog_core::{FPS_CAPS, Id, Power};

use super::{DIM, Frame, GO, Look, SURFACE, TEXT, TEXT_HI, label};
use crate::draw::{Rect, rgb};

/// Padding, of the area's height.
const PAD: f32 = 0.045;
/// Vertical positions and sizes, of the area's height.
const TITLE: f32 = 0.065;
const NOTE: f32 = 0.038;
const SECTION: f32 = 0.04;
const ROW_H: f32 = 0.13;
const POWER_Y: f32 = 0.24;
const FPS_Y: f32 = 0.47;
const LAUNCH_Y: f32 = 0.74;
const LAUNCH_H: f32 = 0.16;
/// Label size, of a button's height.
const BUTTON_TEXT: f32 = 0.32;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let s = f.s;
    let Some(game) = s.context() else {
        return;
    };
    let running = s.running.as_ref() == Some(&game.id);
    let current = s.settings_of(&game.id);
    let pad = body.h * PAD;
    let x = body.x + pad;
    let w = body.w - 2.0 * pad;
    let title_w = f.c.text(x, body.y + pad, body.h * TITLE, rgb(TEXT_HI), &game.title);
    let note = if running { "applies now" } else { "applies at launch" };
    f.c.text(x + title_w + pad, body.y + pad + body.h * (TITLE - NOTE) * 0.8, body.h * NOTE, rgb(DIM), note);

    let row = |f: &mut Frame, y: f32, name: &str| {
        f.c.text(x, body.y + body.h * y - body.h * SECTION * 1.5, body.h * SECTION, rgb(DIM), name);
        Rect::new(x, body.y + body.h * y, w, body.h * ROW_H)
    };
    let look = Look { radius: 8.0, fill: Some(SURFACE), inset: 0.0, ring_inset: -2.0 };
    let gap = pad * 0.5;

    let r = row(f, POWER_Y, "Power");
    let n = Power::ALL.len() as f32;
    let bw = (r.w - (n - 1.0) * gap) / n;
    for (i, power) in Power::ALL.into_iter().enumerate() {
        let b = Rect::new(r.x + i as f32 * (bw + gap), r.y, bw, r.h);
        let chosen = power == current.power;
        f.button(Id::Power(power), b, look, |c| {
            if chosen {
                c.outline(b, 8.0, 2.0, rgb(game.color));
            }
            label(c, b, b.h * BUTTON_TEXT, if chosen { TEXT_HI } else { TEXT }, power.label());
        });
    }

    let r = row(f, FPS_Y, "FPS cap");
    let n = FPS_CAPS.len() as f32;
    let bw = (r.w - (n - 1.0) * gap) / n;
    for (i, cap) in FPS_CAPS.into_iter().enumerate() {
        let b = Rect::new(r.x + i as f32 * (bw + gap), r.y, bw, r.h);
        let chosen = cap == current.fps_cap;
        let text = match cap {
            0 => "Off",
            30 => "30",
            40 => "40",
            60 => "60",
            90 => "90",
            _ => "120",
        };
        f.button(Id::FpsCap(cap), b, look, |c| {
            if chosen {
                c.outline(b, 8.0, 2.0, rgb(game.color));
            }
            label(c, b, b.h * BUTTON_TEXT, if chosen { TEXT_HI } else { TEXT }, text);
        });
    }

    if !running {
        let launch = Rect::new(x, body.y + body.h * LAUNCH_Y, w, body.h * LAUNCH_H);
        let look = Look { radius: 10.0, fill: None, inset: 0.0, ring_inset: -2.0 };
        f.button(Id::Launch, launch, look, |c| {
            c.outline(launch, 10.0, 2.0, rgb(GO));
            label(c, launch, launch.h * BUTTON_TEXT, GO, "Launch");
        });
    }
}
