//! The keyboard tab: the PC-style layout from `core`, drawn as black cells without gaps. Each key shows
//! its main function, with its Shift character small above and its Fn function small below in blue. The
//! active layer's function moves to the middle, also while a modifier is only held. Modifiers that are on
//! (armed or held) are amber, and Caps Lock has a light in its corner. Backspace is red and Enter amber.

use clamshell_core::Id;
use clamshell_core::bottom::keyboard::{self, Key, KeyPos, Keyboard, LAYOUT, Modifier, Named, Slot};

use crate::gfx::icons::Icon;
use crate::gfx::{Align, Canvas, Rect};
use crate::toolkit::Frame;
use crate::toolkit::style::{Look, Tone};

/// Label sizes as fractions of a key's height: the main label, and the small ones above and below.
const MAIN_TEXT: f32 = 0.3;
const SMALL_TEXT: f32 = 0.17;
/// The main label size of named keys (Tab, Esc, Ctrl), as a fraction of `MAIN_TEXT`.
const NAMED_TEXT: f32 = 0.7;
/// How far the small labels sit above and below the key's centre, as fractions of its height. Below is a
/// little further, to clear descenders like y, p and j.
const ABOVE: f32 = 0.26;
const BELOW: f32 = 0.31;
/// Icon size, as a fraction of the label size.
const ICON: f32 = 1.4;
/// Caps Lock's light: its radius and the radius of the glow around it, and its offset from the key's top
/// right corner, all as fractions of the key's height.
const LED: f32 = 0.06;
const GLOW: f32 = 0.13;
const LED_AT: f32 = 0.2;
/// The margin at a key's sides, as a fraction of its height. Labels shrink to fit inside it.
const SIDE: f32 = 0.08;
/// How strongly Caps Lock's glow shows.
const GLOW_ALPHA: f32 = 0.18;

/// Each key's position, slot and cell. The rows split `body` evenly, and each row fills the width, its
/// keys sharing it in proportion to their widths.
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
        slots
            .iter()
            .zip(starts)
            .enumerate()
            .map(move |(col, (slot, x))| (KeyPos { row: row as u8, col: col as u8 }, slot, Rect::new(x, y, slot.width * unit, row_h)))
    })
}

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let kb = &f.s.keyboard();
    // The cell is also the tap area: a touch between labels goes to the nearer key.
    for (pos, slot, cell) in cells(body) {
        f.button(Id::Key(pos), cell, Look::KEY, |f| draw_key(f.c, kb, slot, cell));
    }
}

/// Where a key label goes: big in the middle, or small above it (the Shift layer) or below it (Fn). All
/// are centred, so that each key reads as one column even without lines between keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Center,
    Top,
    Bottom,
}

/// A key's label: the key, and whether it shows its Shift character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Label {
    key: Key,
    shifted: bool,
}

impl Label {
    fn plain(key: Key) -> Label {
        Label { key, shifted: false }
    }
}

/// What a key shows for the keyboard's current state: in the middle, small above (the Shift layer) and
/// small below (Fn).
struct KeyFace {
    main: Label,
    above: Option<Label>,
    below: Option<Label>,
}

fn face(kb: &Keyboard, slot: &Slot) -> KeyFace {
    // Letters have no Shift label: they change case.
    let shift_char = match slot.base {
        Key::Char(ch) if !ch.is_ascii_alphabetic() && keyboard::shifted(ch) != ch => Some(ch),
        _ => None,
    };
    let fn_label = slot.fn_key.map(Label::plain);
    match (kb.latch(Modifier::Fn).on(), slot.fn_key, kb.latch(Modifier::Shift).on(), shift_char) {
        (true, Some(fn_key), _, _) => KeyFace { main: Label::plain(fn_key), above: None, below: Some(Label::plain(slot.base)) },
        (_, _, true, Some(ch)) => KeyFace { main: Label { key: Key::Char(ch), shifted: true }, above: Some(Label::plain(Key::Char(ch))), below: fn_label },
        _ => KeyFace {
            main: Label { key: slot.base, shifted: kb.shifts(slot.base) },
            above: shift_char.map(|ch| Label { key: Key::Char(ch), shifted: true }),
            below: fn_label,
        },
    }
}

fn draw_key(c: &mut Canvas, kb: &Keyboard, slot: &Slot, r: Rect) {
    let fn_on = kb.latch(Modifier::Fn).on();
    let face = face(kb, slot);
    let mut tone = match face.main.key {
        Key::Char(_) => Tone::Normal,
        Key::Named(Named::Backspace) => Tone::Stop,
        Key::Named(Named::Enter) => Tone::Accent,
        _ if fn_on && slot.fn_key.is_some() => Tone::Fn,
        Key::Mod(Modifier::Fn) => Tone::Fn,
        _ => Tone::Dim,
    };
    if let Key::Mod(m) = slot.base
        && kb.latch(m).on()
    {
        tone = Tone::Accent;
    }
    if slot.base == Key::Caps {
        // The light: a dim ring when off, and lit with a soft glow when on.
        let (lx, ly) = (r.x + r.w - r.h * LED_AT, r.y + r.h * LED_AT);
        if kb.caps() {
            c.circle(lx, ly, r.h * GLOW, Tone::Accent.rgb().alpha(GLOW_ALPHA));
            c.circle(lx, ly, r.h * LED, Tone::Accent);
        } else {
            let d = r.h * LED * 2.0;
            c.outline(Rect::new(lx - d / 2.0, ly - d / 2.0, d, d), d / 2.0, 1.2, Tone::Dim);
        }
    }
    key_label(c, face.main, r, r.h * MAIN_TEXT, tone, Place::Center);
    if let Some(above) = face.above {
        key_label(c, above, r, r.h * SMALL_TEXT, Tone::Dim, Place::Top);
    }
    if let Some(below) = face.below {
        let tone = if fn_on { Tone::Dim } else { Tone::Fn };
        key_label(c, below, r, r.h * SMALL_TEXT, tone, Place::Bottom);
    }
}

/// A key's label at `place`.
fn key_label(c: &mut Canvas, Label { key, shifted }: Label, r: Rect, size: f32, tone: Tone, place: Place) {
    let side = r.h * SIDE;
    let cy = r.y + r.h / 2.0;
    let icon_box = |h: f32| match place {
        Place::Center => Rect::new(r.x, r.y + (r.h - h) / 2.0, r.w, h),
        Place::Top => Rect::new(r.x + (r.w - h) / 2.0, cy - ABOVE * r.h - h / 2.0, h, h),
        Place::Bottom => Rect::new(r.x + (r.w - h) / 2.0, cy + BELOW * r.h - h / 2.0, h, h),
    };
    let text = |c: &mut Canvas, size: f32, t: &str| {
        // Shrink labels that don't fit the key (PgUp, Home on narrow keys).
        let room = r.w - 2.0 * side;
        let width = c.text_width(size, t);
        let size = if width > room { size * room / width } else { size };
        let w = c.text_width(size, t);
        let x = r.x + (r.w - w) / 2.0;
        match place {
            Place::Center => {
                c.text_in(r, size, tone, t, Align::Center);
            }
            // Centred on their line above or below the middle.
            Place::Top => {
                c.text(x, cy - ABOVE * r.h - size * 0.6, size, tone, t);
            }
            Place::Bottom => {
                c.text(x, cy + BELOW * r.h - size * 0.6, size, tone, t);
            }
        }
    };
    let icon = |c: &mut Canvas, i: Icon| c.icon(i, icon_box(size * ICON), tone);
    let named = if place == Place::Center { size * NAMED_TEXT } else { size };
    match key {
        Key::Char(' ') if place == Place::Center => {
            c.fill(Rect::new(r.x + r.w * 0.3, r.y + r.h * 0.62, r.w * 0.4, 2.0), Tone::Dim);
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
        Key::Caps => text(c, named, "Caps"),
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
