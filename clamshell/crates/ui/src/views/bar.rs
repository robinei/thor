//! The bar along the top of the second screen: the modes' icons on the left (the active one lit);
//! volume and brightness in the middle, where a slider has room on both sides of the finger; on the
//! right the pad mouse toggle, Bluetooth, Wi-Fi and the battery (each opens its mode, lit while open)
//! and the clock. Status icons carry their level.
//!
//! While a finger is on volume or brightness, a slider is drawn over the bar (the bar stays visible
//! under it): a track whose current level sits under the finger, and the level as a number beside it.

use std::fmt::Write;

use clamshell_core::controls::{Control, Slider};
use clamshell_core::{Id, Mode, Status};

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
/// A button's tap slot (modes, controls, toggles), of the bar's height.
const SLOT: f32 = 1.15;
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
        Mode::Wifi => Icon::Wifi { bars: Some(3) },
        Mode::Battery => Icon::Battery { tenths: 10, charging: false },
        Mode::Bluetooth => Icon::Bluetooth { connected: false },
    }
}

fn volume_icon(level: u8, muted: bool) -> Icon {
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

/// Draws the bar (and a slider over it while one moves); returns its height.
pub(super) fn draw(f: &mut Frame, w: f32, h: f32, scratch: &mut Scratch) -> f32 {
    let bar_h = (h * BAR_H).max(20.0);
    let side = bar_h * ICON;
    let slot = bar_h * SLOT;
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    let icon_button = |f: &mut Frame, id: Id, r: Rect, icon: Icon, color: u32| {
        f.button(id, r, look, |c| {
            let (cx, cy) = r.center();
            c.icon(icon, Rect::new(cx - side / 2.0, cy - side / 2.0, side, side), rgb(color));
        });
    };
    for (i, mode) in Mode::BAR.into_iter().enumerate() {
        let color = if f.s.mode == mode { ACCENT } else { DIM };
        icon_button(f, Id::Mode(mode), Rect::new(i as f32 * slot, 0.0, slot, bar_h), mode_icon(mode), color);
    }

    // Volume left of the middle, brightness right of it; dim without a reading (always shown, so a
    // desktop shows everything the device does).
    let volume = f.s.level(Control::Volume);
    let icon = volume_icon(volume.unwrap_or(0), f.s.status.muted);
    let color = if volume.is_some() { TEXT } else { DIM };
    icon_button(f, Id::Control(Control::Volume), Rect::new(w / 2.0 - slot, 0.0, slot, bar_h), icon, color);
    let color = if f.s.level(Control::Brightness).is_some() { TEXT } else { DIM };
    icon_button(f, Id::Control(Control::Brightness), Rect::new(w / 2.0, 0.0, slot, bar_h), Icon::Brightness, color);

    // From the right: the clock, the battery, Wi-Fi, the pad mouse.
    let clock = scratch.clock(f.s.unix_secs);
    let size = bar_h * CLOCK_TEXT;
    let clock_w = f.c.text_width(size, clock) + bar_h * CLOCK_PAD;
    label(f.c, Rect::new(w - clock_w, 0.0, clock_w, bar_h), size, TEXT, clock);
    let mut x = w - clock_w;
    // Opens the battery mode (lit while open); an empty, dim battery when there is none.
    let (icon, color) = battery(&f.s.status).unwrap_or((Icon::Battery { tenths: 0, charging: false }, DIM));
    let color = if f.s.mode == Mode::Battery { ACCENT } else { color };
    x -= slot;
    icon_button(f, Id::Mode(Mode::Battery), Rect::new(x, 0.0, slot, bar_h), icon, color);
    x -= slot;
    let bars = wifi_bars(f.s.status.wifi);
    let color = match (f.s.mode == Mode::Wifi, bars) {
        (true, _) => ACCENT,
        (false, Some(_)) => TEXT,
        (false, None) => DIM,
    };
    icon_button(f, Id::Mode(Mode::Wifi), Rect::new(x, 0.0, slot, bar_h), Icon::Wifi { bars }, color);
    x -= slot;
    let bt = &f.s.bluetooth;
    let connected = bt.connected() > 0;
    let color = match (f.s.mode == Mode::Bluetooth, bt.powered, connected) {
        (true, _, _) => ACCENT,
        (false, true, true) => TEXT,
        _ => DIM,
    };
    icon_button(f, Id::Mode(Mode::Bluetooth), Rect::new(x, 0.0, slot, bar_h), Icon::Bluetooth { connected }, color);
    x -= slot;
    let color = if f.s.pad_mouse { ACCENT } else { DIM };
    icon_button(f, Id::PadMouse, Rect::new(x, 0.0, slot, bar_h), Icon::Mouse, color);
    if let Some(slider) = f.s.slider {
        draw_slider(f, slider, w, bar_h, scratch);
    }
    bar_h
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
