//! The session: the state built from what was saved, the UI run, and the system threads started once it
//! is up.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use clamshell_core::bottom::touch::TouchLayout;
use clamshell_core::keymap::Keymaps;
use clamshell_core::settings::Values;
use clamshell_core::{Init, State};
use clamshell_system::library::{self, Sources, meta};
use clamshell_system::paths::Paths;
use clamshell_system::store::Store;
use clamshell_system::wiring::{self, Handles};
use clamshell_system::{Closed, Sink};
use clamshell_ui::{Config, EventSender};

use crate::config::Env;
use crate::control;

/// What was saved: each piece's default where there is none (or it can't be read: logged). The keymaps
/// the first time: the ready-made ones, saved at once.
pub(crate) fn saved(paths: &Paths) -> Init {
    let Some(dir) = paths.state_dir.as_deref() else {
        return Init { keymaps: Keymaps::initial(), ..Init::default() };
    };
    let mut store = Store::open(dir);
    let keymaps = store.keymaps().unwrap_or_else(|| {
        let initial = Keymaps::initial();
        if let Err(e) = store.save(&clamshell_core::Save::Keymaps(initial.clone())) {
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

/// The session: the state from what was saved, the UI, and the system threads.
pub(crate) fn run(env: &Env, paths: Paths, sources: &Sources) -> Result<(), String> {
    let loaded = library::load(sources);
    // Metadata to fetch once the threads run (with somewhere to keep it).
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
    let global = state.settings().global;
    // The threads start once the UI's event loop is there (they send to it); effects come only after.
    let handles: Rc<RefCell<Option<Handles>>> = Rc::default();
    let started = handles.clone();
    let result = clamshell_ui::run(
        state,
        config,
        move |sender| *started.borrow_mut() = Some(start(env, &paths, sources, layout, &global, fetch, sender)),
        move |effect| {
            if let Some(handles) = handles.borrow_mut().as_mut() {
                handles.dispatch(effect);
            }
        },
    );
    result.map_err(|e| e.to_string())
}

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
