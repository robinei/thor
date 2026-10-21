//! The Device view, on the main screen in shelf's place (see `clamshell_core::device`): a row each, as the
//! time tab's: Wi-Fi (its tab), the time zone (the time tab), the password (typed in the second screen's
//! editor, shown as dots there), remote login; the sticks and the gyro calibrated; the top screen's
//! refresh rate calibrated; Done at the first start, else Close. A sticks' calibration shows on the
//! right: what to do, and each stick as a circle with its dot and the extremes reached so far. Off the
//! device (no helper) the password and remote login read as unavailable.

use std::fmt::{self, Display, Formatter, Write};

use clamshell_core::Id;
use clamshell_core::account::PasswordStep;
use clamshell_core::device::{DeviceId, Refresh};
use clamshell_core::editor::{Editor, Purpose};
use clamshell_core::sticks::{Phase, Progress, StickAxis};

use super::time::Name;
use crate::draw::{Align, Column, Keep, Rect};
use crate::toolkit::Frame;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{self, Look, SURFACE_HI, Text, Tone};
use crate::toolkit::widgets;

/// A row's height, of the screen's height; the rows' column, of its width.
const ROW_H: f32 = 0.085;
const ROWS_W: f32 = 0.5;
/// A stick's circle, of the right half's width; the dot's radius, of the circle's.
const STICK: f32 = 0.36;
const DOT: f32 = 0.07;

/// The view, over all of `screen`.
pub(super) fn draw(f: &mut Frame, screen: Rect) {
    let s = f.s;
    let device = s.device();
    let (pad, size) = (f.pad(), f.text(Text::Body));
    let area = style::content(screen).inset(pad);
    let small = f.text(Text::Small);
    let heading = f.text(Text::Heading);
    f.c.text_fit(area.top_line(heading), heading, Tone::Normal, "Device", Keep::Start);
    let intro = if s.first_run() { "First start: set these up now, or later from Tools, Device" } else { "Set up once, and now and then" };
    f.c.text_fit(area.top_line(small).shifted(0.0, heading * 1.5), small, Tone::Dim, intro, Keep::Start);
    let top = heading * 1.5 + small * 2.2;
    let mut right = Rect::new(area.x, area.y + top, area.w, area.h - top);
    let rows = right.take_left(area.w * ROWS_W);
    let row_h = screen.h * ROW_H;
    let mut col = Column::new(rows);

    // Wi-Fi: the network joined (its name when the Wi-Fi tab has read it).
    let wifi = col.take(row_h);
    let joined = s.radios().wifi.networks.iter().find(|n| n.active).map(|n| n.ssid.as_str());
    let value = match (joined, s.sys().status.wifi) {
        (Some(ssid), _) => Fmt::<96>::of(format_args!("{ssid}")),
        (None, Some(_)) => Fmt::of(format_args!("Connected")),
        (None, None) => Fmt::of(format_args!("Not connected")),
    };
    let connected = s.sys().status.wifi.is_some();
    f.button(Id::Device(DeviceId::Wifi), wifi, Look::ROW, |f| widgets::value_row(f, wifi, size, "Wi-Fi", &value, connected));

    let zone = col.take(row_h);
    let name = match s.time_zones().current.as_deref() {
        Some(current) => Fmt::<96>::of(format_args!("{}", Name(current))),
        None => Fmt::of(format_args!("Unknown")),
    };
    f.button(Id::Device(DeviceId::Zone), zone, Look::ROW, |f| widgets::value_row(f, zone, size, "Time zone", &name, true));

    let account = s.account();
    let password = col.take(row_h);
    if account.available {
        let value = if account.password { "Set" } else { "Not set" };
        f.button(Id::Device(DeviceId::Password), password, Look::ROW, |f| widgets::value_row(f, password, size, "Password", value, account.password));
    } else {
        widgets::value_row(f, password, size, "Password", "Unavailable", false);
    }
    let ssh = col.take(row_h);
    match (account.available, account.password) {
        (true, true) => widgets::switch_row(f, Id::Device(DeviceId::Ssh), ssh, size, "Remote login (ssh)", account.ssh),
        (true, false) => widgets::value_row(f, ssh, size, "Remote login (ssh)", "Needs a password", false),
        (false, _) => widgets::value_row(f, ssh, size, "Remote login (ssh)", "Unavailable", false),
    }

    // The controller and the screen.
    col.skip(row_h * 0.3);
    let sticks = col.take(row_h);
    let value = match (device.sticks(), device.sticks_failed(), device.calibrations().is_empty()) {
        (Some(_), _, _) => "Calibrating",
        (None, true, _) => "Not calibrated: again?",
        (None, false, false) => "Calibrated",
        (None, false, true) => "Calibrate",
    };
    f.button(Id::Device(DeviceId::Sticks), sticks, Look::ROW, |f| widgets::value_row(f, sticks, size, "Sticks", value, device.sticks().is_none()));
    let gyro = col.take(row_h);
    f.button(Id::Device(DeviceId::Gyro), gyro, Look::ROW, |f| widgets::value_row(f, gyro, size, "Gyro (held still)", "Calibrate", true));
    let refresh = col.take(row_h);
    let (value, ok) = match device.refresh() {
        Refresh::Idle => (Fmt::<128>::of(format_args!("Calibrate")), true),
        Refresh::Running => (Fmt::of(format_args!("Calibrating (the bottom screen goes off)")), false),
        Refresh::Done(Ok(said)) => (Fmt::of(format_args!("{}", said.lines().last().unwrap_or("Calibrated"))), true),
        Refresh::Done(Err(why)) => (Fmt::of(format_args!("{}", why.lines().last().unwrap_or("Failed"))), false),
    };
    f.button(Id::Device(DeviceId::Refresh), refresh, Look::ROW, |f| widgets::value_row(f, refresh, size, "Top screen's refresh", &value, ok));

    col.skip(row_h * 0.3);
    let end = col.take(row_h);
    let inner = Rect::new(end.x + size * 0.4, end.y, end.w - size * 0.8, end.h);
    if s.first_run() {
        f.text_button(Id::Device(DeviceId::Done), inner, size, Tone::Fn, "Done", Align::Start);
    } else {
        f.text_button(Id::Device(DeviceId::Close), inner, size, Tone::Fn, "Close", Align::Start);
    }

    if let Some(p) = device.sticks() {
        draw_sticks(f, right.inset(pad), p, size);
    }
}

/// A sticks' calibration going: what to do, and each stick.
fn draw_sticks(f: &mut Frame, area: Rect, p: &Progress, size: f32) {
    let say = match p.phase {
        Phase::LetGo => "Let go of the sticks",
        Phase::Turn => "Turn each stick around its edge",
        Phase::Release => "Let go of the sticks",
    };
    f.c.text_fit(area.top_line(size), size, Tone::Normal, say, Keep::Start);
    let r = area.w * STICK / 2.0;
    let cy = area.y + size * 2.0 + r * 1.2;
    for (i, cx) in [area.x + area.w * 0.27, area.x + area.w * 0.73].into_iter().enumerate() {
        let (x, y) = (i * 2, i * 2 + 1);
        f.c.circle(cx, cy, r, SURFACE_HI);
        let to = |v: i32| (v - (p.min + p.max) / 2) as f32 / ((p.max - p.min) as f32 / 2.0) * r;
        // The extremes reached: a box (from the centre taken; before that, nothing).
        if p.phase != Phase::LetGo {
            let (ax, ay) = (axis(p, x), axis(p, y));
            let b = Rect::new(cx + to(ax.low), cy + to(ay.low), to(ax.high) - to(ax.low), to(ay.high) - to(ay.low));
            f.c.outline(b, 2.0, 2.0, Tone::Dim.color());
        }
        let now = |i: usize| p.now.get(i).copied().unwrap_or(0);
        f.c.circle(cx + to(now(x)), cy + to(now(y)), r * DOT, Tone::Accent.color());
    }
}

fn axis(p: &Progress, i: usize) -> StickAxis {
    p.seen.get(i).copied().unwrap_or(StickAxis { low: 0, centre: 0, high: 0 })
}

/// The password's editor, on the second screen's keyboard tab: what is asked, the text as dots.
pub(super) fn draw_password(f: &mut Frame) {
    let Some(Editor { purpose: Purpose::AccountPassword(step), text }) = f.s.bottom().editor() else {
        return;
    };
    let (title, save) = match step {
        PasswordStep::Current => ("Current password", "Next"),
        PasswordStep::New { current: Some(_) } => ("New password (8 characters at least)", "Set"),
        PasswordStep::New { current: None } => ("A password, for sudo and remote login (8 characters at least)", "Set"),
    };
    let dots = Fmt::<256>::of(format_args!("{}", Dots(text.chars().count())));
    let cancel = (Id::Editor(clamshell_core::EditorId::Cancel), "Cancel");
    super::editor::field(f, title, &dots, cancel, (Id::Editor(clamshell_core::EditorId::Save), save));
}

/// A password as dots, one a character.
struct Dots(usize);

impl Display for Dots {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for _ in 0..self.0 {
            f.write_char('•')?;
        }
        Ok(())
    }
}
