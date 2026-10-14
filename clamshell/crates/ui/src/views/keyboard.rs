//! Keyboard mode: the PC-style layout from `core`, as black cells without gaps. Each key shows its main
//! function, with its Shift character small above and its Fn function small below (in blue); the active
//! layer's function takes the middle.

use clamshell_core::Id;
use clamshell_core::keyboard::{self, Key, KeyPos, Keyboard, LAYOUT, Latch, Modifier, Named, Slot};

use super::{ACCENT, DIM, FN, Frame, Look, TEXT, label};
use crate::draw::{Canvas, Rect, rgb};
use crate::icons::Icon;

/// Lines between keys, if drawn at all (the labels alone are enough on black).
const SEPARATORS: bool = false;
const SEPARATOR: u32 = 0x24242c;
/// Label sizes, of a key's height: the main one and the small ones above and below.
const MAIN_TEXT: f32 = 0.3;
const SMALL_TEXT: f32 = 0.17;
/// Named keys' main label (Tab, Esc, Ctrl), of MAIN_TEXT.
const NAMED_TEXT: f32 = 0.7;
/// How far the small labels sit above and below the key's centre, of its height (a little more below,
/// clear of descenders like y, p and j).
const ABOVE: f32 = 0.26;
const BELOW: f32 = 0.31;
/// Icons, of their label's size; Caps Lock's height, of the key's.
const ICON: f32 = 1.4;
const CAPS: f32 = 0.42;
/// Side padding inside a key, of its height (labels shrink to fit).
const PAD: f32 = 0.08;
/// A key as tapped or focused: square cells; the focus ring and a lock's outline just inside the cell.
const KEY: Look = Look { radius: 0.0, fill: None, inset: 0.0, ring_inset: 3.0 };

/// Each key's position, slot and cell: rows split `body` evenly; each row fills the width, its keys
/// sharing it by their widths.
fn cells(body: Rect) -> impl Iterator<Item = (KeyPos, &'static Slot, Rect)> {
    let row_h = body.h / LAYOUT.len() as f32;
    LAYOUT.iter().enumerate().flat_map(move |(row, slots)| {
        let units: f32 = slots.iter().map(|k| k.width).sum();
        let unit = body.w / units.max(1.0);
        let y = body.y + row as f32 * row_h;
        let starts = slots.iter().scan(body.x, move |x, slot| {
            let start = *x;
            *x += slot.width * unit;
            Some(start)
        });
        slots.iter().zip(starts).enumerate().map(move |(col, (slot, x))| {
            (KeyPos { row: row as u8, col: col as u8 }, slot, Rect::new(x, y, slot.width * unit, row_h))
        })
    })
}

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let kb = &f.s.keyboard;
    // The cell is also the tap area: a touch between labels goes to the nearer key.
    for (pos, slot, cell) in cells(body) {
        f.button(Id::Key(pos), cell, KEY, |c| draw_key(c, kb, slot, cell));
    }
    if SEPARATORS {
        // Two buffer pixels on every boundary between keys, and above each row.
        for (pos, _, cell) in cells(body) {
            if pos.col == 0 {
                f.c.hline(body.x, body.x + body.w, cell.y, 2, rgb(SEPARATOR));
            } else {
                f.c.vline(cell.x, cell.y, cell.y + cell.h, 2, rgb(SEPARATOR));
            }
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

/// What a key shows where, for the keyboard's state: `(key, shifted)` for the middle, above and below.
type Labels = ((Key, bool), Option<(Key, bool)>, Option<(Key, bool)>);

fn labels(kb: &Keyboard, slot: &Slot) -> Labels {
    // Letters have no Shift label: they change case.
    let shift_char = match slot.base {
        Key::Char(ch) if !ch.is_ascii_alphabetic() && keyboard::shifted(ch) != ch => Some(ch),
        _ => None,
    };
    match (kb.fn_layer.on(), slot.fn_key, kb.shift == Latch::Once, shift_char) {
        (true, Some(fn_key), _, _) => ((fn_key, false), None, Some((slot.base, false))),
        (_, _, true, Some(ch)) => ((Key::Char(ch), true), Some((Key::Char(ch), false)), slot.fn_key.map(|k| (k, false))),
        _ => (
            (slot.base, kb.shifts(slot.base)),
            shift_char.map(|ch| (Key::Char(ch), true)),
            slot.fn_key.map(|k| (k, false)),
        ),
    }
}

fn draw_key(c: &mut Canvas, kb: &Keyboard, slot: &Slot, r: Rect) {
    let fn_on = kb.fn_layer.on();
    let (main, top, bottom) = labels(kb, slot);
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
                c.outline(r.inset(KEY.ring_inset), 6.0, 1.5, rgb(ACCENT));
            }
        }
    }
    let caps = slot.base == Key::Mod(Modifier::Shift) && kb.shift == Latch::Locked;
    if caps {
        let h = r.h * CAPS;
        c.icon(Icon::CapsLock, Rect::new(r.x, r.y + (r.h - h) / 2.0, r.w, h), rgb(color));
    } else {
        key_label(c, main, r, r.h * MAIN_TEXT, color, Place::Center);
    }
    if let Some(top) = top {
        key_label(c, top, r, r.h * SMALL_TEXT, DIM, Place::Top);
    }
    if let Some(bottom) = bottom {
        let color = if fn_on { DIM } else { FN };
        key_label(c, bottom, r, r.h * SMALL_TEXT, color, Place::Bottom);
    }
}

/// A key's label at `place`; `(key, shifted)`.
fn key_label(c: &mut Canvas, (key, shifted): (Key, bool), r: Rect, size: f32, color: u32, place: Place) {
    let pad = r.h * PAD;
    let cy = r.y + r.h / 2.0;
    let icon_box = |h: f32| match place {
        Place::Center => Rect::new(r.x, r.y + (r.h - h) / 2.0, r.w, h),
        Place::Top => Rect::new(r.x + (r.w - h) / 2.0, cy - ABOVE * r.h - h / 2.0, h, h),
        Place::Bottom => Rect::new(r.x + (r.w - h) / 2.0, cy + BELOW * r.h - h / 2.0, h, h),
    };
    let text = |c: &mut Canvas, size: f32, t: &str| {
        // Shrink labels that don't fit the key (PgUp, Home on narrow keys).
        let room = r.w - 2.0 * pad;
        let width = c.text_width(size, t);
        let size = if width > room { size * room / width } else { size };
        let w = c.text_width(size, t);
        let x = r.x + (r.w - w) / 2.0;
        match place {
            Place::Center => label(c, r, size, color, t),
            Place::Top => {
                c.text(x, cy - ABOVE * r.h - size * 0.6, size, rgb(color), t);
            }
            Place::Bottom => {
                c.text(x, cy + BELOW * r.h - size * 0.6, size, rgb(color), t);
            }
        }
    };
    let icon = |c: &mut Canvas, i: Icon| c.icon(i, icon_box(size * ICON), rgb(color));
    let named = if place == Place::Center { size * NAMED_TEXT } else { size };
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
        Key::Named(n) => text(c, named, named_label(n)),
        Key::Mod(m) => text(c, named, modifier_label(m)),
    }
}

fn modifier_label(m: Modifier) -> &'static str {
    match m {
        Modifier::Shift => "Shift",
        Modifier::Ctrl => "Ctrl",
        Modifier::Alt => "Alt",
        Modifier::Super => "Super",
        Modifier::Fn => "Fn",
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
