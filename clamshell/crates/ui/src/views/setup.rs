//! Setup on the main screen (see `clamshell_state::setup::wizard`): the steps in a line along the top, the
//! one showing lit, then what to do there (not the step's name again), its content, and Back and Next at
//! the bottom. The password step says how the password is typed and what went wrong with the last one;
//! the Wi-Fi step shows the Wi-Fi tab's networks and the time zone step the time tab, each in a panel in
//! the middle. The second screen shows only the keyboard meanwhile (see `super::bottom`).

use clamshell_state::setup::wizard::Step;
use clamshell_state::{Id, SetupId};

use crate::gfx::{Align, Rect};
use crate::toolkit::Frame;
use crate::toolkit::style::{self, Text, Tone};

/// The steps' line, the panel's width and the buttons' row, as fractions of the area.
const STEPS_H: f32 = 0.07;
const PANEL_W: f32 = 0.6;
const BUTTONS_H: f32 = 0.09;

pub(super) fn draw(f: &mut Frame, screen: Rect) {
    let Some(step) = f.s.setup().wizard() else {
        return;
    };
    let (pad, small, heading) = (f.pad(), f.text(Text::Small), f.text(Text::Heading));
    let mut area = style::content(screen).inset(pad);

    // The steps, the one showing lit.
    let steps = area.take_top(area.h * STEPS_H);
    for (i, (s, name)) in [(Step::Password, "Password"), (Step::Wifi, "Wi-Fi"), (Step::Zone, "Time zone")].into_iter().enumerate() {
        let cell = steps.cell(3, 1, 0.0, i);
        f.c.text_in(cell, small, if s == step { Tone::High } else { Tone::Dim }, name, Align::Center);
    }

    let (title, intro) = match step {
        Step::Password => ("Choose a password", "sudo and remote login ask for it. It can't be skipped, and nothing starts until it is set."),
        Step::Wifi => ("Join a network", "Now, or later from the bar."),
        Step::Zone => ("Where you are", "The clock follows the time zone."),
    };
    f.c.text_in(area.take_top(heading * 1.6), heading, Tone::Normal, title, Align::Center);
    f.c.text_in(area.take_top(small * 2.0), small, Tone::Dim, intro, Align::Center);

    let buttons = area.take_bottom(screen.h * BUTTONS_H);
    let panel = area.centered(area.w * PANEL_W, area.h - pad);
    match step {
        Step::Password => password(f, panel),
        Step::Wifi => in_panel(f, panel, super::wifi::list),
        Step::Zone => in_panel(f, panel, super::time::draw),
    }
    self::buttons(f, buttons, step);
}

/// The password step: how it is typed, or why it can't be here, and what went wrong with the last one.
fn password(f: &mut Frame, panel: Rect) {
    let s = f.s;
    let size = f.text(Text::Body);
    let mut col = panel;
    if s.setup().password_passable() {
        col.take_top(size * 2.0);
        f.c.text_in(col.take_top(size * 1.6), size, Tone::Dim, "There is no account here to set one for: Next goes on.", Align::Center);
        return;
    }
    if !s.setup().account().available {
        col.take_top(size * 2.0);
        f.c.text_in(col.take_top(size * 1.6), size, Tone::Dim, "Reading the account…", Align::Center);
        return;
    }
    col.take_top(size * 2.0);
    let typing = s.bottom().editor().is_some();
    let row = col.take_top(size * 2.4).centered(panel.w * 0.6, size * 2.4);
    if typing {
        f.c.text_in(row, size, Tone::Normal, "Type it on the keyboard below, then again", Align::Center);
    } else {
        f.text_button(Id::Setup(SetupId::Password), row, size, Tone::Fn, "Type a password", Align::Center);
    }
    col.take_top(size);
    if let Some(problem) = s.setup().password_problem() {
        f.c.text_in(col.take_top(size * 1.6), size, Tone::Accent, problem, Align::Center);
    }
}

/// Draws a tab's view (`draw`) in `panel`, as it is drawn below the bar.
fn in_panel(f: &mut Frame, panel: Rect, draw: fn(&mut Frame)) {
    let body = std::mem::replace(&mut f.body, panel);
    draw(f);
    f.body = body;
}

/// Back, on the steps that have one before them to go back to, and Next: Skip on Wi-Fi while not joined,
/// Done on the last step, and dimmed where it can't go on yet.
fn buttons(f: &mut Frame, row: Rect, step: Step) {
    let s = f.s;
    let size = f.text(Text::Body);
    let back = row.cell(4, 1, 0.0, 0);
    let next = row.cell(4, 1, 0.0, 3);
    if step == Step::Zone {
        f.text_button(Id::Setup(SetupId::Back), back, size, Tone::Fn, "Back", Align::Center);
    }
    let joined = s.radios().wifi().networks.iter().any(|n| n.connected);
    let (label, can) = match step {
        Step::Password => ("Next", s.setup().password_passable()),
        Step::Wifi if joined => ("Next", true),
        Step::Wifi => ("Skip", true),
        Step::Zone => ("Done", true),
    };
    if can {
        f.text_button(Id::Setup(SetupId::Next), next, size, Tone::Fn, label, Align::Center);
    } else {
        f.c.text_in(next, size, Tone::Dim, label, Align::Center);
    }
}
