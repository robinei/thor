//! Settings: the global settings every game runs with, and the game in context's overrides (the running
//! game, else shelf's selection). Tabs choose which; the game's tab shows its effective values, marking
//! the ones that come from the global settings, with Reset to drop its overrides. Changes reach a running
//! game at once. A Launch button while the game isn't running.

use sysprog_core::{FPS_CAPS, Id, Power, Scope};

use super::{ACCENT, DIM, Frame, GO, Look, SURFACE, TEXT, TEXT_HI, label};
use crate::draw::{Rect, rgb};

/// Padding, of the area's height.
const PAD: f32 = 0.045;
/// Vertical positions and sizes, of the area's height.
const TABS_H: f32 = 0.12;
const TAB_TEXT: f32 = 0.05;
const NOTE: f32 = 0.036;
const SECTION: f32 = 0.04;
const ROW_H: f32 = 0.13;
const POWER_Y: f32 = 0.26;
const FPS_Y: f32 = 0.5;
const ACTIONS_Y: f32 = 0.76;
const ACTIONS_H: f32 = 0.15;
/// Label size, of a button's height.
const BUTTON_TEXT: f32 = 0.32;
/// The tab label's width at most, of the area's width (long titles are cut).
const TAB_MAX: f32 = 0.5;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let s = f.s;
    let context = s.context();
    let game = s.editing();
    let pad = body.h * PAD;
    let x = body.x + pad;
    let w = body.w - 2.0 * pad;

    // Tabs: All games, then the game in context.
    let size = body.h * TAB_TEXT;
    let tabs_h = body.h * TABS_H;
    let mut tx = x - pad * 0.6;
    let tabs = [(Scope::Global, Some("All games")), (Scope::Game, context.map(|g| g.title.as_str()))];
    for (scope, name) in tabs {
        let Some(name) = name else {
            continue;
        };
        let current = s.scope == scope || context.is_none();
        let text_w = f.c.text_width(size, name).min(w * TAB_MAX);
        let r = Rect::new(tx, body.y, text_w + pad * 1.2, tabs_h);
        let look = Look { radius: 6.0, fill: None, inset: 4.0, ring_inset: 2.0 };
        f.button(Id::Scope(scope), r, look, |c| {
            let color = if current { TEXT_HI } else { DIM };
            let ty = r.y + (r.h - size * 1.2) / 2.0;
            super::wrap(c, (r.x + pad * 0.6, ty, text_w + 1.0), size, color, name, 1);
            if current {
                c.fill(Rect::new(r.x + pad * 0.6, r.y + r.h - 4.0, text_w, 2.0), rgb(ACCENT));
            }
        });
        tx += r.w + pad * 0.4;
    }
    let running = context.is_some_and(|g| s.running.as_ref() == Some(&g.id));
    let note = match (game, running) {
        (Some(_), true) => "applies now",
        (Some(_), false) => "applies at launch",
        (None, _) => "every game's defaults",
    };
    let note_w = f.c.text_width(body.h * NOTE, note);
    if tx + pad + note_w < x + w {
        f.c.text(x + w - note_w, body.y + (tabs_h - body.h * NOTE * 1.2) / 2.0, body.h * NOTE, rgb(DIM), note);
    }

    let (current, own) = match game {
        Some(g) => (s.settings_of(&g.id), s.overrides_of(&g.id)),
        None => (s.global, sysprog_core::Overrides { power: Some(s.global.power), fps_cap: Some(s.global.fps_cap) }),
    };
    let row = |f: &mut Frame, y: f32, name: &str, inherited: bool| {
        let ny = body.y + body.h * y - body.h * SECTION * 1.5;
        let nw = f.c.text(x, ny, body.h * SECTION, rgb(DIM), name);
        if inherited {
            f.c.text(x + nw + pad * 0.5, ny, body.h * SECTION, rgb(SURFACE_TEXT), "· from All games");
        }
        Rect::new(x, body.y + body.h * y, w, body.h * ROW_H)
    };
    let look = Look { radius: 8.0, fill: Some(SURFACE), inset: 0.0, ring_inset: -2.0 };
    let gap = pad * 0.5;
    // The chosen value: ringed bright when chosen here, dim when it comes from the global settings.
    let ring = |set_here: bool| if set_here { TEXT_HI } else { DIM };

    let r = row(f, POWER_Y, "Power", own.power.is_none());
    let n = Power::ALL.len() as f32;
    let bw = (r.w - (n - 1.0) * gap) / n;
    for (i, power) in Power::ALL.into_iter().enumerate() {
        let b = Rect::new(r.x + i as f32 * (bw + gap), r.y, bw, r.h);
        let chosen = power == current.power;
        f.button(Id::Power(power), b, look, |c| {
            if chosen {
                c.outline(b, 8.0, 2.0, rgb(ring(own.power.is_some())));
            }
            label(c, b, b.h * BUTTON_TEXT, if chosen { TEXT_HI } else { TEXT }, power.label());
        });
    }

    let r = row(f, FPS_Y, "FPS cap", own.fps_cap.is_none());
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
                c.outline(b, 8.0, 2.0, rgb(ring(own.fps_cap.is_some())));
            }
            label(c, b, b.h * BUTTON_TEXT, if chosen { TEXT_HI } else { TEXT }, text);
        });
    }

    // Reset (when the game has overrides) and Launch (when it isn't running) share the bottom row.
    let y = body.y + body.h * ACTIONS_Y;
    let h = body.h * ACTIONS_H;
    let has_overrides = game.is_some_and(|g| !s.overrides_of(&g.id).is_empty());
    let launchable = context.is_some() && !running;
    let reset_w = if launchable { (w - gap) / 3.0 } else { w };
    if has_overrides {
        let r = Rect::new(x, y, reset_w, h);
        f.button(Id::Reset, r, Look { radius: 10.0, fill: Some(SURFACE), inset: 0.0, ring_inset: -2.0 }, |c| {
            label(c, r, r.h * BUTTON_TEXT, TEXT, "Reset");
        });
    }
    if launchable {
        let lx = if has_overrides { x + reset_w + gap } else { x };
        let launch = Rect::new(lx, y, x + w - lx, h);
        f.button(Id::Launch, launch, Look { radius: 10.0, fill: None, inset: 0.0, ring_inset: -2.0 }, |c| {
            c.outline(launch, 10.0, 2.0, rgb(GO));
            label(c, launch, launch.h * BUTTON_TEXT, GO, "Launch");
        });
    }
}

/// Barely-there text (the "from All games" note).
const SURFACE_TEXT: u32 = 0x3c3c44;
