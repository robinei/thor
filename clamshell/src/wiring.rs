//! Starting the policy threads once the UI's event loop is up (their events go to it), and gathering
//! the handles the state's effects go to.

use std::io;
use std::sync::{Arc, Mutex};

use clamshell_core::settings::Values;
use clamshell_core::touch::TouchLayout;
use clamshell_policy::brightness::Backlights;
use clamshell_policy::job::Runner;
use clamshell_policy::keyboard::VirtualKeyboard;
use clamshell_policy::paths::Paths;
use clamshell_policy::tuning::Tuning;
use clamshell_policy::worker::Worker;
use clamshell_policy::{
    Closed, Handles, Sink, battery, bluetooth, brightness, clock, frames, launcher, metrics, mixer, pad, sound, status, sway, touchpad, tuning, units,
    wifi,
};
use clamshell_ui::EventSender;

use crate::config::Env;
use crate::control;

/// The second screen's pixels per millimetre: the Thor's bottom panel, 1240 pixels across about 75 mm.
const BOTTOM_PX_PER_MM: f32 = 16.5;

/// The policy threads' sink: the UI's event loop.
#[derive(Clone)]
pub struct UiSink(pub EventSender);

impl Sink for UiSink {
    fn send(&self, event: clamshell_core::Event) -> Result<(), Closed> {
        if self.0.send(event) { Ok(()) } else { Err(Closed) }
    }
}

/// A thread with commands, or a stand-in that takes none if it couldn't be started.
fn started<C>(name: &'static str, result: io::Result<Worker<C>>) -> Worker<C> {
    result.unwrap_or_else(|e| {
        log::error!("{name} thread: {e}");
        Worker::gone(name)
    })
}

/// A thread without commands: only its failure to start is worth a word.
fn detached(name: &str, result: io::Result<()>) {
    if let Err(e) = result {
        log::error!("{name} thread: {e}");
    }
}

/// Starts every policy thread, sending to `sender`, and applies the global settings (in force until a
/// game brings its own). `layout`: the touch layout the UI keeps up to date.
pub fn start(env: &Env, paths: &Paths, layout: Arc<Mutex<TouchLayout>>, global: &Values, sender: EventSender) -> Handles {
    let sink = UiSink(sender);
    let backlights = Backlights::new(env.backlights.clone());
    let scope = paths.has_user_systemd();
    detached("clock", clock::spawn(sink.clone()));
    detached("status", status::spawn(backlights.clone(), sink.clone()));
    detached("metrics", metrics::spawn(sink.clone()));
    detached("frames", frames::spawn(paths.mangohud.clone(), sink.clone()));
    let touchpad = started("touchpad", touchpad::spawn(touchpad::Config { device: env.touchscreen.clone(), px_per_mm: BOTTOM_PX_PER_MM }, layout, sink.clone()));
    // Tests drive the program through a pipe, not input devices.
    if let Some(path) = &env.control {
        control::spawn(path.clone(), sink.clone());
    }
    let sway = sway::spawn(env.sway.clone(), env.bottom_output.clone(), sink.clone()).unwrap_or_else(|e| {
        log::error!("sway thread: {e}");
        env.sway.as_ref().map(|_| Worker::gone("sway commands"))
    });
    detached("units", units::spawn(sink.clone()));
    // Games start from the launcher (each on a thread of its own, so several run side by side); plays go
    // to stats.db in the state dir.
    let jobs = Runner { jobs: paths.jobs.clone(), sway: sway.clone() };
    let launcher = started("launcher", launcher::spawn(launcher::Config { stats_db: paths.stats_db(), scope, jobs: jobs.clone() }, sink.clone()));
    // The controller, grabbed for good with a virtual Xbox pad for games.
    let pad = started("pad", pad::spawn(env.gamepad.clone(), sink.clone()));
    let battery = started("battery", battery::spawn(sink.clone()));
    let mixer = started("mixer", mixer::spawn(backlights.clone(), sink.clone()));
    let bluetooth = started("bluetooth", bluetooth::spawn(sink.clone()));
    let wifi = started("wifi", wifi::spawn(sink));
    let tuning = Tuning { hardware: started("tuning", tuning::spawn()), pad: pad.clone(), mangohud: paths.mangohud.clone() };
    tuning.apply(global, None);
    Handles {
        paths: paths.clone(),
        scope,
        touchpad,
        mixer,
        wifi,
        bluetooth,
        battery,
        pad,
        launcher,
        sway,
        jobs,
        sound: started("sound", sound::spawn()),
        brightness: started("brightness", brightness::spawn(backlights)),
        tuning,
        // The on-screen keyboard types through a virtual keyboard (US positions).
        keyboard: VirtualKeyboard::new().map_err(|e| log::error!("virtual keyboard: {e}")).ok(),
    }
}
