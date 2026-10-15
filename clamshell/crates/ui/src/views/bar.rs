//! The bar along the top of the second screen: the tabs (the modes), the open one lit; volume and
//! brightness, near the middle, where a slider has room on both sides of the finger; Bluetooth, Wi-Fi and
//! the battery (tabs too, that carry their status), all spread evenly; then, past a faint line, the clock
//! and the pad mouse toggle, which is a switch rather than a tab.
//!
//! While a finger is held on volume or brightness (or slides off it sideways), a slider is drawn over
//! the bar (the bar stays visible
//! under it): a track whose current level sits under the finger, and the level as a number beside it.

use std::fmt::Write;

use clamshell_core::controls::{Control, Slider};
use clamshell_core::{Id, Mode, State, Status};

use super::{ACCENT, DIM, Frame, Look, Scratch, TEXT, TEXT_HI, label};
use crate::draw::{Rect, rgb, rgba};
use crate::icons::Icon;

/// The bar's height, of the screen's (at least 20 pixels).
const BAR_H: f32 = 0.075;
/// Icon and clock text sizes, of the bar's height.
const ICON: f32 = 0.62;
const CLOCK_TEXT: f32 = 0.34;
/// Room around the clock text, of the bar's height.
const CLOCK_PAD: f32 = 0.5;
/// The pad mouse switch's tap slot, of the bar's height (the tabs share the room before the clock).
const SLOT: f32 = 1.15;
/// The line before the clock and the pad mouse toggle: its height, of the bar's, and how strongly it shows.
const DIVIDER: f32 = 0.4;
const DIVIDER_ALPHA: f32 = 0.35;
/// Battery at or below this (and not charging) shows in the accent colour.
const LOW_BATTERY: u8 = 15;
/// The slider: track and thumb, of the bar's height; the level's text size, of the bar's height, and
/// how far beside the thumb it sits, of the screen's width.
const TRACK: f32 = 0.08;
const THUMB: f32 = 0.24;
const LEVEL_TEXT: f32 = 0.5;
const LEVEL_AWAY: f32 = 0.12;
/// Behind the slider and its level: dark and see-through, so the bar shows under it, dimmed.
const SHADE: u32 = 0x1f1f24;
const SHADE_ALPHA: f32 = 0.85;

fn mode_icon(mode: Mode) -> Icon {
    match mode {
        Mode::Settings => Icon::Sliders,
        Mode::Dock => Icon::Apps,
        Mode::Keyboard => Icon::Keyboard,
        Mode::Touchpad => Icon::Touchpad,
        Mode::Sysinfo => Icon::Sysinfo,
        Mode::Volume => Icon::Volume { waves: 3, muted: false },
        Mode::Brightness => Icon::Brightness,
        Mode::Wifi => Icon::Wifi { bars: Some(3) },
        Mode::Battery => Icon::Battery { tenths: 10, charging: false },
        Mode::Bluetooth => Icon::Bluetooth { connected: false },
    }
}

pub(super) fn volume_icon(level: u8, muted: bool) -> Icon {
    let waves = match level {
        0 => 0,
        1..=33 => 1,
        34..=66 => 2,
        _ => 3,
    };
    Icon::Volume { waves, muted }
}

fn battery(status: &Status) -> Option<(Icon, u32)> {
    status.battery.map(|level| {
        let low = level <= LOW_BATTERY && !status.charging;
        let icon = Icon::Battery { tenths: ((u16::from(level) + 5) / 10) as u8, charging: status.charging };
        (icon, if low { ACCENT } else { TEXT })
    })
}

fn wifi_bars(quality: Option<u8>) -> Option<u8> {
    quality.map(|q| match q {
        0..=24 => 0,
        25..=49 => 1,
        50..=74 => 2,
        _ => 3,
    })
}

/// A tab's button in the bar: its id (volume and brightness are controls, which open their tab on a
/// tap), its icon (with its level or status), and its colour: lit while open, dim without a reading.
fn tab_button(s: &State, mode: Mode) -> (Id, Icon, u32) {
    let lit = |reading: bool| if s.mode == mode { ACCENT } else if reading { TEXT } else { DIM };
    match mode {
        Mode::Volume => {
            let volume = s.level(Control::Volume);
            (Id::Control(Control::Volume), volume_icon(volume.unwrap_or(0), s.status.muted), lit(volume.is_some()))
        }
        Mode::Brightness => (Id::Control(Control::Brightness), Icon::Brightness, lit(s.level(Control::Brightness).is_some())),
        Mode::Bluetooth => {
            let connected = s.bluetooth.connected() > 0;
            (Id::Mode(mode), Icon::Bluetooth { connected }, lit(s.bluetooth.powered && connected))
        }
        Mode::Wifi => {
            let bars = wifi_bars(s.status.wifi);
            (Id::Mode(mode), Icon::Wifi { bars }, lit(bars.is_some()))
        }
        // An empty, dim battery when there is none; low in the accent colour.
        Mode::Battery => {
            let (icon, color) = battery(&s.status).unwrap_or((Icon::Battery { tenths: 0, charging: false }, DIM));
            (Id::Mode(mode), icon, if s.mode == mode { ACCENT } else { color })
        }
        Mode::Settings | Mode::Dock | Mode::Keyboard | Mode::Touchpad | Mode::Sysinfo => {
            (Id::Mode(mode), mode_icon(mode), if s.mode == mode { ACCENT } else { DIM })
        }
    }
}

/// Draws the bar; returns its height.
pub(super) fn draw(f: &mut Frame, w: f32, h: f32, scratch: &mut Scratch) -> f32 {
    let bar_h = (h * BAR_H).max(20.0);
    let slot = bar_h * SLOT;
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    let icon_button = |f: &mut Frame, id: Id, r: Rect, icon: Icon, color: u32| {
        // (Smaller in a narrow window, so neighbours don't touch.)
        let side = (bar_h * ICON).min(r.w * 0.8);
        f.button(id, r, look, |c| {
            let (cx, cy) = r.center();
            c.icon(icon, Rect::new(cx - side / 2.0, cy - side / 2.0, side, side), rgb(color));
        });
    };
    // On the right: the pad mouse, the clock, and the line before them.
    let mouse_x = w - slot;
    let color = if f.s.pad_mouse { ACCENT } else { DIM };
    icon_button(f, Id::PadMouse, Rect::new(mouse_x, 0.0, slot, bar_h), Icon::Mouse, color);
    let clock = scratch.clock(f.s.unix_secs);
    let size = bar_h * CLOCK_TEXT;
    let clock_w = f.c.text_width(size, clock) + bar_h * CLOCK_PAD;
    let line_x = mouse_x - clock_w;
    label(f.c, Rect::new(line_x, 0.0, clock_w, bar_h), size, TEXT, clock);
    let line = bar_h * DIVIDER;
    f.c.fill(Rect::new(line_x - 0.5, (bar_h - line) / 2.0, 1.0, line), rgba(DIM, DIVIDER_ALPHA));

    // Before the line, every tab, evenly spread (volume and brightness land near the middle; their
    // slider makes do with the room on either side). Each opens its tab; volume and brightness, held or
    // slid, bring up the slider.
    let cell = line_x.max(0.0) / Mode::TABS.len() as f32;
    for (i, mode) in Mode::TABS.into_iter().enumerate() {
        let (id, icon, color) = tab_button(f.s, mode);
        icon_button(f, id, Rect::new(i as f32 * cell, 0.0, cell, bar_h), icon, color);
    }
    bar_h
}

/// The slider, while one moves: drawn last, over the bar and the mode below it.
pub(super) fn draw_slider_over(f: &mut Frame, w: f32, bar_h: f32, scratch: &mut Scratch) {
    if let Some(slider) = f.s.slider {
        draw_slider(f, slider, w, bar_h, scratch);
    }
}

/// The slider over the bar's row: the track (the level lit), the thumb under the finger, and the level
/// as a number below the bar, to the side away from the finger.
fn draw_slider(f: &mut Frame, slider: Slider, w: f32, bar_h: f32, scratch: &mut Scratch) {
    let x0 = slider.track_start();
    let cy = bar_h / 2.0;
    let track = bar_h * TRACK;
    let at = x0 + f32::from(slider.value) / 100.0 * slider.length;
    let thumb = bar_h * THUMB;
    let band = Rect::new(x0 - thumb * 2.0, cy - thumb * 1.8, slider.length + thumb * 4.0, thumb * 3.6);
    f.c.rounded(band, band.h / 2.0, rgba(SHADE, SHADE_ALPHA));
    f.c.rounded(Rect::new(x0, cy - track / 2.0, slider.length, track), track / 2.0, rgb(DIM));
    f.c.rounded(Rect::new(x0, cy - track, at - x0, track * 2.0), track, rgb(TEXT_HI));
    f.c.circle(at, cy, thumb, rgb(TEXT_HI));

    let side = bar_h * ICON;
    let icon = match slider.control {
        Control::Volume => volume_icon(slider.value, f.s.status.muted),
        Control::Brightness => Icon::Brightness,
    };
    scratch.text.clear();
    let _ = write!(scratch.text, "{}", slider.value);
    let size = bar_h * LEVEL_TEXT;
    let text_w = f.c.text_width(size, &scratch.text);
    let group_w = side * 1.3 + text_w;
    let away = w * LEVEL_AWAY;
    let gx = if at < w / 2.0 { at + away } else { at - away - group_w };
    let gy = bar_h * 1.3;
    // On a small card of its own, over whatever the mode shows there.
    let card = Rect::new(gx - side * 0.4, gy - side * 0.3, group_w + side * 0.8, side * 1.6);
    f.c.rounded(card, side * 0.3, rgba(SHADE, SHADE_ALPHA));
    f.c.icon(icon, Rect::new(gx, gy, side, side), rgb(TEXT));
    f.c.text(gx + side * 1.3, gy + (side - size * 1.2) / 2.0, size, rgb(TEXT_HI), &scratch.text);
}
