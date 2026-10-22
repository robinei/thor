//! The bar along the top of the second screen: a button per tab, the open one lit; volume and
//! brightness, near the middle, where a slider has room on both sides of the finger; Bluetooth, Wi-Fi and
//! the battery (tabs too, that carry their status), all spread evenly; then, past a faint line, the clock
//! (the time tab's button), the pad mouse toggle, which is a switch rather than a tab, and at the end the
//! front program's windows on this screen when ClamShell's view hides them (a tap shows them; its room is
//! kept without them, so nothing moves).
//!
//! While a finger is held on volume or brightness (or slides off it sideways), a slider is drawn over
//! the bar (the bar stays visible under it): a track whose current level sits under the finger, and the
//! level as a number beside it.

use clamshell_core::levels::slider::{Control, Slider};
use clamshell_core::notice::NoticeKind;
use clamshell_core::{Id, State, Tab};

use crate::draw::{Align, Rect};
use crate::icons::Icon;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Tone};
use crate::toolkit::{Frame, Track, widgets};

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

/// A tab's button in the bar: its id (volume and brightness are controls, which open their tab on a
/// tap), its icon (with its level or status), and its tone: lit while open, dim without a reading.
/// The bar's target for `tab`'s icon: volume and brightness are their controls (a tap opens the tab, a
/// slide sets the level), the rest the tab.
pub(crate) fn tab_target(tab: Tab) -> Id {
    match tab {
        Tab::Volume => Id::Control(Control::Volume),
        Tab::Brightness => Id::Control(Control::Brightness),
        tab => Id::Tab(tab),
    }
}

fn tab_button(s: &State, tab: Tab) -> (Id, Icon, Tone) {
    let open = s.bottom().tab() == tab;
    let lit = |reading: bool| {
        if open {
            Tone::Accent
        } else if reading {
            Tone::Normal
        } else {
            Tone::Dim
        }
    };
    match tab {
        Tab::Volume => {
            let volume = s.levels().control(Control::Volume);
            (tab_target(tab), Icon::volume(volume.unwrap_or(0), s.levels().muted()), lit(volume.is_some()))
        }
        Tab::Brightness => (tab_target(tab), Icon::Brightness, lit(s.levels().control(Control::Brightness).is_some())),
        Tab::Bluetooth => {
            let bt = &s.radios().bluetooth;
            let connected = bt.connected() > 0;
            (Id::Tab(tab), Icon::Bluetooth { connected }, lit(bt.on && connected))
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
        Tab::Settings | Tab::Dock | Tab::Keyboard | Tab::Touchpad | Tab::Sysinfo | Tab::Time => {
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
    // On the right: the hidden windows' button, the pad mouse, the clock, and the line before them.
    let windows = bar.take_right(bar_h * SLOT);
    if f.s.hidden_down().is_some() {
        icon_button(f, Id::BottomWindows, windows, Icon::Windows, Tone::Fn);
    }
    let mouse = bar.take_right(bar_h * SLOT);
    let tone = if f.s.pad().mouse_on() { Tone::Accent } else { Tone::Dim };
    icon_button(f, Id::PadMouse, mouse, Icon::Mouse, tone);
    let clock = f.scratch.local(f.s.unix_secs(), f.s.time_zones().current.as_deref()).time(f.s.twelve_hour());
    let size = bar_h * CLOCK_TEXT;
    let clock_w = f.c.text_width(size, &clock) + bar_h * CLOCK_PAD;
    let clock_r = bar.take_right(clock_w);
    let tone = if f.s.bottom().tab() == Tab::Time { Tone::Accent } else { Tone::Normal };
    f.button(Id::Tab(Tab::Time), clock_r, Look::ICON, |f| {
        f.c.text_in(clock_r, size, tone, &clock, Align::Center);
    });
    let line = bar_h * DIVIDER;
    f.c.fill(Rect::new(clock_r.x - 0.5, (bar_h - line) / 2.0, 1.0, line), Tone::Dim.rgb().alpha(DIVIDER_ALPHA));

    // Before the line, every tab, evenly spread (volume and brightness land near the middle; their
    // slider makes do with the room on either side). Each opens its tab; volume and brightness, held or
    // slid, bring up the slider, which runs along the screen's whole width.
    let cell = bar.w.max(0.0) / Tab::ALL.into_iter().filter(|t| t.in_bar()).count() as f32;
    for (i, tab) in Tab::ALL.into_iter().filter(|t| t.in_bar()).enumerate() {
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
    let (icon, text): (Option<Icon>, Fmt<96>) = match what {
        NoticeKind::Level(control) => {
            let level = f.s.levels().control(*control);
            let icon = level_icon(f, *control, level.unwrap_or(0));
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
        NoticeKind::BottomRule(true) => (None, Fmt::of(format_args!("On the bottom screen from now on"))),
        NoticeKind::BottomRule(false) => (None, Fmt::of(format_args!("Back on top from now on"))),
        NoticeKind::Files(Ok(done)) | NoticeKind::Account(Ok(done)) => (None, Fmt::of(format_args!("{done}"))),
        NoticeKind::Files(Err(e)) | NoticeKind::Account(Err(e)) => (None, Fmt::of(format_args!("{e}"))),
        NoticeKind::Enqueued(1, to) => (None, Fmt::of(format_args!("Added to {to}"))),
        NoticeKind::Enqueued(n, to) => (None, Fmt::of(format_args!("{n} added to {to}"))),
        NoticeKind::NoWindow(title) => (None, Fmt::of(format_args!("{title} has no window"))),
        NoticeKind::NotStarted(title) => (None, Fmt::of(format_args!("{title} didn't start"))),
        NoticeKind::Starting(title) => (None, Fmt::of(format_args!("Starting {title}…"))),
        NoticeKind::Problems(first, 1) => (None, Fmt::of(format_args!("{first}"))),
        NoticeKind::Problems(_, n) => (None, Fmt::of(format_args!("{n} problems: see Tools, Rescan library"))),
    };
    let x = (w - widgets::card_width(f, bar_h, icon.is_some(), &text)) / 2.0;
    widgets::card(f, x, bar_h * 1.3, bar_h, icon, &text);
}

/// The slider over the bar's row (see [`widgets::held_slider`]): the level as a number below the bar.
fn draw_slider(f: &mut Frame, slider: Slider, w: f32, bar_h: f32) {
    let icon = level_icon(f, slider.control, slider.value);
    let text = Fmt::<4>::of(format_args!("{}", slider.value));
    let within = Rect::new(0.0, 0.0, w, f32::INFINITY);
    widgets::held_slider(f, widgets::HeldSlider { slider: &slider, x0: 0.0, cy: bar_h / 2.0, h: bar_h, within, icon: Some(icon), text: &text });
}

/// A level's icon at `value`: volume's (muted, or how loud), brightness's. (A keymap parameter's slider
/// is drawn in its row on the edit screen, without one.)
fn level_icon(f: &Frame, control: Control, value: u8) -> Icon {
    match control {
        Control::Volume => Icon::volume(value, f.s.levels().muted()),
        Control::Brightness | Control::Param(_) => Icon::Brightness,
    }
}
