//! Desktop files: the installed apps and games, as library entries (every app desktop file in
//! `<data dir>/applications` of the XDG data dirs, the user's first, not hidden), each with its name,
//! command, working dir and icon (from the icon themes). Its categories say which: `Game` is a game
//! (shelf's Games), unless also `Emulator` (an emulator is started for a game: an app); anything else is
//! an app (Apps). Steam's shortcuts to its games (`steam://rungameid/`) are left out: Steam's own
//! library has them.

use std::fs;
use std::path::{Path, PathBuf};

/// The XDG data dirs, the user's first.
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let user = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    user.into_iter().chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).collect()
}

/// An installed app or game: its entry, and what library.json's desktop rules match it by (see
/// [`crate::library::content::Rule`]).
#[derive(Debug, Clone)]
pub struct Desktop {
    /// The desktop file's id (its name without `.desktop`).
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub game: clamshell_library::Game,
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
    /// NoDisplay or Hidden: not to be shown in menus.
    hidden: bool,
    /// SingleMainWindow: a second launch makes no second window worth having.
    single_window: bool,
}

/// The installed apps and games as library entries (id `desktop:<id>`; an app a tool), by desktop file:
/// the first of an id in `dirs` (the user's dir first), applications with a command that aren't hidden
/// (nor Steam's game shortcuts).
pub fn entries(dirs: &[PathBuf]) -> Vec<Desktop> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let mut files: Vec<PathBuf> = fs::read_dir(dir.join("applications")).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.sort();
        for path in files {
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop")).map(str::to_owned) else {
                continue;
            };
            // (A user's file hides the system's of the same id, whatever it says.)
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let entry = parse(&text);
            let argv = exec_args(&entry.exec);
            if entry.hidden || entry.kind != "Application" || argv.is_empty() || entry.exec.contains("steam://rungameid/") {
                continue;
            }
            let category = |c: &str| entry.categories.split(';').any(|x| x == c);
            let title = if entry.name.is_empty() { id.clone() } else { entry.name.clone() };
            let mut game = clamshell_library::Game::new(&format!("desktop:{id}"), &title);
            game.launch = clamshell_library::Launch::Program { argv, workdir: (!entry.path.is_empty()).then(|| PathBuf::from(&entry.path)) };
            game.icon = (!entry.icon.is_empty()).then(|| find_icon(&entry.icon, dirs)).flatten();
            game.tool = !category("Game") || category("Emulator");
            // (A game's desktop file starts a game: once is enough.)
            game.another = game.tool && !entry.single_window;
            let categories = entry.categories.split(';').filter(|c| !c.is_empty()).map(str::to_owned).collect();
            out.push(Desktop { id, name: title, categories, game });
        }
    }
    out
}

/// The command of the desktop file at `path` (an app's, opened from the Files tab), if it has one.
pub fn command(path: &Path) -> Option<clamshell_library::Launch> {
    let entry = parse(&fs::read_to_string(path).ok()?);
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

/// An Exec line as program and arguments: split at unquoted spaces, double quotes with backslash
/// escapes, field codes (%f, %U, ...) dropped, `%%` a percent sign.
pub fn exec_args(exec: &str) -> Vec<String> {
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
            // `%%` is a percent sign; other field codes go (we start apps without files or URLs).
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

/// Icon themes looked in, in order (hicolor is the fallback every app installs into).
const THEMES: [&str; 3] = ["hicolor", "Adwaita", "breeze"];
/// Bitmap sizes, largest first: icons are drawn at about 120 pixels.
const SIZES: [u32; 8] = [512, 256, 192, 128, 96, 64, 48, 32];

/// An icon's image file: an absolute path as is, else the largest in the themes (scalable first), else
/// pixmaps.
pub fn find_icon(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    for dir in dirs {
        for theme in THEMES {
            let base = dir.join("icons").join(theme);
            let mut candidates = vec![base.join("scalable/apps").join(format!("{name}.svg"))];
            for size in SIZES {
                for ext in ["png", "svg"] {
                    candidates.push(base.join(format!("{size}x{size}/apps/{name}.{ext}")));
                    // breeze's layout: apps/<size>/<name>.svg
                    candidates.push(base.join(format!("apps/{size}/{name}.{ext}")));
                }
            }
            if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
                return Some(found);
            }
        }
    }
    dirs.iter().flat_map(|d| ["svg", "png"].map(|ext| d.join("pixmaps").join(format!("{name}.{ext}")))).find(|c| c.is_file())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn installed_apps_and_games_are_entries_the_users_first_hidden_ones_left_out() {
        let base = std::env::temp_dir().join(format!("clamshell-desktop-entries-{}", std::process::id()));
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
        let entries: Vec<clamshell_library::Game> = entries(&[user.clone(), system.clone()]).into_iter().map(|d| d.game).collect();
        let found: Vec<(&str, &str, bool)> = entries.iter().map(|g| (&*g.id, g.title.as_str(), g.tool)).collect();
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
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn exec_lines_become_arguments() {
        assert_eq!(exec_args("firefox %u"), ["firefox"]);
        assert_eq!(exec_args(r#"/usr/bin/steam-runtime -silent %U"#), ["/usr/bin/steam-runtime", "-silent"]);
        assert_eq!(exec_args(r#""/opt/My App/run" --name "a \"b\"" 100%%"#), ["/opt/My App/run", "--name", r#"a "b""#, "100%"]);
    }
}
