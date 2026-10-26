//! Starting the system threads, and gathering the handles that the state's effects go to ([`Handles`]).
//! Both are about the same set of threads, so they are kept together.

mod handles;
mod settings;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clamshell_state::bottom::touch::TouchLayout;
use clamshell_state::settings::Values;

pub use handles::Handles;
pub(crate) use settings::Tuning;

use crate::Sink;
use crate::audio::{mixer, sound};
use crate::display::brightness::{self, Backlights};
use crate::input::{pad, touchpad};
use crate::launch::{launcher, units};
use crate::library::meta::cache::Cache;
use crate::library::{self, Sources, meta};
use crate::network::{bluetooth, wifi};
use crate::os::evdev::device::DeviceSpec;
use crate::os::worker::Worker;
use crate::paths::Paths;
use crate::power::tuning;
use crate::readings::{battery, metrics, status};
use crate::store::{Store, Writer};
use crate::time::clock;

/// What the threads are told about the environment, from the binary's `Env`.
pub struct Config {
    pub profile: clamshell_device::Profile,
    /// The controller.
    pub gamepad: DeviceSpec,
    /// Whether ClamShell is the device's own session, in which case it takes the hardware keys.
    pub device_session: bool,
    /// The compositor's socket, for the clipboard.
    pub clipboard_display: PathBuf,
    /// The compositor's displays (`WAYLAND_DISPLAY`, `DISPLAY`), for launches.
    pub displays: Vec<(String, String)>,
    /// The backlights' folder: /sys/class/backlight, or a stand-in.
    pub backlights: PathBuf,
}

/// A thread with commands, or a stand-in that takes none if it couldn't be started.
fn started<C>(name: &'static str, result: io::Result<Worker<C>>) -> Worker<C> {
    result.unwrap_or_else(|e| {
        log::error!("{name} thread: {e}");
        Worker::gone(name)
    })
}

/// A thread without commands. Only its failure to start is worth logging.
fn detached(name: &str, result: io::Result<()>) {
    if let Err(e) = result {
        log::error!("{name} thread: {e}");
    }
}

/// The Tools tab's rescan. On a thread of its own, it forgets the metadata misses if `refetch` is set,
/// reads the library again and sends it, and fetches what the library wants.
fn rescanner<S: Sink + Clone>(sources: Sources, paths: Paths, queue: meta::Queue, sink: S) -> Box<dyn Fn(bool)> {
    Box::new(move |refetch| {
        let (sources, paths, queue, sink) = (sources.clone(), paths.clone(), queue.clone(), sink.clone());
        detached(
            "rescan",
            crate::os::worker::detached("rescan", move || {
                if refetch && let Some(art) = paths.art.as_deref() {
                    log::info!("rescan: {} misses forgotten", Cache::open(art).forget_misses());
                }
                let loaded = library::load(&sources);
                log::info!("rescan: {} entries", loaded.library.entries.len());
                if sink.send(clamshell_state::Event::Library(Box::new(loaded.library))).is_err() {
                    return;
                }
                if let Some(art) = paths.art.clone() {
                    detached("meta", meta::spawn(meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art }, &queue, sink));
                }
            }),
        );
    })
}

/// Starts every system thread, sending to `sink`, and applies the global settings, which are in force
/// until a game brings its own. `layout` is the touch layout the UI keeps up to date, and `fetch` is the
/// library's metadata to fetch.
pub fn start<S: Sink + Clone>(
    env: &Config,
    paths: &Paths,
    sources: &Sources,
    layout: Arc<Mutex<TouchLayout>>,
    global: &Values,
    fetch: Option<meta::Fetch>,
    sink: S,
) -> Handles {
    let backlights = Backlights::new(env.backlights.clone());
    if !paths.has_user_systemd() {
        log::error!("no systemd user manager: ClamShell needs one to run apps and games (as services of their own)");
    }
    // A thread started again after a panic is sent again what it lost.
    let told = sink.clone();
    crate::os::worker::on_restart(move |name| match clamshell_state::Thread::named(name) {
        Some(thread) => drop(told.send(clamshell_state::Event::Restarted(thread))),
        None => log::warn!("{name}: started again, unknown to the state"),
    });
    detached("clock", clock::spawn(sink.clone()));
    let status = started("status", status::spawn(backlights.clone(), sink.clone()));
    let metrics = started("metrics", metrics::spawn(sink.clone()));
    let fetching = meta::Queue::default();
    if let Some(fetch) = fetch {
        detached("meta", meta::spawn(fetch, &fetching, sink.clone()));
    }
    let rescan = rescanner(sources.clone(), paths.clone(), fetching.clone(), sink.clone());
    let touchpad = started("touchpad", touchpad::spawn(layout));
    // Watch input on every device for idle, and the hardware keys in the device's own session.
    detached("input watch", crate::input::watch::spawn(env.device_session, sink.clone()));
    detached("units", units::spawn(sink.clone()));
    // Games start from the launcher, each on a thread of its own, so several start side by side. Plays
    // go to the database.
    let launcher = started(
        "launcher",
        launcher::spawn(
            launcher::Config { watch_later: paths.watch_later.clone(), sockets: paths.sockets.clone(), displays: env.displays.clone() },
            sink.clone(),
        ),
    );
    // The controller is grabbed for good, with a virtual Xbox pad for games.
    let pad_taken = pad::Taken::default();
    let pad = started("pad", pad::spawn(env.gamepad.clone(), env.profile.controls, env.profile.sensor_hub, pad_taken.clone(), sink.clone()));
    // Send the sticks' calibrations as saved. The state has them too, from the same file.
    if let Some(dir) = paths.state_dir.as_deref() {
        pad.send(pad::Command::Sticks(Store::open(dir).sticks()));
    }
    // Sleep waits for the pad to close the gyro's stream.
    detached("sleep", crate::power::sleep::spawn_hold(pad.clone()));
    let battery = started("battery", battery::spawn(sink.clone()));
    let mixer = started("mixer", mixer::spawn(backlights.clone(), sink.clone()));
    let bluetooth = started("bluetooth", bluetooth::spawn(sink.clone()));
    let tray = started("tray", crate::session::tray::spawn(sink.clone()));
    detached("clipboard", crate::session::clipboard::spawn(Some(env.clipboard_display.clone()), sink.clone()));
    let icons = started("icons", crate::session::icons::spawn(sink.clone()));
    let system = started(
        "system",
        crate::errands::spawn(paths.screenshots.clone(), env.profile.refresh_calibration.map(|calibration| calibration.program), sink.clone()),
    );
    // The lengths and frames of browsed videos are read by the metadata threads, as for scanned ones.
    let probe: Box<dyn Fn(Vec<meta::Wanted>) + Send> = {
        let (queue, sink, art) = (fetching.clone(), sink.clone(), paths.art.clone());
        Box::new(move |wanted| {
            if let Some(art) = art.clone() {
                detached("meta", meta::spawn(meta::Fetch { wanted, keys: meta::Keys::default(), art }, &queue, sink.clone()));
            }
        })
    };
    let browsing = crate::files::browse::Browsing {
        config: sources.config.clone(),
        home: sources.home.clone(),
        art: paths.art.clone(),
        probe,
        cache: paths.art.as_deref().and_then(std::path::Path::parent).map(std::path::PathBuf::from),
    };
    let browse = started("browse", crate::files::browse::spawn_browse(browsing, sink.clone()));
    let files = started("files", crate::files::ops::spawn_ops(sources.home.clone(), sink.clone()));
    let wifi = started("wifi", wifi::spawn(sink));
    let tuning = Tuning { hardware: started("tuning", tuning::spawn(env.profile.charge)), pad: pad.clone() };
    tuning.apply(global);
    let store = paths.state_dir.as_deref().and_then(|dir| Writer::spawn(Store::open(dir)).map_err(|e| log::error!("store thread: {e}")).ok());
    Handles {
        paths: paths.clone(),
        store,
        fetching,
        rescan,
        touchpad,
        mixer,
        wifi,
        bluetooth,
        tray,
        metrics,
        status,
        icons,
        battery,
        pad,
        pad_taken,
        launcher,
        sound: started("sound", sound::spawn()),
        system,
        brightness: started("brightness", brightness::spawn(backlights)),
        tuning,
        browse,
        files,
    }
}
