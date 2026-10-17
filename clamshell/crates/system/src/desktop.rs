//! Desktop files: the installed apps, as library entries (shelf's Apps: every app desktop file in
//! `<data dir>/applications` of the XDG data dirs, the user's first, not hidden), each with its name,
//! command and icon (from the icon themes).

use std::fs;
use std::path::{Path, PathBuf};

/// The XDG data dirs, the user's first.
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let user = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    user.into_iter().chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).collect()
}

/// The `[Desktop Entry]` keys we use.
#[derive(Debug, Default, PartialEq, Eq)]
struct Entry {
    name: String,
    exec: String,
    icon: String,
    kind: String,
    /// NoDisplay or Hidden: not to be shown in menus.
    hidden: bool,
}

/// The installed apps as library entries (id `desktop:<id>`), by desktop file: the first of an id in
/// `dirs` (the user's dir first), applications with a command that aren't hidden.
pub fn entries(dirs: &[PathBuf]) -> Vec<clamshell_library::Game> {
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
            if entry.hidden || entry.kind != "Application" || argv.is_empty() {
                continue;
            }
            let title = if entry.name.is_empty() { id.clone() } else { entry.name.clone() };
            let mut game = clamshell_library::Game::new(&format!("desktop:{id}"), &title);
            game.launch = clamshell_library::Launch::Program(argv);
            game.icon = (!entry.icon.is_empty()).then(|| find_icon(&entry.icon, dirs)).flatten();
            game.tool = true;
            out.push(game);
        }
    }
    out
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
            "NoDisplay" | "Hidden" if value == "true" => entry.hidden = true,
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn installed_apps_are_entries_the_users_first_hidden_ones_left_out() {
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
        let entries = entries(&[user.clone(), system.clone()]);
        let [firefox] = entries.as_slice() else { unreachable!("{entries:?}") };
        assert_eq!((&*firefox.id, firefox.title.as_str()), ("desktop:firefox", "My Firefox"));
        assert_eq!(firefox.launch, clamshell_library::Launch::Program(vec!["firefox".into(), "--private".into()]));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn exec_lines_become_arguments() {
        assert_eq!(exec_args("firefox %u"), ["firefox"]);
        assert_eq!(exec_args(r#"/usr/bin/steam-runtime -silent %U"#), ["/usr/bin/steam-runtime", "-silent"]);
        assert_eq!(exec_args(r#""/opt/My App/run" --name "a \"b\"" 100%%"#), ["/opt/My App/run", "--name", r#"a "b""#, "100%"]);
    }
}
