//! The session: it builds the state from what was saved, runs the UI, and starts the system threads once
//! the UI is up.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use clamshell_state::bottom::touch::TouchLayout;
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
/// logged. On the first start the keymaps are the ready-made ones, and they are saved at once.
pub(crate) fn saved(paths: &Paths) -> Init {
    let Some(dir) = paths.state_dir.as_deref() else {
        return Init { keymaps: Keymaps::initial(), ..Init::default() };
    };
    let mut store = Store::open(dir);
    let keymaps = store.keymaps().unwrap_or_else(|| {
        let initial = Keymaps::initial();
        if let Err(e) = store.save(&clamshell_state::Save::Keymaps(initial.clone())) {
            log::warn!("store: keymaps: {e}");
        }
        initial
    });
    Init {
        settings: store.settings(),
        keymaps,
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

/// Runs the session: the state from what was saved, the UI, and the system threads.
pub(crate) fn run(env: &Env, paths: Paths, sources: &Sources) -> Result<(), String> {
    run_host(env, paths, sources, false)
}

#[cfg(feature = "compositor")]
pub(crate) fn run_nested(env: &Env, paths: Paths, sources: &Sources) -> Result<(), String> {
    run_host(env, paths, sources, true)
}

fn run_host(env: &Env, paths: Paths, sources: &Sources, nested: bool) -> Result<(), String> {
    let loaded = library::load(sources);
    // Metadata to fetch once the threads run, if there is somewhere to keep it.
    let fetch = paths.art.clone().map(|art| meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art });
    let state = State::new(Init {
        library: loaded.library,
        start_tab: env.start_tab,
        pad_mouse: env.pad_mouse,
        device_session: env.device_session,
        // Settings, keymaps, window rules, brightness, the dock, where shelf was.
        ..saved(&paths)
    });
    let config = Config {
        bottom_output: env.bottom_output.clone(),
        // The Thor's panels at half size: top 1920x1080 (landscape), bottom 1240x1080.
        top_size: (960, 540),
        bottom_size: (620, 540),
        bottom_panel: env.bottom_panel,
        touch_layout: Arc::default(),
    };
    let layout = config.touch_layout.clone();
    let global = state.prefs().settings().global;
    // The threads send to the UI's event loop, so they start once it exists. Effects are dispatched only
    // after that.
    let handles: Rc<RefCell<Option<Handles>>> = Rc::default();
    let started = handles.clone();
    let mut system_config = wiring::Config {
        gamepad: env.gamepad.clone(),
        device_session: env.device_session,
        sway: env.sway.clone(),
        clipboard_display: None,
        bottom_output: env.bottom_output.clone(),
        backlights: env.backlights.clone(),
    };
    let start = move |sender: EventSender, display| {
        system_config.clipboard_display = display;
        if let Some(path) = env.control.as_ref().or(paths.control.as_ref()) {
            control::spawn(path.clone(), UiSink(sender.clone()));
        }
        *started.borrow_mut() = Some(start(&system_config, &paths, sources, layout, &global, fetch, sender));
    };
    let on_effect = move |effect: &clamshell_state::Effect| {
        if let Some(handles) = handles.borrow_mut().as_mut() {
            handles.dispatch(effect);
        }
    };
    #[cfg(feature = "compositor")]
    if nested {
        return clamshell_compositor::run(
            state,
            config,
            |sender, display| start(sender, Some(display)),
            on_effect,
            |pid| clamshell_compositor::Process { unit: clamshell_system::os::unit_of(pid), pids: clamshell_system::os::lineage(pid) },
        )
        .map_err(|e| e.to_string());
    }
    let _ = nested;
    let result = clamshell_ui::run(state, config, |sender| start(sender, None), on_effect);
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
