//! The two screens as clients see them: one output each, side by side in one scene (the top screen on
//! the left), their sizes, the refresh rate told to clients, and what the state wants of the screens'
//! power and rate for a host to carry out. A host sets the scene's size and the refresh; the server
//! lays out the outputs, tells the UI each screen's size, and tells the state the top screen's size and
//! the screens' colours.

use clamshell_ui::{Config, Screen};
use smithay::{
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{DisplayHandle, Weak, protocol::wl_output::WlOutput},
    utils::{Logical, Rectangle, Transform},
};

use crate::{clients::ClientState, server::Server};

pub(crate) struct Screens {
    /// The top screen's output and the second's.
    pub outputs: [Output; 2],
    /// The scene's size: both screens side by side, the nested window's or the panels' together.
    pub size: (i32, i32),
    /// Where the second screen begins in the scene.
    pub top_width: i32,
    /// The screens' widths in the configuration, whose proportion splits a scene of any width.
    preview_widths: (u32, u32),
    /// The outputs' refresh in mHz: the host monitor's when nested, so clients pacing to it (mpv's
    /// display-resample) match the frame callbacks they get.
    pub refresh: i32,
    /// Whether the state wants the top screen and the second on (`Compositor::Outputs`), for a host with
    /// screens.
    pub on: [bool; 2],
    /// The top screen's refresh the state wants in Hz, 0 for the rate it had at start
    /// (`Compositor::Refresh`), for a host with screens.
    pub refresh_wanted: u16,
    /// The top output's instances clients bound, each told its game's mode (see [`Server::tell_game_mode`]).
    pub top_bound: Vec<Weak<WlOutput>>,
    /// The screens last told to the state (see [`Server::report_screens`]).
    sent: Option<clamshell_state::presentation::Screens>,
}

impl Screens {
    /// The outputs, with their globals, at the configuration's sizes.
    pub fn new(dh: &DisplayHandle, config: &Config) -> Self {
        let outputs = ["ClamShell-Top", "ClamShell-Bottom"].map(|name| {
            Output::new(
                name.into(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "ClamShell".into(),
                    model: "Nested".into(),
                    serial_number: name.into(),
                },
            )
        });
        // A game sees only the top screen: Wine lays out every output it is shown as a monitor, and a game
        // may open on the second. A dual-screen emulator sees both (see `ClientState::top_only`).
        outputs[0].create_global::<Server>(dh);
        outputs[1].create_global_with_filter::<Server, _>(dh, |client| !ClientState::top_only(client));
        Self {
            outputs,
            size: (config.top_size.0.saturating_add(config.bottom_size.0).min(i32::MAX as u32) as i32, config.top_size.1.min(i32::MAX as u32) as i32),
            top_width: config.top_size.0.min(i32::MAX as u32) as i32,
            preview_widths: (config.top_size.0.max(1), config.bottom_size.0.max(1)),
            refresh: 60_000,
            on: [true; 2],
            refresh_wanted: 0,
            top_bound: Vec::new(),
            sent: None,
        }
    }
}

impl Server {
    /// Sets the scene's size, splitting it between the screens in the configuration's proportion.
    pub fn resize(&mut self, (width, height): (i32, i32)) {
        self.touch_cancel();
        self.screens.size = (width.max(4), height.max(4));
        let (top, bottom) = self.screens.preview_widths;
        self.screens.top_width =
            ((u64::from(self.screens.size.0 as u32) * u64::from(top) / (u64::from(top) + u64::from(bottom)).max(1)) as i32).clamp(2, self.screens.size.0 - 2);
        for (screen, output, x, w) in [
            (Screen::Top, &self.screens.outputs[0], 0, self.screens.top_width),
            (Screen::Bottom, &self.screens.outputs[1], self.screens.top_width, self.screens.size.0 - self.screens.top_width),
        ] {
            let mode = Mode { size: (w, self.screens.size.1).into(), refresh: self.screens.refresh };
            output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((x, 0).into()));
            output.set_preferred(mode);
            // One mode, the screen's: Smithay keeps every mode an output had, and Wine would offer them.
            for old in output.modes().into_iter().filter(|old| *old != mode) {
                output.delete_mode(old);
            }
            self.core.resize(screen, (w as u32, self.screens.size.1 as u32), 1);
            self.core.record(screen, true);
        }
        // Games are told their own modes again, the screen's having been sent to everyone.
        self.screens.top_bound.retain(|output| output.upgrade().is_ok());
        for output in self.screens.top_bound.clone() {
            if let Ok(output) = output.upgrade() {
                self.tell_game_mode(&output);
            }
        }
        self.sync_windows();
    }

    /// Where windows go: the top screen, or (`down`) the second screen's part that shows windows.
    pub fn area(&self, down: bool) -> Rectangle<i32, Logical> {
        if down {
            let b = self.core.content_bounds(Screen::Bottom).unwrap_or_default();
            Rectangle::new((self.screens.top_width + b.x0, b.y0).into(), ((b.x1 - b.x0).max(1), (b.y1 - b.y0).max(1)).into())
        } else {
            Rectangle::from_size((self.screens.top_width, self.screens.size.1).into())
        }
    }

    /// Each screen's colour (the top's, the second's), described to clients (and told to the state, see
    /// [`Server::report_screens`]).
    pub fn describe_screens(&mut self, descriptions: [clamshell_state::colour::ColourDescription; 2]) {
        self.colour.set_outputs(descriptions);
    }

    /// Tells the state the screens' colours and the top screen's size when they change: a curve is shown
    /// within the colour, and a game's resolution is a fraction of the size.
    pub(crate) fn report_screens(&mut self) {
        let screens = clamshell_state::presentation::Screens {
            colour: self.colour.descriptions,
            top_size: (self.screens.top_width.max(0) as u32, self.screens.size.1.max(0) as u32),
        };
        if self.screens.sent != Some(screens) {
            self.screens.sent = Some(screens);
            self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Screens(Box::new(screens))));
        }
    }
}
