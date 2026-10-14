//! The bar along the top of the second screen: the modes' icons on the left (the active one lit), then
//! the pad mouse toggle, the status on the right (Wi-Fi, volume, battery as icons that carry their
//! level, then the clock). The middle stays free for notices.

use clamshell_core::{Id, Mode, Status};

use super::{ACCENT, DIM, Frame, Look, Scratch, TEXT, label};
use crate::draw::{Rect, rgb};
use crate::icons::Icon;

/// The bar's height, of the screen's (at least 20 pixels).
const BAR_H: f32 = 0.075;
/// Icon and clock text sizes, of the bar's height.
const ICON: f32 = 0.62;
const CLOCK_TEXT: f32 = 0.42;
/// Room around the clock text, of the bar's height.
const CLOCK_PAD: f32 = 0.8;
/// A mode icon's tap slot, of the bar's height.
const MODE_SLOT: f32 = 1.6;
/// A status icon's width, of the icon size.
const STATUS_SLOT: f32 = 1.35;
/// Battery at or below this (and not charging) shows in the accent colour.
const LOW_BATTERY: u8 = 15;

fn mode_icon(mode: Mode) -> Icon {
    match mode {
        Mode::Settings => Icon::Sliders,
        Mode::Launcher => Icon::Apps,
        Mode::Keyboard => Icon::Keyboard,
        Mode::Touchpad => Icon::Touchpad,
        Mode::Sysinfo => Icon::Sysinfo,
    }
}

/// The status icons, right to left from the clock, with their colours.
fn status_icons(status: &Status) -> impl Iterator<Item = (Icon, u32)> {
    let battery = status.battery.map(|level| {
        let low = level <= LOW_BATTERY && !status.charging;
        let icon = Icon::Battery { tenths: ((u16::from(level) + 5) / 10) as u8, charging: status.charging };
        (icon, if low { ACCENT } else { TEXT })
    });
    let volume = status.volume.map(|v| {
        let waves = match v {
            0 => 0,
            1..=33 => 1,
            34..=66 => 2,
            _ => 3,
        };
        (Icon::Volume { waves, muted: status.muted }, TEXT)
    });
    let bars = status.wifi.map(|q| match q {
        0..=24 => 0,
        25..=49 => 1,
        50..=74 => 2,
        _ => 3,
    });
    let wifi = Some((Icon::Wifi { bars }, if bars.is_some() { TEXT } else { DIM }));
    [battery, volume, wifi].into_iter().flatten()
}

/// Draws the bar; returns its height.
pub(super) fn draw(f: &mut Frame, w: f32, h: f32, scratch: &mut Scratch) -> f32 {
    let bar_h = (h * BAR_H).max(20.0);
    let side = bar_h * ICON;
    let slot = bar_h * MODE_SLOT;
    let look = Look { radius: 8.0, fill: None, inset: 2.0, ring_inset: 0.0 };
    for (i, mode) in Mode::BAR.into_iter().enumerate() {
        let icon = mode_icon(mode);
        let r = Rect::new(i as f32 * slot, 0.0, slot, bar_h);
        let color = if f.s.mode == mode { ACCENT } else { DIM };
        f.button(Id::Mode(mode), r, look, |c| {
            let (cx, cy) = r.center();
            c.icon(icon, Rect::new(cx - side / 2.0, cy - side / 2.0, side, side), rgb(color));
        });
    }

    // The pad mouse toggle, a little apart from the modes: it combines with any of them.
    let r = Rect::new((Mode::BAR.len() as f32 + 0.5) * slot, 0.0, slot, bar_h);
    let color = if f.s.pad_mouse { ACCENT } else { DIM };
    f.button(Id::PadMouse, r, look, |c| {
        let (cx, cy) = r.center();
        c.icon(Icon::Mouse, Rect::new(cx - side / 2.0, cy - side / 2.0, side, side), rgb(color));
    });

    let clock = scratch.clock(f.s.unix_secs);
    let size = bar_h * CLOCK_TEXT;
    let clock_w = f.c.text_width(size, clock) + bar_h * CLOCK_PAD;
    label(f.c, Rect::new(w - clock_w, 0.0, clock_w, bar_h), size, TEXT, clock);
    let mut x = w - clock_w;
    for (icon, color) in status_icons(&f.s.status) {
        x -= side * STATUS_SLOT;
        let r = Rect::new(x, (bar_h - side) / 2.0, side * STATUS_SLOT, side);
        f.c.icon(icon, r, rgb(color));
    }
    bar_h
}
