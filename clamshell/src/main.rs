//! ClamShell: shelf, the second screen, the bar, input and power policy in one process with one state.
//! This is the program around them: the command line ([`cli`]), the environment ([`config`]), starting
//! the system threads ([`wiring`]), the test pipe ([`control`]) and the snapshots ([`snapshot`]).

mod cli;
mod config;
mod control;
mod demo;
mod snapshot;
mod wiring;

use std::cell::RefCell;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clamshell_core::keymap::Keymaps;
use clamshell_core::levels::Brightness;
use clamshell_core::settings::Settings;
use clamshell_core::{Init, Library, State};
use clamshell_system::library::{self, Sources};
use clamshell_system::paths::Paths;
use clamshell_system::{Handles, desktop, job, tuning};
use clamshell_ui::Config;

use cli::Command;
use config::Env;

/// An example launcher.json: the defaults and a few more (an app without an icon shows its initial).
pub const LAUNCHER_EXAMPLE: &str = r#"{
  "apps": [
    { "desktop": "steam" },
    { "name": "Terminal", "icon": "terminal", "command": ["foot"], "multiple": true },
    { "desktop": "firefox" },
    { "name": "Files", "icon": "files", "command": ["thunar"], "multiple": true },
    { "name": "Music", "icon": "music", "command": ["foot", "ncmpcpp"] },
    { "name": "Kodi", "command": ["kodi"] }
  ]
}
"#;

fn main() -> ExitCode {
    // zbus logs its handshake as info: only its warnings.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = cli::parse(&args) else {
        eprintln!("{}", cli::USAGE);
        return ExitCode::FAILURE;
    };
    let env = Env::from_env();
    let paths = Paths::new(&env.base, env.pegasus_config.clone());
    let sources = Sources::new(&paths, env.base.home.as_deref(), env.steam);
    match command {
        Command::Ctl(words) if words == ["help"] => {
            print!("{}", control::help());
            ExitCode::SUCCESS
        }
        Command::Ctl(words) => match control::send(env.control.as_ref().or(paths.control.as_ref()), &words.join(" ")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("clamshell ctl: {e}");
                ExitCode::FAILURE
            }
        },
        Command::RunJob(file) => {
            log::error!("{}: {}", file.display(), job::run(&file));
            ExitCode::FAILURE
        }
        Command::LauncherExample => {
            print!("{LAUNCHER_EXAMPLE}");
            ExitCode::SUCCESS
        }
        Command::MangohudEnv => match tuning::mangohud_config(&paths.mangohud, "default", 0, 0) {
            Ok(config) => {
                for (k, v) in tuning::mangohud_env(&config) {
                    println!("{k}={v}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                log::error!("{e}");
                ExitCode::FAILURE
            }
        },
        Command::Library => {
            print_library(State::new(Init { library: load_library(&sources), ..Init::default() }).shelf().library());
            ExitCode::SUCCESS
        }
        Command::Snapshot(dir) => {
            // The demo games unless asked for the real library.
            let library = if env.snapshot_real_library { load_library(&sources) } else { clamshell_library::demo_library() };
            let now = if env.snapshot_now { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() } else { snapshot::FIXED_NOW };
            match snapshot::render(&|| snapshot::state(library.clone()), now, &dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    log::error!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Run => run(&env, paths, &sources),
    }
}

/// The keymaps from keymaps.txt; without the file (the first start), the ready-made ones, saved.
fn load_keymaps(paths: &Paths) -> Keymaps {
    let Some(file) = paths.keymaps() else {
        return Keymaps::initial();
    };
    match std::fs::read_to_string(&file) {
        Ok(text) => Keymaps::load(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let keymaps = Keymaps::initial();
            if let Err(e) = clamshell_system::paths::write(&file, &keymaps.text()) {
                log::warn!("{}: {e}", file.display());
            }
            keymaps
        }
        Err(e) => {
            log::warn!("{}: {e}", file.display());
            Keymaps::default()
        }
    }
}

/// The library: Pegasus game dirs and Steam; the demo games if it is empty.
fn load_library(sources: &Sources) -> Library {
    let library = library::load(sources);
    if library.collections.is_empty() { clamshell_library::demo_library() } else { library }
}

fn print_library(library: &Library) {
    for c in &library.collections {
        println!("{} ({}, {} games){}", c.name, c.short_name, c.games.len(), if c.tools { " tools" } else { "" });
        for g in c.games.iter().filter_map(|&i| library.games.get(i)) {
            let art = if g.box_front.is_some() { "art" } else { "no art" };
            println!("  {}  [{art}] {:?}", g.title, g.launch);
        }
    }
}

/// The session: the state from what was saved, the UI, and the system threads.
fn run(env: &Env, paths: Paths, sources: &Sources) -> ExitCode {
    let read = |file: Option<&Path>| file.and_then(|f| std::fs::read_to_string(f).ok());
    let state = State::new(Init {
        library: load_library(sources),
        // Each game's settings (power profile, FPS cap).
        settings: read(paths.game_settings().as_deref()).map_or_else(Settings::default, |t| Settings::load(&t)),
        // The keymaps; the first time, the Nintendo layout, written at once.
        keymaps: load_keymaps(&paths),
        // The screens' brightness as it was left: the level and each screen's share.
        brightness: read(paths.brightness.as_deref()).map_or_else(Brightness::default, |t| Brightness::from_text(&t)),
        // The dock's apps (without launcher.json: Steam and a terminal).
        apps: desktop::load_apps(paths.launcher.as_deref()),
        // Where shelf and the second screen were last time.
        resume: read(paths.resume.as_deref()),
        start_tab: env.start_tab,
        pad_mouse: env.pad_mouse,
        device_session: env.device_session,
    });
    if let Some(dir) = &paths.state_dir
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::warn!("state dir {}: {e}", dir.display());
    }
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
        move |sender| *started.borrow_mut() = Some(wiring::start(env, &paths, layout, &global, sender)),
        move |effect| {
            if let Some(handles) = handles.borrow_mut().as_mut() {
                handles.dispatch(effect);
            }
        },
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}
