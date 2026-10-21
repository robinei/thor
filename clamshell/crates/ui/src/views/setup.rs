//! The setup tab (the settings' Setup tile opens it; ClamShell's first start too): what a new device
//! needs, a row each, as the time tab's rows: Wi-Fi (its tab), the time zone (the time tab), the
//! password, remote login; Done. The password is typed in the shared editor, shown as dots: the current
//! one first when there is one, then the new one. Off the device (no helper) the password and remote
//! login read as unavailable.

use std::fmt::{self, Display, Formatter, Write};

use clamshell_core::Id;
use clamshell_core::account::{PasswordStep, SetupId};
use clamshell_core::editor::{Editor, Purpose};

use super::time::Name;
use crate::draw::{Align, Column, Rect};
use crate::toolkit::Frame;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{self, Look, Text, Tone};
use crate::toolkit::widgets;

/// A row's height, of the area's height (the time tab's).
const ROW_H: f32 = 0.105;

pub(super) fn draw(f: &mut Frame) {
    if let Some(Editor { purpose: Purpose::AccountPassword(step), text }) = f.s.bottom().editor() {
        let (title, save) = match step {
            PasswordStep::Current => ("Current password", "Next"),
            PasswordStep::New { current: Some(_) } => ("New password (4 characters at least)", "Set"),
            PasswordStep::New { current: None } => ("A password, for sudo and remote login (4 characters at least)", "Set"),
        };
        let dots = Fmt::<256>::of(format_args!("{}", Dots(text.chars().count())));
        let cancel = (Id::Editor(clamshell_core::EditorId::Cancel), "Cancel");
        return super::editor::field(f, title, &dots, cancel, (Id::Editor(clamshell_core::EditorId::Save), save));
    }
    let s = f.s;
    let body = f.body();
    let (pad, size) = (f.pad(), f.text(Text::Body));
    let area = style::content(body).shifted(0.0, pad * 0.5);
    let small = f.text(Text::Small);
    let intro = if s.first_run() { "First start: set these up now, or later from Settings, Setup" } else { "The device's setup" };
    f.c.text_fit(area.top_line(small), small, Tone::Dim, intro, crate::draw::Keep::Start);
    let row_h = body.h * ROW_H;
    let mut col = Column::new(Rect::new(area.x, area.y + small * 1.8, area.w, area.h - small * 1.8));

    // Wi-Fi: the network joined (its name when the Wi-Fi tab has read it).
    let wifi = col.take(row_h);
    let joined = s.radios().wifi.networks.iter().find(|n| n.active).map(|n| n.ssid.as_str());
    let value = match (joined, s.sys().status.wifi) {
        (Some(ssid), _) => Fmt::<96>::of(format_args!("{ssid}")),
        (None, Some(_)) => Fmt::of(format_args!("Connected")),
        (None, None) => Fmt::of(format_args!("Not connected")),
    };
    let connected = s.sys().status.wifi.is_some();
    f.button(Id::Setup(SetupId::Wifi), wifi, Look::ROW, |f| widgets::value_row(f, wifi, size, "Wi-Fi", &value, connected));

    let zone = col.take(row_h);
    let name = match s.time_zones().current.as_deref() {
        Some(current) => Fmt::<96>::of(format_args!("{}", Name(current))),
        None => Fmt::of(format_args!("Unknown")),
    };
    f.button(Id::Setup(SetupId::Zone), zone, Look::ROW, |f| widgets::value_row(f, zone, size, "Time zone", &name, true));

    let account = s.account();
    let password = col.take(row_h);
    if account.available {
        let value = if account.password { "Set" } else { "Not set" };
        f.button(Id::Setup(SetupId::Password), password, Look::ROW, |f| widgets::value_row(f, password, size, "Password", value, account.password));
    } else {
        widgets::value_row(f, password, size, "Password", "Unavailable", false);
    }
    let ssh = col.take(row_h);
    match (account.available, account.password) {
        (true, true) => widgets::switch_row(f, Id::Setup(SetupId::Ssh), ssh, size, "Remote login (ssh)", account.ssh),
        (true, false) => widgets::value_row(f, ssh, size, "Remote login (ssh)", "Needs a password", false),
        (false, _) => widgets::value_row(f, ssh, size, "Remote login (ssh)", "Unavailable", false),
    }

    let done = col.take(row_h);
    let inner = Rect::new(done.x + size * 0.4, done.y, done.w - size * 0.8, done.h);
    f.text_button(Id::Setup(SetupId::Done), inner, size, Tone::Fn, "Done", Align::Start);
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
