//! Starting the system threads once the UI's event loop is up (their events go to it), and gathering
//! the handles the state's effects go to.

use std::io;
use std::sync::{Arc, Mutex};

use clamshell_core::settings::Values;
use clamshell_core::touch::TouchLayout;
use clamshell_system::brightness::Backlights;
use clamshell_system::job::Runner;
use clamshell_system::keyboard::VirtualKeyboard;
use clamshell_system::library::{self, Sources};
use clamshell_system::meta;
use clamshell_system::paths::Paths;
use clamshell_system::store::Store;
use clamshell_system::tuning::Tuning;
use clamshell_system::worker::Worker;
use clamshell_system::{
    Closed, Handles, Sink, battery, bluetooth, brightness, clock, frames, launcher, metrics, mixer, pad, sound, status, sway, touchpad, tuning, units, wifi,
};
use clamshell_ui::EventSender;

use crate::config::Env;
use crate::control;

/// The second screen's pixels per millimetre: the Thor's bottom panel, 1240 pixels across about 75 mm.
const BOTTOM_PX_PER_MM: f32 = 16.5;

/// The system threads' sink: the UI's event loop.
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

/// The Tools tab's rescan: on a thread, the misses forgotten (`refetch`), the library read again and sent,
/// and what it wants fetched.
fn rescanner(sources: Sources, paths: Paths, queue: meta::Queue, sink: UiSink) -> Box<dyn Fn(bool)> {
    Box::new(move |refetch| {
        let (sources, paths, queue, sink) = (sources.clone(), paths.clone(), queue.clone(), sink.clone());
        detached(
            "rescan",
            clamshell_system::worker::detached("rescan", move || {
                if refetch
                    && let Some(db) = paths.db()
                    && let Err(e) = Store::open(&db).and_then(|s| s.forget_misses())
                {
                    log::warn!("rescan: forgetting the misses: {e}");
                }
                let loaded = library::load(&sources);
                log::info!("rescan: {} entries", loaded.library.games.len());
                if sink.send(clamshell_core::Event::Library(Box::new(loaded.library))).is_err() {
                    return;
                }
                if let (Some(art), Some(db)) = (paths.art.clone(), paths.db()) {
                    detached("meta", meta::spawn(meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art, db }, &queue, sink));
                }
            }),
        );
    })
}

/// Starts every system thread, sending to `sender`, and applies the global settings (in force until a
/// game brings its own). `layout`: the touch layout the UI keeps up to date; `fetch`: the library's
/// metadata to fetch.
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
    let backlights = Backlights::new(env.backlights.clone());
    if !paths.has_user_systemd() {
        log::error!("no systemd user manager: ClamShell needs one to run apps and games (in scopes of their own)");
    }
    detached("clock", clock::spawn(sink.clone()));
    let status = started("status", status::spawn(backlights.clone(), sink.clone()));
    let metrics = started("metrics", metrics::spawn(sink.clone()));
    let fetching = meta::Queue::default();
    if let Some(fetch) = fetch {
        detached("meta", meta::spawn(fetch, &fetching, sink.clone()));
    }
    let rescan = rescanner(sources.clone(), paths.clone(), fetching.clone(), sink.clone());
    detached("frames", frames::spawn(paths.mangohud.clone(), sink.clone()));
    let touchpad = started("touchpad", touchpad::spawn(touchpad::Config { px_per_mm: BOTTOM_PX_PER_MM }, layout));
    // Commands (`clamshell ctl`, tests) come through a pipe.
    if let Some(path) = env.control.as_ref().or(paths.control.as_ref()) {
        control::spawn(path.clone(), sink.clone());
    }
    // Input on every device, for idle; the hardware keys in the device's own session.
    detached("input watch", clamshell_system::input::watch::spawn(env.device_session, sink.clone()));
    let sway = sway::spawn(env.sway.clone(), env.bottom_output.clone(), sink.clone()).unwrap_or_else(|e| {
        log::error!("sway thread: {e}");
        env.sway.as_ref().map(|_| Worker::gone("sway commands"))
    });
    detached("units", units::spawn(sink.clone()));
    // Games start from the launcher (each on a thread of its own, so several run side by side); plays go
    // to the database.
    let jobs = Runner { jobs: paths.jobs.clone(), sway: sway.clone() };
    let launcher = started(
        "launcher",
        launcher::spawn(launcher::Config { watch_later: paths.watch_later.clone(), jobs: jobs.clone(), sockets: paths.sockets.clone() }, sink.clone()),
    );
    // The controller, grabbed for good with a virtual Xbox pad for games.
    let pad_taken = pad::Taken::default();
    let pad = started("pad", pad::spawn(env.gamepad.clone(), pad_taken.clone(), sink.clone()));
    let battery = started("battery", battery::spawn(sink.clone()));
    let mixer = started("mixer", mixer::spawn(backlights.clone(), sink.clone()));
    let bluetooth = started("bluetooth", bluetooth::spawn(sink.clone()));
    let tray = started("tray", clamshell_system::tray::spawn(sink.clone()));
    detached("clipboard", clamshell_system::clipboard::spawn(sink.clone()));
    let icons = started("icons", clamshell_system::icons::spawn(sink.clone()));
    let system = started("system", clamshell_system::device::spawn(paths.screenshots.clone(), sink.clone()));
    // (Browsed videos' lengths and frames read as scanned ones' are, by the metadata threads.)
    let probe: Box<dyn Fn(Vec<meta::Wanted>) + Send> = {
        let (queue, sink, art, db) = (fetching.clone(), sink.clone(), paths.art.clone(), paths.db());
        Box::new(move |wanted| {
            if let (Some(art), Some(db)) = (art.clone(), db.clone()) {
                detached("meta", meta::spawn(meta::Fetch { wanted, keys: meta::Keys::default(), art, db }, &queue, sink.clone()));
            }
        })
    };
    let browsing = clamshell_system::files::Browsing {
        config: sources.config.clone(),
        home: sources.home.clone(),
        db: paths.db(),
        probe,
        cache: paths.art.as_deref().and_then(std::path::Path::parent).map(std::path::PathBuf::from),
    };
    let browse = started("browse", clamshell_system::files::spawn_browse(browsing, sink.clone()));
    let files = started("files", clamshell_system::files::spawn_ops(sources.home.clone(), sink.clone()));
    let wifi = started("wifi", wifi::spawn(sink));
    let tuning = Tuning { hardware: started("tuning", tuning::spawn()), pad: pad.clone(), mangohud: paths.mangohud.clone() };
    tuning.apply(global, None);
    let store = paths.db().and_then(|f| Store::open(&f).map_err(|e| log::error!("store: {}: {e}", f.display())).ok());
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
        sway,
        jobs,
        sound: started("sound", sound::spawn()),
        system,
        brightness: started("brightness", brightness::spawn(backlights)),
        tuning,
        browse,
        files,
        // The on-screen keyboard types through a virtual keyboard (US positions).
        keyboard: VirtualKeyboard::new().map_err(|e| log::error!("virtual keyboard: {e}")).ok(),
    }
}
