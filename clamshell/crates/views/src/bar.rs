//! The bar along the top of the second screen. It has a button per tab, with the open one lit, all spread
//! evenly. Volume and brightness sit near the middle, where a slider has room on both sides of the
//! finger. Bluetooth, Wi-Fi and the battery are tabs too, and their icons show their status. Then come
//! the clock (the time tab's button) and the pad mouse toggle, which is a switch rather than a tab. At the end is a button for the front program's windows on this screen when ClamShell's view hides
//! them. A tap shows them. Its room is kept when there are none, so nothing moves.
//!
//! While a finger is held on volume or brightness, or slides off it sideways, a slider is drawn over the
//! bar, which stays visible under it. The slider is a track with the current level under the finger, and
//! the level as a number beside it.

use clamshell_state::bottom::BarPlace;
use clamshell_state::bottom::notice::NoticeKind;
use clamshell_state::levels::slider::{Control, Slider};
use clamshell_state::{Id, Tab};

use clamshell_ui::gfx::icons::Icon;
use clamshell_ui::gfx::{Align, Rect};
use clamshell_ui::toolkit::format::Fmt;
use clamshell_ui::toolkit::style::{Look, Tone};
use clamshell_ui::toolkit::{Drag, Frame, Track, widgets};

use super::tabs::{self, Face};

/// The bar's height as a fraction of the screen's, at least 20 pixels.
const BAR_H: f32 = 0.075;
/// Icon and clock text sizes, as fractions of the bar's height.
const ICON: f32 = 0.62;
const CLOCK_TEXT: f32 = 0.34;
/// Space around the clock text, as a fraction of the bar's height.
const CLOCK_PAD: f32 = 0.5;
/// The width of the pad mouse switch's tap slot, as a fraction of the bar's height. The tabs share the
/// room before the clock.
const SLOT: f32 = 1.15;
// Every tab is a stop of the shoulders (see `draw`): the frame holds that many.
const _: () = assert!(Tab::ALL.len() <= clamshell_ui::toolkit::MAX_TABS, "more tabs than the shoulders' stops");

/// The bar's height on `screen`.
pub(super) fn height(screen: Rect) -> f32 {
    (screen.h * BAR_H).max(20.0)
}

/// Draws the bar along the top of `screen` and returns the area below it: the tabs' buttons where their
/// definitions put them (see `super::tabs`), and the pad mouse's. A tab's button is lit while it is open.
pub(super) fn draw(f: &mut Frame, screen: Rect) -> Rect {
    let s = f.s;
    // The pad starts on the open tab's button.
    f.start((tabs::view(s.bottom().tab()).button)(s).id);
    let bar_h = height(screen);
    let mut body = screen;
    let mut bar = body.take_top(bar_h);
    let at = |place: BarPlace| Tab::ALL.into_iter().filter(move |t| t.definition().button == place);
    let button = |tab: Tab| {
        let b = (tabs::view(tab).button)(s);
        let tone = if s.bottom().tab() == tab { Tone::Accent } else { b.tone };
        (b, tone)
    };
    let icon_button = |f: &mut Frame, id: Id, r: Rect, icon: Icon, tone: Tone| {
        // Smaller in a narrow window, so that neighbours don't touch.
        let side = (bar_h * ICON).min(r.w * 0.8);
        f.button(id, r, Look::ICON, |f| f.c.icon(icon, r.square_center(side), tone));
    };
    // On the right: the tabs at its end, the pad mouse and the clock.
    for tab in at(BarPlace::End) {
        let (b, tone) = button(tab);
        let r = bar.take_right(bar_h * SLOT);
        if let Face::Icon(icon) = b.face {
            icon_button(f, b.id, r, icon, tone);
        }
    }
    let mouse = bar.take_right(bar_h * SLOT);
    let tone = if s.pad().mouse_on() { Tone::Accent } else { Tone::Dim };
    icon_button(f, Id::PadMouse, mouse, Icon::Mouse, tone);
    for tab in at(BarPlace::Clock) {
        let (b, tone) = button(tab);
        let clock = f.scratch.local(s.clock().unix(), s.sys().time_zones().current.as_deref()).time(s.sys().twelve_hour());
        let size = bar_h * CLOCK_TEXT;
        let clock_w = f.c.text_width(size, &clock) + bar_h * CLOCK_PAD;
        let clock_r = bar.take_right(clock_w);
        f.button(b.id, clock_r, Look::ICON, |f| {
            f.c.text_in(clock_r, size, tone, &clock, Align::Center);
        });
    }

    // Before the clock, the spread tabs, evenly. Volume and brightness land near the middle, and their
    // slider makes do with the room on either side. Each button opens its tab. Holding or sliding on
    // volume or brightness brings up the slider, which runs along the screen's whole width.
    let cell = bar.w.max(0.0) / at(BarPlace::Spread).count() as f32;
    for (i, tab) in at(BarPlace::Spread).enumerate() {
        let (b, tone) = button(tab);
        if let Face::Icon(icon) = b.face {
            icon_button(f, b.id.clone(), Rect::new(i as f32 * cell, 0.0, cell, bar_h), icon, tone);
            if let Id::Control(control) = b.id {
                f.drag(Drag::Slide { control, track: Track { x0: screen.x, w: screen.w }, at_once: false });
            }
        }
        f.tab_stop(Id::Tab(tab), b.id, tab == s.bottom().tab());
    }
    // Then, left to right, those from the clock on.
    for tab in at(BarPlace::Clock).chain(at(BarPlace::End)) {
        f.tab_stop(Id::Tab(tab), (tabs::view(tab).button)(s).id, tab == s.bottom().tab());
    }
    body
}

/// The slider while one is moving, or else a notice of what a key just did. It is drawn last, over the
/// bar and the tab below it.
pub(super) fn draw_slider_over(f: &mut Frame, screen: Rect, bar_h: f32) {
    if let Some(slider) = f.s.held_slider().filter(|s| s.control.level().is_some()) {
        draw_slider(f, slider, screen.w, bar_h);
    } else if let Some(notice) = f.s.bottom().notice() {
        draw_notice(f, &notice.what, screen.w, bar_h);
    }
}

/// A notice on a card below the bar, in the middle: a level a key changed, with its icon and number, or
/// the outcome of a screenshot.
fn draw_notice(f: &mut Frame, what: &NoticeKind, w: f32, bar_h: f32) {
    let (icon, text): (Option<Icon>, Fmt<96>) = match what {
        NoticeKind::Level(control) => {
            let level = f.s.control(*control);
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
        NoticeKind::WithoutSettings(title) => (None, Fmt::of(format_args!("{title} started without its settings (see the log)"))),
        NoticeKind::KeyboardBusy => (None, Fmt::of(format_args!("Finish the text on the keyboard first"))),
        NoticeKind::Problems(first, 1) => (None, Fmt::of(format_args!("{first}"))),
        NoticeKind::Problems(_, n) => (None, Fmt::of(format_args!("{n} problems: see Tools, Rescan library"))),
        NoticeKind::NotSaved(file) => (None, Fmt::of(format_args!("{file} not saved (see the log)"))),
        NoticeKind::Controller(name, player) => (None, Fmt::of(format_args!("{name}: player {}", player.number()))),
    };
    // Half a bar's height kept free on either side; a longer text is cut, keeping its start.
    let room = w - bar_h;
    let x = (w - widgets::card_width(f, bar_h, icon.is_some(), &text, room)) / 2.0;
    widgets::card(f, x, bar_h * 1.3, bar_h, icon, &text, room);
}

/// The slider over the bar's row (see [`widgets::held_slider`]), with the level as a number below the bar.
fn draw_slider(f: &mut Frame, slider: Slider, w: f32, bar_h: f32) {
    let icon = level_icon(f, slider.control, slider.value);
    let text = Fmt::<4>::of(format_args!("{}", slider.value));
    let within = Rect::new(0.0, 0.0, w, f32::INFINITY);
    widgets::held_slider(f, widgets::HeldSlider { slider: &slider, x0: 0.0, cy: bar_h / 2.0, h: bar_h, within, icon: Some(icon), text: &text });
}

/// The icon for a level at `value`: for volume, muted or how loud; for brightness, its one icon. A keymap
/// parameter's slider has no icon, as it is drawn in its row on the edit screen.
fn level_icon(f: &Frame, control: Control, value: u8) -> Icon {
    match control {
        Control::Volume => Icon::volume(value, f.s.levels().muted()),
        Control::Brightness | Control::Param(_) | Control::Curve(_) | Control::Light(_) | Control::Setting(_) => Icon::Brightness,
    }
}
