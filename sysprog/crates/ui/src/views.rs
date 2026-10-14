//! The screens. Each is a function of the state and its size that draws everything and lists what can
//! be tapped or focused (into a vector kept between frames) (immediate mode; docs/plans.md, "UI code").
//!
//! OLED-friendly: black background, grey text and lines (no full white on static elements), colour only
//! where it means something (focus, the active mode, the game's own colour, Launch).

use std::fmt::Write;

use sysprog_core::keyboard::{self, Key, KeyPos, Keyboard, LAYOUT, Latch, Modifier, Named, Slot};
use sysprog_core::{Id, Mode, Power, State};

use crate::draw::{Canvas, Rect, rgb};
use crate::icons::Icon;

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
/// A target under the finger.
const PRESSED: u32 = 0x3a3a44;
const TEXT: u32 = 0xa4a4ae;
const TEXT_HI: u32 = 0xc8c8d0;
const DIM: u32 = 0x5a5a64;
const ACCENT: u32 = 0xe0a040;
const GO: u32 = 0x45a866;
/// Fn-layer labels: a muted blue, to tell them from the Shift layer's grey.
const FN: u32 = 0x5f82b8;
/// Lines between keys, if drawn at all (the labels alone may be enough on black).
const SEPARATORS: bool = false;
const SEPARATOR: u32 = 0x24242c;

fn is_pressed(s: &State, id: &Id) -> bool {
    s.pressed.as_ref() == Some(id)
}

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
pub fn top(c: &mut Canvas, s: &State, w: f32, h: f32, targets: &mut Vec<Target>) {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    targets.clear();
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
        let id = Id::Game(game.id.clone());
        if is_pressed(s, &id) {
            c.rounded(r, 8.0, rgb(PRESSED));
        } else if selected {
            c.rounded(r, 8.0, rgb(SURFACE_HI));
        }
        let swatch = Rect::new(r.x + pad * 0.5, r.y + r.h * 0.3, r.h * 0.4, r.h * 0.4);
        c.rounded(swatch, 3.0, rgb(game.color));
        let text_x = swatch.x + swatch.w + pad * 0.7;
        let color = if selected { TEXT_HI } else { TEXT };
        c.text(text_x, r.y + (r.h - size * 1.2) / 2.0, size, rgb(color), &game.title);
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
        let x = art.x + c.text(art.x, y, h * 0.032, rgb(DIM), "Power: ");
        c.text(x, y, h * 0.032, rgb(DIM), power);
    }
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
/// Text kept between frames so drawing doesn't allocate: the clock, formatted once a minute.
#[derive(Debug, Default)]
pub struct Scratch {
    minute: Option<u64>,
    clock: String,
}

impl Scratch {
    fn clock(&mut self, unix_secs: u64) -> &str {
        let minute = unix_secs / 60;
        if self.minute != Some(minute) {
            self.minute = Some(minute);
            self.clock.clear();
            if let Some(t) = i64::try_from(unix_secs).ok().and_then(|secs| jiff::Timestamp::from_second(secs).ok()) {
                let local = t.to_zoned(jiff::tz::TimeZone::system());
                let _ = write!(self.clock, "{}", local.strftime("%H:%M"));
            }
        }
        &self.clock
    }
}

pub fn bottom(
    c: &mut Canvas,
    s: &State,
    w: f32,
    h: f32,
    scratch: &mut Scratch,
    targets: &mut Vec<Target>,
) {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    targets.clear();
    let bar_h = (h * 0.075).max(20.0);
    let icon = bar_h * 0.62;
    let clock = scratch.clock(s.unix_secs);
    let size = bar_h * 0.42;
    let clock_w = c.text_width(size, clock) + bar_h * 0.8;
    // The mode icons share the width left of the clock.
    let slot = (w - clock_w) / Mode::ALL.len() as f32;
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        let r = Rect::new(i as f32 * slot, 0.0, slot, bar_h);
        let active = s.mode == mode;
        let color = if active { ACCENT } else { DIM };
        let id = Id::Mode(mode);
        if is_pressed(s, &id) {
            c.rounded(r.inset(2.0), 8.0, rgb(PRESSED));
        }
        let (cx, cy) = r.center();
        c.icon(mode_icon(mode), Rect::new(cx - icon / 2.0, cy - icon / 2.0, icon, icon), rgb(color));
        focus_ring(c, s, &id, r.inset(2.0));
        targets.push(Target { rect: r, id });
    }
    label(c, Rect::new(w - clock_w, 0.0, clock_w, bar_h), size, TEXT, clock);

    let body = Rect::new(0.0, bar_h, w, h - bar_h);
    match s.mode {
        Mode::Follow => game_settings(c, s, body, targets),
        Mode::Keyboard => keyboard(c, s, body, targets),
        Mode::Launcher => label(c, body, body.h * 0.05, DIM, "Launcher: not built yet"),
        Mode::Touchpad => label(c, body, body.h * 0.05, DIM, "Touchpad: not built yet"),
        Mode::Sysinfo => label(c, body, body.h * 0.05, DIM, "Sysinfo: not built yet"),
    }
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
        let id = Id::Power(power);
        c.rounded(r, 8.0, rgb(if is_pressed(s, &id) { PRESSED } else { SURFACE }));
        if chosen {
            c.outline(r, 8.0, 2.0, rgb(game.color));
        }
        label(c, r, row.h * 0.3, if chosen { TEXT_HI } else { TEXT }, power.label());
        focus_ring(c, s, &id, r);
        targets.push(Target { rect: r, id });
    }

    let launch = Rect::new(pad, body.y + body.h * 0.62, body.w - 2.0 * pad, body.h * 0.2);
    if is_pressed(s, &Id::Launch) {
        c.rounded(launch, 10.0, rgb(PRESSED));
    }
    c.outline(launch, 10.0, 2.0, rgb(GO));
    label(c, launch, launch.h * 0.3, GO, "Launch");
    focus_ring(c, s, &Id::Launch, launch);
    targets.push(Target { rect: launch, id: Id::Launch });
}

/// Keyboard mode: what was typed lately, then the keys. Each key shows its Fn function small in the
/// corner; while Fn is on, the two swap.
fn keyboard(c: &mut Canvas, s: &State, body: Rect, targets: &mut Vec<Target>) {
    let kb = &s.keyboard;
    let row_h = body.h / LAYOUT.len() as f32;
    for (row, slots) in LAYOUT.iter().enumerate() {
        // Every row fills the width: its keys share it by their widths.
        let units: f32 = slots.iter().map(|k| k.width).sum();
        let unit = body.w / units.max(1.0);
        let y = body.y + row as f32 * row_h;
        let mut x = body.x;
        for (col, slot) in slots.iter().enumerate() {
            // Keys are black cells with no gaps; the cell is also the tap area, so a touch on a separator
            // goes to the nearer key.
            let cell = Rect::new(x, y, slot.width * unit, row_h);
            let id = Id::Key(KeyPos { row: row as u8, col: col as u8 });
            draw_key(c, kb, slot, cell, is_pressed(s, &id));
            targets.push(Target { rect: cell, id });
            x += cell.w;
        }
    }
    if SEPARATORS {
        // Two buffer pixels on every boundary between keys, and above the first row.
        for (row, slots) in LAYOUT.iter().enumerate() {
            let units: f32 = slots.iter().map(|k| k.width).sum();
            let unit = body.w / units.max(1.0);
            let y = body.y + row as f32 * row_h;
            c.hline(body.x, body.x + body.w, y, 2, rgb(SEPARATOR));
            let mut x = body.x;
            for slot in slots.iter().take(slots.len().saturating_sub(1)) {
                x += slot.width * unit;
                c.vline(x, y, y + row_h, 2, rgb(SEPARATOR));
            }
        }
    }
    for target in targets.iter().filter(|t| matches!(t.id, Id::Key(_))) {
        if s.focus.as_ref() == Some(&target.id) {
            c.outline(target.rect.inset(3.0), 6.0, 2.0, rgb(ACCENT));
        }
    }
}

/// Where a key label goes: big in the middle, or small above it (the Shift layer) or below it (Fn), all
/// centred, so each key reads as one column even without lines between keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Center,
    Top,
    Bottom,
}

fn draw_key(c: &mut Canvas, kb: &Keyboard, slot: &Slot, r: Rect, pressed: bool) {
    if pressed {
        c.fill(r, rgb(PRESSED));
    }
    // What each place shows: the active layer's function in the middle, the others above and below.
    // Letters have no Shift label: they change case.
    let shift_char = match slot.base {
        Key::Char(ch) if !ch.is_ascii_alphabetic() && keyboard::shifted(ch) != ch => Some(ch),
        _ => None,
    };
    let fn_on = kb.fn_layer.on();
    let shift_on = kb.shift == Latch::Once;
    let mut main = (slot.base, kb.shifts(slot.base));
    let mut top = shift_char.map(|ch| (Key::Char(ch), true));
    let mut bottom = slot.fn_key.map(|k| (k, false));
    if let (true, Some(fn_key)) = (fn_on, slot.fn_key) {
        main = (fn_key, false);
        bottom = Some((slot.base, false));
        top = None;
    } else if let (true, Some(ch)) = (shift_on, shift_char) {
        main = (Key::Char(ch), true);
        top = Some((Key::Char(ch), false));
    }

    let mut color = match main.0 {
        Key::Char(_) => TEXT,
        _ if fn_on && slot.fn_key.is_some() => FN,
        Key::Mod(Modifier::Fn) => FN,
        _ => DIM,
    };
    if let Key::Mod(m) = slot.base {
        match kb.latch(m) {
            Latch::Off => {}
            Latch::Once => color = ACCENT,
            Latch::Locked => {
                color = ACCENT;
                c.outline(r.inset(3.0), 6.0, 1.5, rgb(ACCENT));
            }
        }
    }
    let caps = slot.base == Key::Mod(Modifier::Shift) && kb.shift == Latch::Locked;
    if caps {
        c.icon(Icon::CapsLock, Rect::new(r.x, r.y + r.h * 0.29, r.w, r.h * 0.42), rgb(color));
    } else {
        key_label(c, main, r, r.h * 0.3, color, Place::Center);
    }
    if let Some(top) = top {
        key_label(c, top, r, r.h * 0.17, DIM, Place::Top);
    }
    if let Some(bottom) = bottom {
        let color = if fn_on { DIM } else { FN };
        key_label(c, bottom, r, r.h * 0.17, color, Place::Bottom);
    }
}

/// A key's label at `place`; `(key, shifted)`.
fn key_label(c: &mut Canvas, (key, shifted): (Key, bool), r: Rect, size: f32, color: u32, place: Place) {
    let pad = r.h * 0.08;
    let icon_box = |h: f32| match place {
        Place::Center => Rect::new(r.x, r.y + (r.h - h) / 2.0, r.w, h),
        Place::Top => Rect::new(r.x + (r.w - h) / 2.0, r.y + pad, h, h),
        Place::Bottom => Rect::new(r.x + (r.w - h) / 2.0, r.y + r.h - h - pad, h, h),
    };
    let text = |c: &mut Canvas, size: f32, t: &str| {
        // Shrink labels that don't fit the key (PgUp, Home on narrow keys).
        let room = r.w - 2.0 * pad;
        let width = c.text_width(size, t);
        let size = if width > room { size * room / width } else { size };
        let w = c.text_width(size, t);
        match place {
            Place::Center => label(c, r, size, color, t),
            Place::Top => {
                c.text(r.x + (r.w - w) / 2.0, r.y + pad * 0.5, size, rgb(color), t);
            }
            Place::Bottom => {
                c.text(r.x + (r.w - w) / 2.0, r.y + r.h - size * 1.3 - pad * 0.5, size, rgb(color), t);
            }
        }
    };
    let icon = |c: &mut Canvas, i: Icon| c.icon(i, icon_box(size * 1.4), rgb(color));
    let small = if place == Place::Center { size * 0.7 } else { size };
    match key {
        Key::Char(' ') if place == Place::Center => {
            c.fill(Rect::new(r.x + r.w * 0.3, r.y + r.h * 0.62, r.w * 0.4, 2.0), rgb(DIM));
        }
        Key::Char(ch) => {
            let shown = if shifted { keyboard::shifted(ch) } else { ch };
            text(c, size, shown.encode_utf8(&mut [0; 4]));
        }
        Key::Named(Named::Backspace) => icon(c, Icon::Backspace),
        Key::Named(Named::Enter) => icon(c, Icon::Enter),
        Key::Named(Named::Up) => icon(c, Icon::ArrowUp),
        Key::Named(Named::Down) => icon(c, Icon::ArrowDown),
        Key::Named(Named::Left) => icon(c, Icon::ArrowLeft),
        Key::Named(Named::Right) => icon(c, Icon::ArrowRight),
        Key::Mod(Modifier::Shift) => icon(c, Icon::Shift),
        Key::Named(named) => text(c, small, named_label(named)),
        Key::Mod(m) => {
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

fn named_label(n: Named) -> &'static str {
    const F: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
    match n {
        Named::F(i) => F.get(usize::from(i).wrapping_sub(1)).copied().unwrap_or("F?"),
        Named::Backspace => "Bksp",
        Named::Tab => "Tab",
        Named::Enter => "Enter",
        Named::Esc => "Esc",
        Named::Delete => "Del",
        Named::Insert => "Ins",
        Named::Home => "Home",
        Named::End => "End",
        Named::PageUp => "PgUp",
        Named::PageDown => "PgDn",
        Named::Left => "Left",
        Named::Right => "Right",
        Named::Up => "Up",
        Named::Down => "Down",
    }
}
