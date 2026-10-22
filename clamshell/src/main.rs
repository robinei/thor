//! ClamShell: shelf, the second screen, the bar, input and power policy in one process with one state.
//! This is the program around them: the command line ([`cli`]), the environment ([`config`]), starting
//! the system threads ([`wiring`]), the control pipe ([`control`]), and, built with the `dev` feature, the
//! snapshots (`snapshot`).

mod cli;
mod config;
mod control;
#[cfg(any(test, feature = "dev"))]
mod demo;
#[cfg(any(test, feature = "dev"))]
mod snapshot;
mod wiring;

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use clamshell_core::keymap::Keymaps;
use clamshell_core::settings::Hud;
use clamshell_core::{Init, Library, State};
use clamshell_system::Handles;
use clamshell_system::launch::job;
use clamshell_system::library::meta;
use clamshell_system::library::{self, Sources};
use clamshell_system::paths::Paths;
use clamshell_system::power::tuning;
use clamshell_system::store::Store;
use clamshell_ui::Config;

use cli::Command;
use config::Env;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,tracing=warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = cli::parse(&args) else {
        eprintln!("{}", cli::USAGE);
        return ExitCode::FAILURE;
    };
    match carry_out(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Carries out what the command line asked for; what went wrong, if anything.
fn carry_out(command: Command) -> Result<(), String> {
    let env = Env::from_env();
    let paths = Paths::new(&env.base);
    let sources = Sources::new(&paths, env.base.home.as_deref(), env.steam);
    match command {
        Command::Run => run(&env, paths, &sources),
        // (It returns only if the job's program couldn't take over.)
        Command::RunJob(file) => Err(format!("{}: {}", file.display(), job::run(&file))),
        Command::Ctl(words) if words == ["help"] => {
            print!("{}", control::help());
            Ok(())
        }
        Command::Ctl(words) => control::send(env.control.as_ref().or(paths.control.as_ref()), &words.join(" ")).map_err(|e| format!("ctl: {e}")),
        Command::FollowSteam(appid) => {
            clamshell_system::launch::launcher::follow_steam(appid);
            Ok(())
        }
        Command::MangohudEnv => {
            let config = tuning::mangohud_config(&paths.mangohud, "default", 0, Hud::Off).map_err(|e| format!("MangoHud config: {e}"))?;
            for (k, v) in tuning::mangohud_env(&config) {
                println!("{k}={v}");
            }
            Ok(())
        }
        Command::Library => {
            print_library(State::new(Init { library: library::load(&sources).library, ..Init::default() }).shelf().library());
            Ok(())
        }
        Command::WaylandFilter => {
            let runtime = paths.mangohud.parent().map_or_else(|| paths.mangohud.clone(), std::path::Path::to_path_buf);
            clamshell_system::wayland_filter::run(runtime, paths.mangohud.clone()).map_err(|e| format!("wayland filter: {e}"))
        }
        Command::ClipboardGet => {
            let text = clamshell_system::clipboard::get().map_err(|e| format!("clipboard: {e}"))?;
            print!("{}", text.unwrap_or_default());
            Ok(())
        }
        Command::ClipboardSet => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| format!("clipboard: {e}"))?;
            clamshell_system::clipboard::set(text).map_err(|e| format!("clipboard: {e}"))
        }
        Command::Tray => print_tray(),
        Command::FetchMetadata => fetch_metadata(&paths, &sources),
        #[cfg(any(test, feature = "dev"))]
        Command::Snapshot(dir) => snapshot::command(&sources, &dir),
    }
}

/// `--tray`: the tray icons there are, and their menus.
fn print_tray() -> Result<(), String> {
    let items = clamshell_system::tray::read_now().map_err(|e| format!("tray: {e}"))?;
    for item in items {
        println!("{} ({}) pid {:?} unit {:?} icon {:?}", item.title, item.id, item.pid, item.unit, item.icon);
        print_tray_menu(&item.menu, 1);
    }
    Ok(())
}

/// `--fetch-metadata`: what the library wants fetched, printing what was found for each entry.
fn fetch_metadata(paths: &Paths, sources: &Sources) -> Result<(), String> {
    let loaded = library::load(sources);
    let art = paths.art.clone().ok_or("no home dir: nowhere to keep it")?;
    let titles: std::collections::HashMap<_, _> = loaded.library.entries.iter().map(|g| (g.id.clone(), g.title.clone())).collect();
    println!("fetching for {} entries", loaded.wanted.len());
    let (tx, rx) = std::sync::mpsc::channel();
    meta::spawn(meta::Fetch { wanted: loaded.wanted, keys: loaded.keys, art }, &meta::Queue::default(), tx).map_err(|e| e.to_string())?;
    // (The threads' senders go when they are done: the channel ends.)
    for event in rx {
        if let clamshell_core::Event::Meta { id, meta } = event {
            let has = |b: bool, what: &'static str| if b { what } else { "" };
            let found = [
                has(meta.cover.is_some(), "cover"),
                has(meta.details.shot.is_some(), "screenshot"),
                has(!meta.details.summary.is_empty(), "summary"),
                has(meta.details.year.is_some(), "year"),
                has(!meta.details.developer.is_empty(), "developer"),
            ];
            let found: Vec<&str> = found.into_iter().filter(|f| !f.is_empty()).collect();
            println!("{}: {}", titles.get(&id).map_or(&*id, String::as_str), found.join(", "));
        }
    }
    Ok(())
}

/// What was saved: each piece's default where there is none (or it can't be read: logged). The keymaps
/// the first time: the ready-made ones, saved at once.
fn saved(paths: &Paths) -> Init {
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

fn print_library(library: &Library) {
    for c in &library.collections {
        println!("{} ({}, {} entries)", c.name, c.icon.as_deref().unwrap_or("no icon"), c.entries.len());
        for &i in &c.entries {
            print_entry(library, i, 1);
        }
    }
    for problem in &library.problems {
        println!("problem: {problem}");
    }
}

/// An entry, `depth` deep, and a group's entries under it.
fn print_entry(library: &Library, i: usize, depth: usize) {
    let Some(g) = library.entries.get(i) else {
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
        let at = g.playback.position.map(|p| format!(" [at {} of {} s]", p, g.playback.duration.map_or("?".to_owned(), |d| d.to_string()))).unwrap_or_default();
        println!("{indent}{}  ({}) [{art}]{at} {:?}", g.title, g.id, g.launch);
    }
}

/// The session: the state from what was saved, the UI, and the system threads.
fn run(env: &Env, paths: Paths, sources: &Sources) -> Result<(), String> {
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
        move |sender| *started.borrow_mut() = Some(wiring::start(env, &paths, sources, layout, &global, fetch, sender)),
        move |effect| {
            if let Some(handles) = handles.borrow_mut().as_mut() {
                handles.dispatch(effect);
            }
        },
    );
    result.map_err(|e| e.to_string())
}

/// A tray menu's entries, indented by depth (`--tray`).
fn print_tray_menu(entries: &[clamshell_core::session::tray::TrayEntry], depth: usize) {
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
