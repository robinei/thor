//! The system threads' sink, and their start once the UI's event loop is up (their events go to it):
//! the control pipe here, the rest in clamshell-system's `wiring`.

use std::sync::{Arc, Mutex};

use clamshell_core::bottom::touch::TouchLayout;
use clamshell_core::settings::Values;
use clamshell_system::library::{Sources, meta};
use clamshell_system::paths::Paths;
use clamshell_system::wiring::{self, Handles};
use clamshell_system::{Closed, Sink};
use clamshell_ui::EventSender;

use crate::config::Env;
use crate::control;

/// The system threads' sink: the UI's event loop.
#[derive(Clone)]
pub struct UiSink(pub EventSender);

impl Sink for UiSink {
    fn send(&self, event: clamshell_core::Event) -> Result<(), Closed> {
        if self.0.send(event) { Ok(()) } else { Err(Closed) }
    }
}

/// Starts the control pipe and every system thread, sending to `sender` (see `clamshell_system::wiring::start`).
pub fn start(
    env: &Env,
    paths: &Paths,
    sources: &Sources,
    layout: Arc<Mutex<TouchLayout>>,
    global: &Values,
    fetch: Option<meta::Fetch>,
    sender: EventSender,
) -> Handles {
    let sink = UiSink(sender);
    // Commands (`clamshell ctl`, tests) come through a pipe.
    if let Some(path) = env.control.as_ref().or(paths.control.as_ref()) {
        control::spawn(path.clone(), sink.clone());
    }
    let config = wiring::Config {
        gamepad: env.gamepad.clone(),
        device_session: env.device_session,
        sway: env.sway.clone(),
        bottom_output: env.bottom_output.clone(),
        backlights: env.backlights.clone(),
    };
    wiring::start(&config, paths, sources, layout, global, fetch, sink)
}
