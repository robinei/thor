//! The bar along the top of the second screen: a button per tab, the open one lit; volume and
//! brightness, near the middle, where a slider has room on both sides of the finger; Bluetooth, Wi-Fi and
//! the battery (tabs too, that carry their status), all spread evenly; then, past a faint line, the clock
//! and the pad mouse toggle, which is a switch rather than a tab.
//!
//! While a finger is held on volume or brightness (or slides off it sideways), a slider is drawn over
//! the bar (the bar stays visible under it): a track whose current level sits under the finger, and the
//! level as a number beside it.

use clamshell_core::hardware::NoticeKind;
use clamshell_core::slider::{Control, Slider};
use clamshell_core::{Id, State, Tab};

use super::format::Fmt;
use super::style::{Look, SURFACE_HI, Tone};
use super::{Frame, Track, widgets};
use crate::draw::{Align, Rect, Rgb};
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
/// The line before the clock and the pad mouse toggle: its height, of the bar's, and how strongly it
/// shows.
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
const SHADE: Rgb = SURFACE_HI;
const SHADE_ALPHA: f32 = 0.85;

/// A tab's button in the bar: its id (volume and brightness are controls, which open their tab on a
/// tap), its icon (with its level or status), and its tone: lit while open, dim without a reading.
fn tab_button(s: &State, tab: Tab) -> (Id, Icon, Tone) {
    let open = s.bottom().tab() == tab;
    let lit = |reading: bool| if open { Tone::Accent } else if reading { Tone::Normal } else { Tone::Dim };
    match tab {
        Tab::Volume => {
            let volume = s.levels().control(Control::Volume);
            (Id::Control(Control::Volume), Icon::volume(volume.unwrap_or(0), s.levels().muted()), lit(volume.is_some()))
        }
        Tab::Brightness => (Id::Control(Control::Brightness), Icon::Brightness, lit(s.levels().control(Control::Brightness).is_some())),
        Tab::Bluetooth => {
            let bt = &s.radios().bluetooth;
            let connected = bt.connected() > 0;
            (Id::Tab(tab), Icon::Bluetooth { connected }, lit(bt.powered && connected))
        }
        Tab::Wifi => {
            let quality = s.sys().status.wifi;
            (Id::Tab(tab), Icon::wifi(quality), lit(quality.is_some()))
        }
        // An empty, dim battery when there is none; low in the accent colour.
        Tab::Battery => {
            let status = &s.sys().status;
            let (icon, tone) = match status.battery {
                Some(level) => {
                    let low = level <= LOW_BATTERY && !status.charging;
                    (Icon::battery(level, status.charging), if low { Tone::Accent } else { Tone::Normal })
                }
                None => (Icon::Battery { tenths: 0, charging: false }, Tone::Dim),
            };
            (Id::Tab(tab), icon, if open { Tone::Accent } else { tone })
        }
        Tab::Settings | Tab::Dock | Tab::Keyboard | Tab::Touchpad | Tab::Sysinfo => {
            (Id::Tab(tab), Icon::for_tab(tab), if open { Tone::Accent } else { Tone::Dim })
        }
    }
}

/// Draws the bar along the top of `screen`; returns the area below it.
pub(super) fn draw(f: &mut Frame, screen: Rect) -> Rect {
    let bar_h = (screen.h * BAR_H).max(20.0);
    let mut body = screen;
    let mut bar = body.take_top(bar_h);
    let icon_button = |f: &mut Frame, id: Id, r: Rect, icon: Icon, tone: Tone| {
        // (Smaller in a narrow window, so neighbours don't touch.)
        let side = (bar_h * ICON).min(r.w * 0.8);
        f.button(id, r, Look::ICON, |f| f.c.icon(icon, r.square_center(side), tone));
    };
    // On the right: the pad mouse, the clock, and the line before them.
    let mouse = bar.take_right(bar_h * SLOT);
    let tone = if f.s.pad().mouse_on() { Tone::Accent } else { Tone::Dim };
    icon_button(f, Id::PadMouse, mouse, Icon::Mouse, tone);
    let clock = f.scratch.clock(f.s.unix_secs());
    let size = bar_h * CLOCK_TEXT;
    let clock_w = f.c.text_width(size, &clock) + bar_h * CLOCK_PAD;
    let clock_r = bar.take_right(clock_w);
    f.c.text_in(clock_r, size, Tone::Normal, &clock, Align::Center);
    let line = bar_h * DIVIDER;
    f.c.fill(Rect::new(clock_r.x - 0.5, (bar_h - line) / 2.0, 1.0, line), Tone::Dim.rgb().alpha(DIVIDER_ALPHA));

    // Before the line, every tab, evenly spread (volume and brightness land near the middle; their
    // slider makes do with the room on either side). Each opens its tab; volume and brightness, held or
    // slid, bring up the slider, which runs along the screen's whole width.
    let cell = bar.w.max(0.0) / Tab::ALL.len() as f32;
    for (i, tab) in Tab::ALL.into_iter().enumerate() {
        let (id, icon, tone) = tab_button(f.s, tab);
        let slides = matches!(id, Id::Control(_));
        icon_button(f, id, Rect::new(i as f32 * cell, 0.0, cell, bar_h), icon, tone);
        if slides {
            f.track(Track { x0: screen.x, w: screen.w });
        }
    }
    body
}

/// The slider, while one moves, else a notice (what a key just did): drawn last, over the bar and the
/// tab below it.
pub(super) fn draw_slider_over(f: &mut Frame, screen: Rect, bar_h: f32) {
    if let Some(slider) = f.s.levels().slider() {
        draw_slider(f, *slider, screen.w, bar_h);
    } else if let Some(notice) = f.s.bottom().notice() {
        draw_notice(f, &notice.what, screen.w, bar_h);
    }
}

/// A notice on a card below the bar, in the middle: a level a key changed (its icon and number), or
/// the screenshot's outcome.
fn draw_notice(f: &mut Frame, what: &NoticeKind, w: f32, bar_h: f32) {
    let side = bar_h * ICON;
    let size = bar_h * LEVEL_TEXT;
    let (icon, text): (Option<Icon>, Fmt<32>) = match what {
        NoticeKind::Level(control) => {
            let level = f.s.levels().control(*control);
            let icon = match control {
                Control::Volume => Icon::volume(level.unwrap_or(0), f.s.levels().muted()),
                Control::Brightness | Control::Param(_) => Icon::Brightness,
            };
            let text = match (control, level) {
                (Control::Volume, _) if f.s.levels().muted() => Fmt::of(format_args!("Muted")),
                (_, Some(v)) => Fmt::of(format_args!("{v}")),
                (_, None) => Fmt::of(format_args!("-")),
            };
            (Some(icon), text)
        }
        NoticeKind::Screenshot(Ok(_)) => (None, Fmt::of(format_args!("Screenshot saved"))),
        NoticeKind::Screenshot(Err(_)) => (None, Fmt::of(format_args!("Screenshot failed"))),
        NoticeKind::Gyro(None) => (None, Fmt::of(format_args!("Hold still: calibrating the gyro"))),
        NoticeKind::Gyro(Some(true)) => (None, Fmt::of(format_args!("Gyro calibrated"))),
        NoticeKind::Gyro(Some(false)) => (None, Fmt::of(format_args!("Moved: gyro not calibrated"))),
    };
    let text_w = f.c.text_width(size, &text);
    let icon_w = if icon.is_some() { side * 1.3 } else { 0.0 };
    let group_w = icon_w + text_w;
    let (gx, gy) = ((w - group_w) / 2.0, bar_h * 1.3);
    let card = Rect::new(gx - side * 0.4, gy - side * 0.3, group_w + side * 0.8, side * 1.6);
    f.c.rounded(card, side * 0.3, SHADE.alpha(SHADE_ALPHA));
    if let Some(icon) = icon {
        f.c.icon(icon, Rect::new(gx, gy, side, side), Tone::Normal);
    }
    f.c.text_in(Rect::new(gx + icon_w, gy, text_w, side), size, Tone::High, &text, Align::Start);
}

/// The slider over the bar's row: the track (the level lit), the thumb under the finger, and the level
/// as a number below the bar, to the side away from the finger.
fn draw_slider(f: &mut Frame, slider: Slider, w: f32, bar_h: f32) {
    let track = Track { x0: slider.track_start(), w: slider.length };
    let cy = bar_h / 2.0;
    let thumb = bar_h * THUMB;
    let at = track.x0 + track.w * slider.fraction(slider.value);
    let band = Rect::new(track.x0 - thumb * 2.0, cy - thumb * 1.8, track.w + thumb * 4.0, thumb * 3.6);
    f.c.rounded(band, band.h / 2.0, SHADE.alpha(SHADE_ALPHA));
    widgets::track(f.c, track, cy, bar_h * TRACK, thumb, Some(slider.percent()), Tone::High);

    let side = bar_h * ICON;
    let icon = match slider.control {
        Control::Volume => Icon::volume(slider.value, f.s.levels().muted()),
        // (A keymap parameter's slider is drawn in its row on the edit screen.)
        Control::Brightness | Control::Param(_) => Icon::Brightness,
    };
    let text = Fmt::<4>::of(format_args!("{}", slider.value));
    let size = bar_h * LEVEL_TEXT;
    let text_w = f.c.text_width(size, &text);
    let group_w = side * 1.3 + text_w;
    let away = w * LEVEL_AWAY;
    let gx = if at < w / 2.0 { at + away } else { at - away - group_w };
    let gy = bar_h * 1.3;
    // On a small card of its own, over whatever the tab shows there.
    let card = Rect::new(gx - side * 0.4, gy - side * 0.3, group_w + side * 0.8, side * 1.6);
    f.c.rounded(card, side * 0.3, SHADE.alpha(SHADE_ALPHA));
    f.c.icon(icon, Rect::new(gx, gy, side, side), Tone::Normal);
    f.c.text_in(Rect::new(gx + side * 1.3, gy, text_w, side), size, Tone::High, &text, Align::Start);
}
