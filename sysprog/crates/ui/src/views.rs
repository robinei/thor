//! The screens. Each is a function of the state and its size that draws everything and returns what can
//! be tapped or focused (immediate mode; docs/plans.md, "UI code").
//!
//! OLED-friendly: black background, grey text and lines (no full white on static elements), colour only
//! where it means something (focus, the active mode, the game's own colour, Launch).

use sysprog_core::keyboard::{self, Key, KeyPos, Keyboard, LAYOUT, Latch, Modifier, Named, Slot};
use sysprog_core::{Id, Mode, Power, State};

use crate::draw::{Canvas, Rect, rgb};
use crate::icons::{self, Icon};

/// Something drawn that can be tapped or focused, in the view's logical pixels.
#[derive(Debug, Clone)]
pub struct Target {
    pub rect: Rect,
    pub id: Id,
}

const BG: u32 = 0x000000;
/// Fills of keys, buttons and the selected row: barely above black.
const SURFACE: u32 = 0x131316;
const SURFACE_HI: u32 = 0x1f1f24;
const TEXT: u32 = 0xa4a4ae;
const TEXT_HI: u32 = 0xc8c8d0;
const DIM: u32 = 0x5a5a64;
const ACCENT: u32 = 0xe0a040;
const GO: u32 = 0x45a866;

fn focus_ring(c: &mut Canvas, s: &State, id: &Id, r: Rect) {
    if s.focus.as_ref() == Some(id) {
        c.outline(r.inset(-2.0), 8.0, 2.0, rgb(ACCENT));
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
    let size = row_h * 0.34;
    for (i, game) in s.games.iter().enumerate() {
        let r = Rect::new(pad, pad + i as f32 * row_h, list_w - 2.0 * pad, row_h - pad * 0.5);
        if r.y + r.h > h {
            break;
        }
        let selected = s.selected.as_ref() == Some(&game.id);
        if selected {
            c.rounded(r, 8.0, rgb(SURFACE_HI));
        }
        let swatch = Rect::new(r.x + pad * 0.5, r.y + r.h * 0.3, r.h * 0.4, r.h * 0.4);
        c.rounded(swatch, 3.0, rgb(game.color));
        let text_x = swatch.x + swatch.w + pad * 0.7;
        let color = if selected { TEXT_HI } else { TEXT };
        c.text(text_x, r.y + (r.h - size * 1.2) / 2.0, size, rgb(color), &game.title);
        let id = Id::Game(game.id.clone());
        focus_ring(c, s, &id, r);
        targets.push(Target { rect: r, id });
    }
    if let Some(game) = s.selected_game() {
        let art = Rect::new(list_w + pad, pad, w - list_w - 2.0 * pad, h * 0.66);
        c.rounded(art, 12.0, rgb(game.color));
        let mut y = art.y + art.h + pad;
        c.text(art.x, y, h * 0.055, rgb(TEXT_HI), &game.title);
        y += h * 0.085;
        let power = s.power_of(&game.id).label();
        c.text(art.x, y, h * 0.032, rgb(DIM), &format!("Power: {power}"));
    }
    targets
}

fn mode_icon(mode: Mode) -> Icon {
    match mode {
        Mode::Follow => Icon::Gamepad,
        Mode::Launcher => Icon::Apps,
        Mode::Keyboard => Icon::Keyboard,
        Mode::Touchpad => Icon::Touchpad,
        Mode::Sysinfo => Icon::Sysinfo,
    }
}

/// The second screen: a thin bar of mode icons and the clock, then the current mode.
pub fn bottom(c: &mut Canvas, s: &State, w: f32, h: f32) -> Vec<Target> {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    let mut targets = Vec::new();
    let bar_h = (h * 0.075).max(20.0);
    let icon = bar_h * 0.62;
    let clock = clock(s.unix_secs);
    let size = bar_h * 0.42;
    let clock_w = c.text_width(size, &clock) + bar_h * 0.8;
    // The mode icons share the width left of the clock.
    let slot = (w - clock_w) / Mode::ALL.len() as f32;
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        let r = Rect::new(i as f32 * slot, 0.0, slot, bar_h);
        let active = s.mode == mode;
        let color = if active { ACCENT } else { DIM };
        let (cx, cy) = r.center();
        icons::draw(c, mode_icon(mode), Rect::new(cx - icon / 2.0, cy - icon / 2.0, icon, icon), rgb(color));
        let id = Id::Mode(mode);
        focus_ring(c, s, &id, r.inset(2.0));
        targets.push(Target { rect: r, id });
    }
    label(c, Rect::new(w - clock_w, 0.0, clock_w, bar_h), size, TEXT, &clock);

    let body = Rect::new(0.0, bar_h, w, h - bar_h);
    match s.mode {
        Mode::Follow => game_settings(c, s, body, &mut targets),
        Mode::Keyboard => keyboard(c, s, body, &mut targets),
        other => label(c, body, body.h * 0.05, DIM, &format!("{other:?}: not built yet")),
    }
    targets
}

/// Follow mode under shelf: the selected game's settings and the launch button.
fn game_settings(c: &mut Canvas, s: &State, body: Rect, targets: &mut Vec<Target>) {
    let Some(game) = s.selected_game() else {
        return;
    };
    let pad = body.h * 0.05;
    c.text(pad, body.y + pad, body.h * 0.07, rgb(TEXT_HI), &game.title);
    c.text(pad, body.y + body.h * 0.2, body.h * 0.045, rgb(DIM), "Power");

    let row = Rect::new(pad, body.y + body.h * 0.29, body.w - 2.0 * pad, body.h * 0.16);
    let n = Power::ALL.len() as f32;
    let button_w = (row.w - (n - 1.0) * pad) / n;
    let current = s.power_of(&game.id);
    for (i, power) in Power::ALL.into_iter().enumerate() {
        let r = Rect::new(row.x + i as f32 * (button_w + pad), row.y, button_w, row.h);
        let chosen = power == current;
        c.rounded(r, 8.0, rgb(SURFACE));
        if chosen {
            c.outline(r, 8.0, 2.0, rgb(game.color));
        }
        label(c, r, row.h * 0.3, if chosen { TEXT_HI } else { TEXT }, power.label());
        let id = Id::Power(power);
        focus_ring(c, s, &id, r);
        targets.push(Target { rect: r, id });
    }

    let launch = Rect::new(pad, body.y + body.h * 0.62, body.w - 2.0 * pad, body.h * 0.2);
    c.outline(launch, 10.0, 2.0, rgb(GO));
    label(c, launch, launch.h * 0.3, GO, "Launch");
    focus_ring(c, s, &Id::Launch, launch);
    targets.push(Target { rect: launch, id: Id::Launch });
}

/// Keyboard mode: what was typed lately, then the keys. Each key shows its Fn function small in the
/// corner; while Fn is on, the two swap.
fn keyboard(c: &mut Canvas, s: &State, body: Rect, targets: &mut Vec<Target>) {
    let kb = &s.keyboard;
    let gap = body.h * 0.012;
    let echo_h = body.h * 0.09;
    let echo_size = echo_h * 0.45;
    let echo = format!("{}_", kb.echo);
    let ew = c.text_width(echo_size, &echo);
    // Keep the end of the line visible: draw from the right edge back when it is too long.
    let ex = if ew > body.w - 4.0 * gap { body.w - 2.0 * gap - ew } else { 2.0 * gap };
    c.text(ex, body.y + (echo_h - echo_size * 1.2) / 2.0, echo_size, rgb(DIM), &echo);

    let area = Rect::new(gap, body.y + echo_h, body.w - 2.0 * gap, body.h - echo_h - gap);
    let row_h = area.h / LAYOUT.len() as f32;
    for (row, slots) in LAYOUT.iter().enumerate() {
        // Every row fills the width: its keys share it by their widths.
        let units: f32 = slots.iter().map(|k| k.width).sum();
        let unit = area.w / units.max(1.0);
        let mut x = area.x;
        let y = area.y + row as f32 * row_h;
        for (col, slot) in slots.iter().enumerate() {
            let r = Rect::new(x + gap / 2.0, y + gap / 2.0, slot.width * unit - gap, row_h - gap);
            draw_key(c, kb, slot, r);
            let id = Id::Key(KeyPos { row: row as u8, col: col as u8 });
            focus_ring(c, s, &id, r);
            targets.push(Target { rect: r, id });
            x += slot.width * unit;
        }
    }
}

fn draw_key(c: &mut Canvas, kb: &Keyboard, slot: &Slot, r: Rect) {
    c.rounded(r, 6.0, rgb(SURFACE));
    let (main, corner) = match (kb.fn_layer.on(), slot.fn_key) {
        (true, Some(fn_key)) => (fn_key, Some(slot.base)),
        (false, Some(fn_key)) => (slot.base, Some(fn_key)),
        (_, None) => (slot.base, None),
    };
    let mut color = if matches!(main, Key::Char(_)) { TEXT } else { DIM };
    if let Key::Mod(m) = slot.base {
        match kb.latch(m) {
            Latch::Off => {}
            Latch::Once => color = ACCENT,
            Latch::Locked => {
                color = ACCENT;
                c.outline(r, 6.0, 1.5, rgb(ACCENT));
            }
        }
    }
    key_label(c, kb, main, r, r.h * 0.3, color, false);
    if let Some(key) = corner {
        key_label(c, kb, key, r, r.h * 0.17, DIM, true);
    }
}

/// A key's label: centred, or small in the top-right corner.
fn key_label(c: &mut Canvas, kb: &Keyboard, key: Key, r: Rect, size: f32, color: u32, corner: bool) {
    let pad = r.h * 0.08;
    let icon_box = |h: f32| {
        if corner {
            Rect::new(r.x + r.w - h - pad, r.y + pad, h, h)
        } else {
            Rect::new(r.x, r.y + (r.h - h) / 2.0, r.w, h)
        }
    };
    let text = |c: &mut Canvas, size: f32, t: &str| {
        // Shrink labels that don't fit the key (PgUp, Home on narrow keys).
        let room = r.w - 2.0 * pad;
        let width = c.text_width(size, t);
        let size = if width > room { size * room / width } else { size };
        if corner {
            let w = c.text_width(size, t);
            c.text(r.x + r.w - w - pad, r.y + pad * 0.5, size, rgb(color), t);
        } else {
            label(c, r, size, color, t);
        }
    };
    let icon = |c: &mut Canvas, i: Icon| icons::draw(c, i, icon_box(size * 1.4), rgb(color));
    match key {
        Key::Char(' ') if !corner => c.fill(Rect::new(r.x + r.w * 0.3, r.y + r.h * 0.62, r.w * 0.4, 2.0), rgb(DIM)),
        Key::Char(ch) => {
            let shown = if kb.shift.on() { keyboard::shifted(ch) } else { ch };
            text(c, size, shown.encode_utf8(&mut [0; 4]));
        }
        Key::Named(Named::Backspace) => icon(c, Icon::Backspace),
        Key::Named(Named::Enter) => icon(c, Icon::Enter),
        Key::Named(Named::Up) => icon(c, Icon::ArrowUp),
        Key::Named(Named::Down) => icon(c, Icon::ArrowDown),
        Key::Named(Named::Left) => icon(c, Icon::ArrowLeft),
        Key::Named(Named::Right) => icon(c, Icon::ArrowRight),
        Key::Mod(Modifier::Shift) => icon(c, Icon::Shift),
        Key::Named(named) => {
            let small = if corner { size } else { size * 0.7 };
            text(c, small, &named_label(named));
        }
        Key::Mod(m) => {
            let small = if corner { size } else { size * 0.7 };
            let t = match m {
                Modifier::Shift => "Shift",
                Modifier::Ctrl => "Ctrl",
                Modifier::Alt => "Alt",
                Modifier::Fn => "Fn",
            };
            text(c, small, t);
        }
    }
}

fn named_label(n: Named) -> String {
    match n {
        Named::F(i) => format!("F{i}"),
        Named::Backspace => "Bksp".into(),
        Named::Tab => "Tab".into(),
        Named::Enter => "Enter".into(),
        Named::Esc => "Esc".into(),
        Named::Delete => "Del".into(),
        Named::Insert => "Ins".into(),
        Named::Home => "Home".into(),
        Named::End => "End".into(),
        Named::PageUp => "PgUp".into(),
        Named::PageDown => "PgDn".into(),
        Named::Left => "Left".into(),
        Named::Right => "Right".into(),
        Named::Up => "Up".into(),
        Named::Down => "Down".into(),
    }
}

fn clock(unix_secs: u64) -> String {
    i64::try_from(unix_secs)
        .ok()
        .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string())
        .unwrap_or_default()
}
