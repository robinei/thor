//! The screens. Each is a function of the state and its size that draws everything and returns what can
//! be tapped or focused (immediate mode; docs/plans.md, "UI code").

use sysprog_core::{Id, Mode, Power, State};

use crate::draw::{Canvas, Rect, rgb};

/// Something drawn that can be tapped or focused, in the view's logical pixels.
#[derive(Debug, Clone)]
pub struct Target {
    pub rect: Rect,
    pub id: Id,
}

const BG: u32 = 0x101014;
const PANEL: u32 = 0x1c1c24;
const PANEL_HI: u32 = 0x30303e;
const TEXT: u32 = 0xe8e8f0;
const DIM: u32 = 0x8888a0;
const ACCENT: u32 = 0xf0c040;
const GO: u32 = 0x2f7a46;

fn focus_ring(c: &mut Canvas, s: &State, id: &Id, r: Rect) {
    if s.focus.as_ref() == Some(id) {
        c.outline(r.inset(-3.0), 10.0, 3.0, rgb(ACCENT));
    }
}

/// Text centred in `r`.
fn label(c: &mut Canvas, r: Rect, size: f32, color: u32, text: &str) {
    let w = c.text_width(size, text);
    c.text(r.x + (r.w - w) / 2.0, r.y + (r.h - size * 1.2) / 2.0, size, rgb(color), text);
}

/// shelf on the main screen: the game list and the selected game's art.
pub fn top(c: &mut Canvas, s: &State, w: f32, h: f32) -> Vec<Target> {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    let mut targets = Vec::new();
    let pad = (h * 0.03).max(4.0);
    let list_w = w * 0.4;
    let row_h = (h - pad) / 8.0;
    let size = row_h * 0.36;
    for (i, game) in s.games.iter().enumerate() {
        let r = Rect::new(pad, pad + i as f32 * row_h, list_w - 2.0 * pad, row_h - pad * 0.5);
        if r.y + r.h > h {
            break;
        }
        let selected = s.selected.as_ref() == Some(&game.id);
        c.rounded(r, 8.0, rgb(if selected { PANEL_HI } else { PANEL }));
        let swatch = Rect::new(r.x + pad * 0.5, r.y + r.h * 0.2, r.h * 0.6, r.h * 0.6);
        c.rounded(swatch, 4.0, rgb(game.color));
        let text_x = swatch.x + swatch.w + pad * 0.6;
        c.text(text_x, r.y + (r.h - size * 1.2) / 2.0, size, rgb(TEXT), &game.title);
        let id = Id::Game(game.id.clone());
        focus_ring(c, s, &id, r);
        targets.push(Target { rect: r, id });
    }
    if let Some(game) = s.selected_game() {
        let art = Rect::new(list_w + pad, pad, w - list_w - 2.0 * pad, h * 0.66);
        c.rounded(art, 12.0, rgb(game.color));
        let mut y = art.y + art.h + pad;
        c.text(art.x, y, h * 0.06, rgb(TEXT), &game.title);
        y += h * 0.09;
        let power = s.power_of(&game.id).label();
        c.text(art.x, y, h * 0.035, rgb(DIM), &format!("Power: {power}"));
    }
    targets
}

/// The second screen: the bar with the mode switcher and clock, then the current mode.
pub fn bottom(c: &mut Canvas, s: &State, w: f32, h: f32) -> Vec<Target> {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    let mut targets = Vec::new();
    let bar_h = (h * 0.14).max(24.0);
    c.fill(Rect::new(0.0, 0.0, w, bar_h), rgb(PANEL));
    let clock_w = w * 0.24;
    let button_w = (w - clock_w) / Mode::ALL.len() as f32;
    let size = bar_h * 0.34;
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        let r = Rect::new(i as f32 * button_w + 4.0, 4.0, button_w - 8.0, bar_h - 8.0);
        let active = s.mode == mode;
        if active {
            c.rounded(r, 6.0, rgb(PANEL_HI));
        }
        label(c, r, size, if active { ACCENT } else { TEXT }, mode.label());
        let id = Id::Mode(mode);
        focus_ring(c, s, &id, r);
        targets.push(Target { rect: r, id });
    }
    label(c, Rect::new(w - clock_w, 0.0, clock_w, bar_h), size, TEXT, &clock(s.unix_secs));

    let body = Rect::new(0.0, bar_h, w, h - bar_h);
    match s.mode {
        Mode::Follow => game_settings(c, s, body, &mut targets),
        other => label(c, body, body.h * 0.07, DIM, &format!("{} mode: not built yet", other.label())),
    }
    targets
}

/// Follow mode under shelf: the selected game's settings and the launch button.
fn game_settings(c: &mut Canvas, s: &State, body: Rect, targets: &mut Vec<Target>) {
    let Some(game) = s.selected_game() else {
        return;
    };
    let pad = body.h * 0.05;
    c.text(pad, body.y + pad, body.h * 0.08, rgb(TEXT), &game.title);
    c.text(pad, body.y + body.h * 0.22, body.h * 0.05, rgb(DIM), "Power");

    let row = Rect::new(pad, body.y + body.h * 0.32, body.w - 2.0 * pad, body.h * 0.18);
    let n = Power::ALL.len() as f32;
    let button_w = (row.w - (n - 1.0) * pad) / n;
    let current = s.power_of(&game.id);
    for (i, power) in Power::ALL.into_iter().enumerate() {
        let r = Rect::new(row.x + i as f32 * (button_w + pad), row.y, button_w, row.h);
        c.rounded(r, 8.0, rgb(if power == current { game.color } else { PANEL }));
        label(c, r, row.h * 0.32, TEXT, power.label());
        let id = Id::Power(power);
        focus_ring(c, s, &id, r);
        targets.push(Target { rect: r, id });
    }

    let launch = Rect::new(pad, body.y + body.h * 0.62, body.w - 2.0 * pad, body.h * 0.24);
    c.rounded(launch, 10.0, rgb(GO));
    label(c, launch, launch.h * 0.32, TEXT, "Launch");
    focus_ring(c, s, &Id::Launch, launch);
    targets.push(Target { rect: launch, id: Id::Launch });
}

fn clock(unix_secs: u64) -> String {
    i64::try_from(unix_secs)
        .ok()
        .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M:%S").to_string())
        .unwrap_or_default()
}
