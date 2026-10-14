//! The screens. Each is a function of the state and its size that draws everything and returns what can
//! be tapped or focused (immediate mode; docs/plans.md, "UI code").
//!
//! OLED-friendly: black background, grey text and lines (no full white on static elements), colour only
//! where it means something (focus, the active mode, the game's own colour, Launch).

use sysprog_core::{Id, Key, KeyLayer, Mode, Power, State};

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

/// A key and its width in key units.
type KeySpec = (Key, f32);

const fn ch(c: char) -> KeySpec {
    (Key::Char(c), 1.0)
}

const LETTERS: &[&[KeySpec]] = &[
    &[ch('q'), ch('w'), ch('e'), ch('r'), ch('t'), ch('y'), ch('u'), ch('i'), ch('o'), ch('p')],
    &[ch('a'), ch('s'), ch('d'), ch('f'), ch('g'), ch('h'), ch('j'), ch('k'), ch('l')],
    &[(Key::Shift, 1.5), ch('z'), ch('x'), ch('c'), ch('v'), ch('b'), ch('n'), ch('m'), (Key::Backspace, 1.5)],
    &[(Key::Layer, 1.5), ch(','), (Key::Char(' '), 5.0), ch('.'), (Key::Enter, 1.5)],
];

/// Digits and all 32 ASCII symbols, with Esc and Tab for terminals.
const SYMBOLS: &[&[KeySpec]] = &[
    &[ch('1'), ch('2'), ch('3'), ch('4'), ch('5'), ch('6'), ch('7'), ch('8'), ch('9'), ch('0')],
    &[ch('!'), ch('@'), ch('#'), ch('$'), ch('%'), ch('^'), ch('&'), ch('*'), ch('('), ch(')')],
    &[ch('-'), ch('_'), ch('='), ch('+'), ch('['), ch(']'), ch('{'), ch('}'), ch('\\'), ch('|')],
    &[(Key::Esc, 1.5), ch(';'), ch(':'), ch('\''), ch('"'), ch('`'), ch('~'), ch('<'), ch('>'), (Key::Backspace, 1.5)],
    &[(Key::Layer, 1.5), (Key::Tab, 1.5), ch(','), ch('/'), (Key::Char(' '), 2.5), ch('?'), ch('.'), (Key::Enter, 1.5)],
];

/// Keyboard mode: what was typed lately, then the keys of the current layer.
fn keyboard(c: &mut Canvas, s: &State, body: Rect, targets: &mut Vec<Target>) {
    let kb = &s.keyboard;
    let rows = match kb.layer {
        KeyLayer::Letters => LETTERS,
        KeyLayer::Symbols => SYMBOLS,
    };
    let gap = body.h * 0.012;
    let echo_h = body.h * 0.1;
    let echo_size = echo_h * 0.45;
    let echo = format!("{}_", kb.echo);
    let ew = c.text_width(echo_size, &echo);
    // Keep the end of the line visible: draw from the right edge back when it is too long.
    let ex = if ew > body.w - 4.0 * gap { body.w - 2.0 * gap - ew } else { 2.0 * gap };
    c.text(ex, body.y + (echo_h - echo_size * 1.2) / 2.0, echo_size, rgb(DIM), &echo);

    let area = Rect::new(gap, body.y + echo_h, body.w - 2.0 * gap, body.h - echo_h - gap);
    let row_h = area.h / rows.len() as f32;
    for (i, row) in rows.iter().enumerate() {
        // Every row fills the width: its keys share it by their widths.
        let row_units: f32 = row.iter().map(|(_, w)| w).sum();
        let unit = area.w / row_units.max(1.0);
        let mut x = area.x;
        let y = area.y + i as f32 * row_h;
        for &(key, width) in row.iter() {
            let r = Rect::new(x + gap / 2.0, y + gap / 2.0, width * unit - gap, row_h - gap);
            draw_key(c, s, key, r);
            let id = Id::Key(key);
            focus_ring(c, s, &id, r);
            targets.push(Target { rect: r, id });
            x += width * unit;
        }
    }
}

fn draw_key(c: &mut Canvas, s: &State, key: Key, r: Rect) {
    let kb = &s.keyboard;
    c.rounded(r, 6.0, rgb(SURFACE));
    let size = r.h * 0.3;
    let small = r.h * 0.22;
    let icon = Rect::new(r.x, r.y + r.h * 0.28, r.w, r.h * 0.44);
    match key {
        Key::Char(' ') => c.fill(Rect::new(r.x + r.w * 0.3, r.y + r.h * 0.62, r.w * 0.4, 2.0), rgb(DIM)),
        Key::Char(ch) => {
            let shown = if kb.shift { ch.to_ascii_uppercase() } else { ch };
            label(c, r, size, TEXT, shown.encode_utf8(&mut [0; 4]));
        }
        Key::Shift => icons::draw(c, Icon::Shift, icon, rgb(if kb.shift { ACCENT } else { DIM })),
        Key::Backspace => icons::draw(c, Icon::Backspace, icon, rgb(DIM)),
        Key::Enter => icons::draw(c, Icon::Enter, icon, rgb(DIM)),
        Key::Layer => {
            let text = match kb.layer {
                KeyLayer::Letters => "?123",
                KeyLayer::Symbols => "ABC",
            };
            label(c, r, small, DIM, text);
        }
        Key::Tab => label(c, r, small, DIM, "Tab"),
        Key::Esc => label(c, r, small, DIM, "Esc"),
    }
}

fn clock(unix_secs: u64) -> String {
    i64::try_from(unix_secs)
        .ok()
        .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string())
        .unwrap_or_default()
}
