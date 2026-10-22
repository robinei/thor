//! The one-shot commands that print and exit: the tray's icons and menus (`--tray`), the library
//! (`--library`), and metadata fetched for it (`--fetch`).

use clamshell_core::Library;
use clamshell_system::library::meta;
use clamshell_system::library::{self, Sources};
use clamshell_system::paths::Paths;

/// `--tray`: the tray icons there are, and their menus.
pub(crate) fn print_tray() -> Result<(), String> {
    let items = clamshell_system::session::tray::read_now().map_err(|e| format!("tray: {e}"))?;
    for item in items {
        println!("{} ({}) pid {:?} unit {:?} icon {:?}", item.title, item.id, item.pid, item.unit, item.icon);
        print_tray_menu(&item.menu, 1);
    }
    Ok(())
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

/// `--fetch-metadata`: what the library wants fetched, printing what was found for each entry.
pub(crate) fn fetch_metadata(paths: &Paths, sources: &Sources) -> Result<(), String> {
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
