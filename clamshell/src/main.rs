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
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clamshell_core::keymap::Keymaps;
use clamshell_core::{Init, Library, State};
use clamshell_system::library::{self, Sources};
use clamshell_system::meta;
use clamshell_system::paths::Paths;
use clamshell_system::store::Store;
use clamshell_system::{Handles, job, tuning};
use clamshell_ui::Config;

use cli::Command;
use config::Env;

fn main() -> ExitCode {
    // zbus logs its handshake as info: only its warnings.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = cli::parse(&args) else {
        eprintln!("{}", cli::USAGE);
        return ExitCode::FAILURE;
    };
    let env = Env::from_env();
    let paths = Paths::new(&env.base);
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
            print_library(State::new(Init { library: load_library(&sources).library, ..Init::default() }).shelf().library());
            ExitCode::SUCCESS
        }
        Command::WaylandFilter => match clamshell_system::wayland_filter::run(
            paths.mangohud.parent().map_or_else(|| paths.mangohud.clone(), std::path::Path::to_path_buf),
            paths.mangohud.clone(),
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("wayland filter: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Tray => match clamshell_system::tray::read_now() {
            Ok(items) => {
                for item in items {
                    println!("{} ({}) pid {:?} unit {:?} icon {:?}", item.title, item.id, item.pid, item.unit, item.icon);
                    print_tray_menu(&item.menu, 1);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("tray: {e}");
                ExitCode::FAILURE
            }
        },
        Command::FetchMetadata => {
            let loaded = library::load(&sources);
            let (Some(art), Some(db)) = (paths.art.clone(), paths.db()) else {
                log::error!("no home dir: nowhere to keep it");
                return ExitCode::FAILURE;
            };
            let titles: std::collections::HashMap<_, _> = loaded.library.games.iter().map(|g| (g.id.clone(), g.title.clone())).collect();
            println!("fetching for {} entries", loaded.wanted.len());
            let (tx, rx) = std::sync::mpsc::channel();
            if let Err(e) = meta::spawn(meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art, db }, &meta::Queue::default(), tx) {
                log::error!("{e}");
                return ExitCode::FAILURE;
            }
            // (The threads' senders go when they are done: the channel ends.)
            for event in rx {
                if let clamshell_core::Event::Meta { id, meta } = event {
                    let has = |b: bool, what: &'static str| if b { what } else { "" };
                    let found = [
                        has(meta.cover.is_some(), "cover"),
                        has(meta.shot.is_some(), "screenshot"),
                        has(!meta.summary.is_empty(), "summary"),
                        has(meta.year.is_some(), "year"),
                        has(!meta.developer.is_empty(), "developer"),
                    ];
                    println!(
                        "{}: {}",
                        titles.get(&id).map_or(&*id, String::as_str),
                        found.iter().filter(|f| !f.is_empty()).copied().collect::<Vec<_>>().join(", ")
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Command::Snapshot(dir) => {
            // The demo games unless asked for the real library.
            let library = if env.snapshot_real_library { load_library(&sources).library } else { clamshell_library::demo_library() };
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

/// What was saved: from the database, each piece's default where there is none (or the database can't
/// be read: logged). The keymaps the first time: the ready-made ones.
fn saved(paths: &Paths) -> Init {
    let Some(mut store) = paths.db().and_then(|f| Store::open(&f).map_err(|e| log::error!("store: {}: {e}", f.display())).ok()) else {
        return Init { keymaps: Keymaps::initial(), ..Init::default() };
    };
    fn or_default<T: Default>(what: &str, r: Result<T, clamshell_system::store::Error>) -> T {
        r.unwrap_or_else(|e| {
            log::warn!("store: {what}: {e}");
            T::default()
        })
    }
    // The first start: the ready-made keymaps, saved at once.
    // (Not when they couldn't be read: then what was saved stays.)
    let keymaps = match store.keymaps() {
        Ok(Some(keymaps)) => keymaps,
        Ok(None) => {
            let initial = Keymaps::initial();
            if let Err(e) = store.save_keymaps(&initial) {
                log::warn!("store: keymaps: {e}");
            }
            initial
        }
        Err(e) => {
            log::warn!("store: keymaps: {e}");
            Keymaps::initial()
        }
    };
    Init {
        settings: or_default("settings", store.settings()),
        keymaps,
        window_rules: or_default("window rules", store.window_rules()),
        brightness: or_default("brightness", store.brightness()),
        twelve_hour: or_default("clock", store.twelve_hour()),
        dock: or_default("dock", store.dock()),
        autostart: or_default("autostart", store.autostart()),
        resume: or_default("resume", store.resume()),
        ..Init::default()
    }
}

/// The library: library.json's content dirs, Steam, the desktop files; the demo library if it is empty.
fn load_library(sources: &Sources) -> library::Loaded {
    let loaded = library::load(sources);
    // (Nothing but the Tools tab: the demo games.)
    let empty = loaded.library.games.iter().all(|g| matches!(g.launch, clamshell_library::Launch::Tool(_)));
    if empty { library::Loaded { library: clamshell_library::demo_library(), ..library::Loaded::default() } } else { loaded }
}

fn print_library(library: &Library) {
    for c in &library.collections {
        println!("{} ({}, {} entries)", c.name, c.icon.as_deref().unwrap_or("no icon"), c.games.len());
        for &i in &c.games {
            print_entry(library, i, 1);
        }
    }
    for problem in &library.problems {
        println!("problem: {problem}");
    }
}

/// An entry, `depth` deep, and a group's entries under it.
fn print_entry(library: &Library, i: usize, depth: usize) {
    let Some(g) = library.games.get(i) else {
        return;
    };
    let indent = "  ".repeat(depth);
    if g.is_group() {
        println!("{indent}{}  ({})", g.title, g.id);
        for &item in &g.items {
            print_entry(library, item, depth + 1);
        }
    } else {
        let art = if g.box_front.is_some() { "art" } else { "no art" };
        let at = g.position.map(|p| format!(" [at {} of {} s]", p, g.duration.map_or("?".to_owned(), |d| d.to_string()))).unwrap_or_default();
        println!("{indent}{}  ({}) [{art}]{at} {:?}", g.title, g.id, g.launch);
    }
}

/// The session: the state from what was saved, the UI, and the system threads.
fn run(env: &Env, paths: Paths, sources: &Sources) -> ExitCode {
    let loaded = load_library(sources);
    // Metadata to fetch once the threads run (with somewhere to keep it).
    let fetch = match (paths.art.clone(), paths.db()) {
        (Some(art), Some(db)) => Some(meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art, db }),
        _ => None,
    };
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
        move |sender| *started.borrow_mut() = Some(wiring::start(env, &paths, sources, layout, &global, fetch, sender)),
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

/// A tray menu's entries, indented by depth (`--tray`).
fn print_tray_menu(entries: &[clamshell_core::tray::TrayEntry], depth: usize) {
    for e in entries {
        let state = match (e.enabled, e.checked) {
            (false, _) => " (disabled)",
            (true, Some(true)) => " (on)",
            (true, Some(false)) => " (off)",
            (true, None) => "",
        };
        println!("{}{} [{}]{state}", "  ".repeat(depth), e.label, e.id);
        print_tray_menu(&e.children, depth + 1);
    }
}
