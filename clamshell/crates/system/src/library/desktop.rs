//! Desktop files: the installed apps and games as library entries. Every application desktop file in
//! `<data dir>/applications` of the XDG data dirs is read, the user's first, skipping hidden ones. Each
//! entry gets its name, command, working dir and icon, the icon looked up in the icon themes.
//!
//! The categories decide between game and app. `Game` is a game (shelf's Games) unless it is also
//! `Emulator`, since an emulator is started for a game and so is an app. Anything else is an app (Apps).
//! Steam's shortcuts to its games (`steam://rungameid/`) are left out because Steam's own library has
//! them.

use std::fs;
use std::path::{Path, PathBuf};

use crate::os::xdg;

/// An installed app or game: its entry, and the fields library.json's desktop rules match it by (see
/// [`crate::library::content::Rule`]).
#[derive(Debug, Clone)]
pub(crate) struct Desktop {
    /// The desktop file's id (its name without `.desktop`).
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) categories: Vec<String>,
    pub(crate) game: clamshell_library::Entry,
}

/// The `[Desktop Entry]` keys we use.
#[derive(Debug, Default, PartialEq, Eq)]
struct Entry {
    name: String,
    exec: String,
    icon: String,
    kind: String,
    /// The working dir (`Path`).
    path: String,
    categories: String,
    /// NoDisplay or Hidden, meaning it isn't to be shown in menus.
    hidden: bool,
    /// SingleMainWindow, meaning a second launch doesn't give a useful second window.
    single_window: bool,
}

/// Desktop files, by id, whose only category is Game but which aren't games: game launchers and tools for
/// games. Others like them are recognized by a category no game has, such as Steam's Network or Lutris'
/// PackageManager.
const NOT_GAMES: &[&str] = &[
    "com.heroicgameslauncher.hgl",
    "heroic",
    "io.github.Faugus.faugus-launcher",
    "faugus-launcher",
    "io.itch.itch",
    "itch",
    "io.github.sharkwouter.Minigalaxy",
    "org.prismlauncher.PrismLauncher",
    "io.github.benjamimgois.goverlay",
    "net.davidotek.pupgui2",
    "com.vysp3r.ProtonPlus",
    "page.kramo.Cartridges",
];

/// The installed apps and games as library entries, with ids `desktop:<id>`. For each id, the first
/// desktop file in `dirs` is used, the user's dir coming first. Only applications with a command that
/// aren't hidden are included, and not Steam's game shortcuts.
pub(crate) fn entries(dirs: &[PathBuf]) -> Vec<Desktop> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let apps = dir.join("applications");
        let mut files: Vec<PathBuf> = fs::read_dir(&apps)
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    log::debug!("desktop: {}: {e}", apps.display());
                } else {
                    log::warn!("desktop: {}: {e}", apps.display());
                }
            })
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop")).map(str::to_owned) else {
                continue;
            };
            // A user's file hides the system's file with the same id, whatever it says.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path).inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    log::debug!("desktop: {}: {e}", path.display());
                } else {
                    log::warn!("desktop: {}: {e}", path.display());
                }
            }) else {
                continue;
            };
            let entry = parse(&text);
            let argv = exec_args(&entry.exec);
            if entry.hidden || entry.kind != "Application" || argv.is_empty() || entry.exec.contains("steam://rungameid/") {
                continue;
            }
            let category = |c: &str| entry.categories.split(';').any(|x| x == c);
            let title = if entry.name.is_empty() { id.clone() } else { entry.name.clone() };
            let mut game = clamshell_library::Entry::new(&format!("{}{id}", clamshell_library::ids::DESKTOP), &title);
            game.launch = clamshell_library::Launch::Program { argv, workdir: (!entry.path.is_empty()).then(|| PathBuf::from(&entry.path)) };
            game.icon = (!entry.icon.is_empty()).then(|| xdg::find_icon(&entry.icon, dirs)).flatten();
            // A game launcher or a tool for games isn't a game; its games are entries of their own.
            let no_game =
                NOT_GAMES.iter().any(|n| n.eq_ignore_ascii_case(&id)) || ["Emulator", "PackageManager", "Network", "Utility"].iter().any(|c| category(c));
            game.app = !category("Game") || no_game;
            // A game's desktop file starts a game, and once is enough.
            game.another = game.app && !entry.single_window;
            let categories = entry.categories.split(';').filter(|c| !c.is_empty()).map(str::to_owned).collect();
            out.push(Desktop { id, name: title, categories, game });
        }
    }
    out
}

/// The command of the desktop file at `path`, if it has one. This is for an app opened from the Files tab.
pub(crate) fn command(path: &Path) -> Option<clamshell_library::Launch> {
    let entry = parse(&fs::read_to_string(path).inspect_err(|e| log::debug!("desktop: {}: {e}", path.display())).ok()?);
    let argv = exec_args(&entry.exec);
    (!argv.is_empty()).then(|| clamshell_library::Launch::Program { argv, workdir: (!entry.path.is_empty()).then(|| PathBuf::from(&entry.path)) })
}

fn parse(text: &str) -> Entry {
    let mut entry = Entry::default();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_entry) else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "Name" => entry.name = value,
            "Exec" => entry.exec = value,
            "Icon" => entry.icon = value,
            "Type" => entry.kind = value,
            "Path" => entry.path = value,
            "Categories" => entry.categories = value,
            "NoDisplay" | "Hidden" if value == "true" => entry.hidden = true,
            "SingleMainWindow" if value == "true" => entry.single_window = true,
            _ => {}
        }
    }
    entry
}

/// Splits an Exec line into program and arguments. It splits at unquoted spaces and handles double quotes
/// with backslash escapes. Field codes (%f, %U, ...) are dropped, and `%%` becomes a percent sign.
fn exec_args(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            // `%%` is a percent sign. Other field codes are dropped, since apps are started without files or
            // URLs.
            '%' => {
                if chars.next() == Some('%') {
                    word.push('%');
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started || !word.is_empty() {
                    args.push(std::mem::take(&mut word));
                }
                started = false;
            }
            c => word.push(c),
        }
    }
    if started || !word.is_empty() {
        args.push(word);
    }
    args.retain(|a| !a.is_empty());
    args
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn installed_apps_and_games_are_entries_the_users_first_hidden_ones_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let (user, system) = (base.join("user"), base.join("system"));
        let write = |dir: &Path, id: &str, text: &str| {
            fs::create_dir_all(dir.join("applications")).unwrap();
            fs::write(dir.join("applications").join(format!("{id}.desktop")), text).unwrap();
        };
        write(&system, "firefox", "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\n");
        write(&user, "firefox", "[Desktop Entry]\nType=Application\nName=My Firefox\nExec=firefox --private %u\n");
        write(&system, "helper", "[Desktop Entry]\nType=Application\nName=Helper\nExec=helper\nNoDisplay=true\n");
        write(&system, "link", "[Desktop Entry]\nType=Link\nName=Link\nURL=x\n");
        write(&system, "celeste", "[Desktop Entry]\nType=Application\nName=Celeste\nExec=Celeste\nPath=/opt/celeste\nCategories=Game;ActionGame;\n");
        write(&system, "retroarch", "[Desktop Entry]\nType=Application\nName=RetroArch\nExec=retroarch\nCategories=Game;Emulator;\nSingleMainWindow=true\n");
        write(&user, "steam_9", "[Desktop Entry]\nType=Application\nName=Nine\nExec=steam steam://rungameid/9\nCategories=Game;\n");
        let entries: Vec<clamshell_library::Entry> = entries(&[user.clone(), system.clone()]).into_iter().map(|d| d.game).collect();
        let found: Vec<(&str, &str, bool)> = entries.iter().map(|g| (&*g.id, g.title.as_str(), g.app)).collect();
        assert_eq!(
            found,
            vec![("desktop:firefox", "My Firefox", true), ("desktop:celeste", "Celeste", false), ("desktop:retroarch", "RetroArch", true)],
            "a game a game, an emulator an app; Steam's shortcuts left out"
        );
        let launch = |argv: &[&str], workdir: Option<&str>| clamshell_library::Launch::Program {
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
            workdir: workdir.map(PathBuf::from),
        };
        let another: Vec<bool> = entries.iter().map(|g| g.another).collect();
        assert_eq!(another, [true, false, false], "an app opens another, a game doesn't, nor one main window's");
        assert_eq!(entries[0].launch, launch(&["firefox", "--private"], None));
        assert_eq!(entries[1].launch, launch(&["Celeste"], Some("/opt/celeste")));
    }

    #[test]
    fn exec_lines_become_arguments() {
        assert_eq!(exec_args("firefox %u"), ["firefox"]);
        assert_eq!(exec_args(r#"/usr/bin/steam-runtime -silent %U"#), ["/usr/bin/steam-runtime", "-silent"]);
        assert_eq!(exec_args(r#""/opt/My App/run" --name "a \"b\"" 100%%"#), ["/opt/My App/run", "--name", r#"a "b""#, "100%"]);
    }

    /// A damaged file reads as far as it makes sense: keys outside the entry's group and lines without
    /// `=` are passed over, a quote left open takes the rest of the line, and a trailing `%` is dropped.
    #[test]
    fn a_malformed_file_reads_as_far_as_it_makes_sense() {
        let entry = parse("Name=outside\n[Desktop Entry]\nno equals here\n=nothing\nName=Thing\nExec=\"run me --flag %\n[Desktop Action x]\nName=inner\n");
        assert_eq!(entry.name, "Thing");
        assert_eq!(exec_args(&entry.exec), ["run me --flag "]);
        assert_eq!(parse("").name, "");
        assert_eq!(parse("[Desktop Entry\nName=half a header").name, "", "not the entry's group");
        assert!(exec_args("").is_empty());
        assert!(exec_args("%").is_empty());
    }
}
