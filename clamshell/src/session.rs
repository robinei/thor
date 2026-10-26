//! The session: it builds the state from what was saved, runs the compositor with the UI, and starts the
//! system threads once it is up.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use clamshell_state::bottom::touch::TouchLayout;
use clamshell_state::curve::Curves;
use clamshell_state::keymap::Keymaps;
use clamshell_state::settings::Values;
use clamshell_state::{Init, State};
use clamshell_system::library::{self, Sources, meta};
use clamshell_system::paths::Paths;
use clamshell_system::store::Store;
use clamshell_system::wiring::{self, Handles};
use clamshell_system::{Closed, Sink};
use clamshell_ui::{Config, EventSender};

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
    let loaded = library::load(sources);
    // Metadata to fetch once the threads run, if there is somewhere to keep it.
    let fetch = paths.art.clone().map(|art| meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art });
    let device_session = host == Host::Drm;
    let state = State::new(Init {
        profile: env.profile,
        library: loaded.library,
        start_tab: env.start_tab,
        pad_mouse: env.pad_mouse,
        device_session,
        // Settings, keymaps, curves, window rules, brightness, the dock, where shelf was.
        ..saved(&paths)
    });
    // Nested, the screens are the device's own sizes, as pixels of the host's window; on the screens,
    // they are taken from the outputs.
    let preview = if host == Host::Nested { env.profile.preview } else { env.profile.preview.half() };
    let config = Config { top_size: preview.top, bottom_size: preview.bottom, touch_layout: Arc::default() };
    let layout = config.touch_layout.clone();
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
    let identify = |pid| clamshell_compositor::Process {
        unit: clamshell_system::os::unit_of(pid),
        pids: clamshell_system::os::lineage(pid),
        steam_app: clamshell_system::os::steam_app_of(pid),
    };
    let on_capture = move |captured: Result<clamshell_compositor::Capture, String>| {
        if let Some(handles) = capture_handles.borrow().as_ref() {
            handles.save_screenshot(captured.map(|capture| wiring::ScreenshotPixels { size: capture.size, rgba: capture.rgba }));
        }
    };
    let start = move |sender: EventSender, displays: clamshell_compositor::Displays| {
        // On the screens, the session's services and what the bus starts are given the displays, and the
        // compositor's thread (this one) a higher priority than the apps'.
        if host == Host::Drm {
            clamshell_system::session::environment::export(&displays.wayland, &displays.x11);
            clamshell_system::session::priority::raise(u64::from(rustix::thread::gettid().as_raw_nonzero().get().unsigned_abs()));
        }
        let system_config = wiring::Config {
            profile: env.profile,
            gamepad: env.gamepad.clone(),
            device_session,
            clipboard_display: displays.socket,
            backlights: env.backlights.clone(),
        };
        if let Some(path) = env.control.as_ref().or(paths.control.as_ref()) {
            control::spawn(path.clone(), UiSink(sender.clone()));
        }
        *started.borrow_mut() = Some(start(&system_config, &paths, sources, layout, &global, fetch, sender));
    };
    let result = match host {
        Host::Drm => clamshell_compositor::run_drm(state, config, start, on_effect, identify, on_capture),
        Host::Nested => clamshell_compositor::run(state, config, start, on_effect, identify, on_capture),
    };
    result.map_err(|e| e.to_string())
}

/// The system threads' sink: the UI's event loop.
#[derive(Clone)]
pub struct UiSink(pub EventSender);

impl Sink for UiSink {
    fn send(&self, event: clamshell_state::Event) -> Result<(), Closed> {
        if self.0.send(event) { Ok(()) } else { Err(Closed) }
    }
}

/// Starts every system thread, sending to `sender` (see `clamshell_system::wiring::start`).
pub fn start(
    config: &wiring::Config,
    paths: &Paths,
    sources: &Sources,
    layout: Arc<Mutex<TouchLayout>>,
    global: &Values,
    fetch: Option<meta::Fetch>,
    sender: EventSender,
) -> Handles {
    let sink = UiSink(sender);
    wiring::start(config, paths, sources, layout, global, fetch, sink)
}
