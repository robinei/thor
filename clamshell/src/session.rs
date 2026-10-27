//! The session: it builds the state from what was saved, runs the compositor with the UI, and starts the
//! system threads once it is up.

use std::cell::RefCell;
use std::rc::Rc;

use clamshell_compositor::EventSender;
use clamshell_state::curve::Curves;
use clamshell_state::keymap::Keymaps;
use clamshell_state::settings::Values;
use clamshell_state::{Init, State};
use clamshell_system::library::Sources;
use clamshell_system::paths::Paths;
use clamshell_system::store::Store;
use clamshell_system::wiring::{self, Handles};
use clamshell_system::{Closed, Sink};
use clamshell_ui::Config;

use crate::config::Env;
use crate::control;

/// Loads what was saved. A piece that is missing or can't be read gets its default, and a read error is
/// logged. On the first start the keymaps and curves are the ready-made ones, and they are saved at once.
pub(crate) fn saved(paths: &Paths) -> Init {
    let Some(dir) = paths.state_dir.as_deref() else {
        return Init { keymaps: Keymaps::initial(), curves: Curves::initial(), ..Init::default() };
    };
    let mut store = Store::open(dir);
    let keymaps = store.keymaps().unwrap_or_else(|| {
        let initial = Keymaps::initial();
        if let Err(e) = store.save(&clamshell_state::Save::Keymaps(initial.clone())) {
            log::warn!("store: keymaps: {e}");
        }
        initial
    });
    let curves = store.curves().unwrap_or_else(|| {
        let initial = Curves::initial();
        if let Err(e) = store.save(&clamshell_state::Save::Curves(initial.clone())) {
            log::warn!("store: curves: {e}");
        }
        initial
    });
    Init {
        settings: store.settings(),
        keymaps,
        curves,
        window_rules: store.window_rules(),
        brightness: store.brightness(),
        twelve_hour: store.twelve_hour(),
        first_start: !store.first_start_done(),
        sticks: store.sticks(),
        dock: store.dock(),
        autostart: store.autostart(),
        resume: store.resume(),
        ..Init::default()
    }
}

/// Where the compositor shows the screens.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    /// In a window on a desktop.
    Nested,
    /// On the machine's screens, as the device's own session: it takes the hardware keys (volume,
    /// brightness) and may put the device to sleep when idle.
    Drm,
}

/// Runs the session: the state from what was saved, the compositor with the UI, and the system threads.
pub(crate) fn run(env: &Env, paths: Paths, sources: &Sources, host: Host) -> Result<(), String> {
    // First, before any thread starts (see `quit_signals`).
    let quit = clamshell_compositor::quit_signals().map_err(|e| format!("quit signals: {e}"))?;
    raise_open_files();
    let device_session = host == Host::Drm;
    // The library is read on a thread as the system threads start, and comes as `Event::Library`.
    let state = State::new(Init {
        profile: env.profile,
        library: None,
        start_tab: env.start_tab,
        pad_mouse: env.pad_mouse,
        device_session,
        // Settings, keymaps, curves, window rules, brightness, the dock, where shelf was.
        ..saved(&paths)
    });
    // Nested, the screens are the device's own sizes, as pixels of the host's window; on the screens,
    // they are taken from the outputs.
    let preview = if host == Host::Nested { env.profile.preview } else { env.profile.preview.half() };
    let config = Config { top_size: preview.top, bottom_size: preview.bottom };
    let global = state.prefs().settings().global;
    // The threads send to the compositor's event loop, so they start once it exists. Effects are
    // dispatched only after that.
    let handles: Rc<RefCell<Option<Handles>>> = Rc::default();
    let started = handles.clone();
    let capture_handles = handles.clone();
    let on_effect = move |effect: &clamshell_state::Effect| {
        if let Some(handles) = handles.borrow_mut().as_mut() {
            handles.dispatch(effect);
        }
    };
    let identify = |pid| {
        let pids = clamshell_system::os::lineage(pid);
        let game = clamshell_system::os::launcher_game_of(pid, &pids);
        clamshell_compositor::Process { unit: clamshell_system::os::unit_of(pid), pids, game }
    };
    let on_capture = move |captured: Result<clamshell_state::ScreenshotPixels, String>| {
        if let Some(handles) = capture_handles.borrow().as_ref() {
            handles.save_screenshot(captured);
        }
    };
    let start = move |sender: EventSender, displays: clamshell_compositor::Displays| {
        // On the screens, the session's services and what the bus starts are given the displays, and the
        // compositor's thread (this one) a higher priority than the apps'.
        if host == Host::Drm {
            set_up_session(&displays);
        }
        let system_config = wiring::Config {
            profile: env.profile,
            gamepad: env.gamepad.clone(),
            device_session,
            clipboard_display: displays.socket,
            displays: vec![("WAYLAND_DISPLAY".into(), displays.wayland.clone()), ("DISPLAY".into(), displays.x11.clone())],
            backlights: env.backlights.clone(),
            charge_limit: env.charge_limit.clone(),
        };
        if let Some(path) = env.control.as_ref().or(paths.control.as_ref()) {
            control::spawn(path.clone(), LoopSink(sender.clone()));
        }
        *started.borrow_mut() = Some(start(&system_config, &paths, sources, &global, sender));
    };
    let result = match host {
        Host::Drm => clamshell_compositor::run_drm(state, config, start, on_effect, identify, on_capture, quit, env.scanout),
        #[cfg(feature = "nested")]
        Host::Nested => clamshell_compositor::run_nested(state, config, start, on_effect, identify, on_capture, quit),
        #[cfg(not(feature = "nested"))]
        Host::Nested => Err("this build has no nested session (built without the `nested` feature)".into()),
    };
    result.map_err(|e| e.to_string())
}

/// Gives the session's services and the bus the displays, and asks rtkit to raise the compositor's
/// thread, the calling one. Each is a D-Bus call that can wait out its timeout (25 s), so each is on a
/// thread of its own. systemd is told ClamShell is up only once the displays are given, since the
/// services started after it (udiskie) take the user manager's environment as they start.
fn set_up_session(displays: &clamshell_compositor::Displays) {
    use clamshell_system::os::worker::detached;
    use clamshell_system::session::{environment, priority};
    let compositor = u64::from(rustix::thread::gettid().as_raw_nonzero().get().unsigned_abs());
    if let Err(e) = detached("priority", move || priority::raise(compositor)) {
        log::warn!("priority: {e}; the compositor's thread runs at the ordinary nice level");
    }
    let (wayland, x11, held) = (displays.wayland.clone(), displays.x11.clone(), displays.readiness.hold());
    // If the thread can't start, the hold goes with it and ClamShell is up without the displays given.
    if let Err(e) = detached("environment", move || {
        environment::export(&wayland, &x11);
        drop(held);
    }) {
        log::error!("session: {e}: WAYLAND_DISPLAY and DISPLAY not given to the user manager and the bus");
    }
}

/// The soft limit of open files raised to the hard one. Every client's socket, buffer and sync file is an
/// fd here, and 1024, the usual soft limit, runs out with a few games and their windows. The service sets
/// both limits (clamshell.service); this covers a session started otherwise, as nested on a desktop. A
/// failure is logged: the session runs on at the limit it had.
fn raise_open_files() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let limit = getrlimit(Resource::Nofile);
    // No hard limit can't be the soft one: the kernel takes at most fs.nr_open.
    let (Some(soft), Some(hard)) = (limit.current, limit.maximum) else { return };
    if soft >= hard {
        return;
    }
    match setrlimit(Resource::Nofile, Rlimit { current: Some(hard), maximum: Some(hard) }) {
        Ok(()) => log::info!("open files: at most {hard} (from {soft})"),
        Err(e) => log::warn!("open files: raising the limit from {soft} to {hard}: {e}; it stays at {soft}"),
    }
}

/// The system threads' sink: the compositor's event loop, which hands each event to the UI.
#[derive(Clone)]
pub struct LoopSink(pub EventSender);

impl Sink for LoopSink {
    fn send(&self, event: clamshell_state::Event) -> Result<(), Closed> {
        if self.0.send(event) { Ok(()) } else { Err(Closed) }
    }
}

/// Starts every system thread, sending to `sender` (see `clamshell_system::wiring::start`).
pub fn start(config: &wiring::Config, paths: &Paths, sources: &Sources, global: &Values, sender: EventSender) -> Handles {
    let sink = LoopSink(sender);
    wiring::start(config, paths, sources, global, sink)
}

#[cfg(test)]
mod tests {
    /// The soft limit of open files is the hard one afterwards (where the hard one is finite).
    #[test]
    fn the_open_files_limit_is_raised_to_the_hard_one() {
        use rustix::process::{Resource, getrlimit};
        super::raise_open_files();
        let limit = getrlimit(Resource::Nofile);
        if limit.maximum.is_some() {
            assert_eq!(limit.current, limit.maximum);
        }
    }
}
