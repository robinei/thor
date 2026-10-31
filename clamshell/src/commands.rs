//! The one-shot commands that print and exit: the tray's icons and menus (`--tray`), the library
//! (`--library`), metadata fetched for it (`--fetch`), and a Steam game's environment (`--launch-env`).

use clamshell_state::Library;
use clamshell_system::library::meta;
use clamshell_system::library::{self, Sources};
use clamshell_system::paths::Paths;
use clamshell_system::store::Store;

/// `--tray`: prints the tray icons and their menus.
pub(crate) fn print_tray() -> Result<(), String> {
    let items = clamshell_system::session::tray::read_now().map_err(|e| format!("tray: {e}"))?;
    for item in items {
        println!("{} ({}) pid {:?} unit {:?} icon {:?}", item.title, item.id, item.pid, item.unit, item.icon);
        print_tray_menu(&item.menu, 1);
    }
    Ok(())
}

/// Prints a tray menu's entries, indented by `depth` (`--tray`).
fn print_tray_menu(entries: &[clamshell_state::session::tray::TrayEntry], depth: usize) {
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

/// `--fetch-metadata`: fetches what the library is missing and prints what was found for each entry.
pub(crate) fn fetch_metadata(paths: &Paths, sources: &Sources) -> Result<(), String> {
    let loaded = library::load(sources);
    let art = paths.art.clone().ok_or("no home dir: nowhere to keep it")?;
    let titles: std::collections::HashMap<_, _> = loaded.library.entries().iter().map(|g| (g.id.clone(), g.title.clone())).collect();
    println!("fetching for {} entries", loaded.wanted.len());
    let (tx, rx) = std::sync::mpsc::channel();
    meta::spawn(meta::Fetch { wanted: loaded.wanted, keys: Some(loaded.keys), art }, &meta::Queue::default(), tx);
    // The channel ends when the fetch threads are done and drop their senders.
    for event in rx {
        if let clamshell_state::Event::Meta { id, meta } = event {
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

pub(crate) fn print_library(library: &Library) {
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

/// Prints an entry indented `depth` levels and, for a group, its entries under it.
fn print_entry(library: &Library, i: usize, depth: usize) {
    let Some(g) = library.entries().get(i) else {
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

/// `--launch-env steam:<id>`: prints the environment Steam game `<id>` gets from ClamShell's settings
/// ([`clamshell_state::settings::launch_env`], as for ClamShell's own launches), a `KEY=value` line each,
/// for Steam's launch hook (`clamshell/steam/steam-launch-wrapper`, docs/runners.md). It needs no running
/// ClamShell: the settings are read from their files, the game's runner from library.json's sources and
/// runners (see [`clamshell_system::library::steam_runner`]), and the grabbed controllers from the file the running one
/// keeps, none without it. `SDL_GAMECONTROLLER_IGNORE_DEVICES` set in its own environment, Steam's list of
/// the controllers it maps with Steam Input, is kept, with the grabbed ones added. Errs if the settings
/// can't be read, so the hook says so.
pub(crate) fn print_launch_env(paths: &Paths, sources: &Sources, entry: &str) -> Result<(), String> {
    let appid: u32 = entry
        .strip_prefix(clamshell_library::ids::STEAM)
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| format!("launch-env: {entry}: not a Steam game's id (steam:<app id>)"))?;
    let dir = paths.state_dir.as_deref().ok_or("launch-env: no HOME: no settings")?;
    let settings = Store::open(dir).try_settings().map_err(|e| format!("launch-env: {e}"))?;
    let runner = library::steam_runner(sources, appid).map_err(|e| format!("launch-env: {e}"))?;
    let levels: Vec<&str> = runner.as_deref().into_iter().collect();
    let values = settings.of_in(entry, &levels);
    let mut tile = Vec::new();
    settings.env_in(entry, &levels, &mut tile);
    let grabbed = paths.grabbed.as_deref().and_then(|file| match std::fs::read_to_string(file) {
        Ok(ids) => Some(ids),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("launch-env: {}: {e}: the grabbed controllers aren't ignored", file.display());
            None
        }
    });
    let steams = std::env::var("SDL_GAMECONTROLLER_IGNORE_DEVICES").ok();
    let ignore: Vec<&str> = steams.iter().chain(&grabbed).map(|ids| ids.trim().trim_matches(',')).filter(|ids| !ids.is_empty()).collect();
    let ignore = ignore.join(",");
    clamshell_system::launch::fex::write_presets(paths.fex.as_deref());
    let facts = clamshell_system::components::launch_facts(&values, paths.fex.as_deref(), Some(ignore.as_str()).filter(|i| !i.is_empty()));
    let mut env = Vec::new();
    clamshell_state::settings::launch_env(&values, facts, &tile, &mut env);
    for (key, value) in env {
        // A line each, read by a shell: a name it can export, and a value on one line.
        let name = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !name || value.contains('\n') {
            log::warn!("launch-env: {entry}: {key:?} isn't a variable a game can be given: skipped");
            continue;
        }
        println!("{key}={value}");
    }
    Ok(())
}
