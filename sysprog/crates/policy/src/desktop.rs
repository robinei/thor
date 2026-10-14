//! Desktop files: a launcher app named by its desktop id gets its name, command, icon and window class
//! from `<data dir>/applications/<id>.desktop` (the XDG data dirs), its icon from the icon themes.

use std::fs;
use std::path::{Path, PathBuf};

use sysprog_core::apps::App;

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
    wm_class: String,
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
            "StartupWMClass" => entry.wm_class = value,
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

/// Fills in from its desktop file what the app's JSON didn't give: name, command, icon image; adds the
/// desktop id, window class and program name to the app ids its windows are known by.
pub fn resolve(app: &mut App, dirs: &[PathBuf]) -> Result<(), String> {
    let Some(id) = app.desktop.clone() else {
        return Ok(());
    };
    let id = id.strip_suffix(".desktop").unwrap_or(&id).to_owned();
    let text = dirs
        .iter()
        .find_map(|d| fs::read_to_string(d.join("applications").join(format!("{id}.desktop"))).ok())
        .ok_or_else(|| format!("no desktop file {id}.desktop"))?;
    let entry = parse(&text);
    if app.name.is_empty() {
        app.name = if entry.name.is_empty() { id.clone() } else { entry.name };
    }
    if app.command.is_empty() {
        app.command = exec_args(&entry.exec);
        if app.command.is_empty() {
            return Err(format!("{id}.desktop: no Exec"));
        }
    }
    app.app_ids.push(id);
    if !entry.wm_class.is_empty() {
        app.app_ids.push(entry.wm_class);
    }
    if let Some(program) = app.command.first().and_then(|p| Path::new(p).file_name()).and_then(|p| p.to_str()) {
        app.app_ids.push(program.to_owned());
    }
    if app.icon.is_empty() && !entry.icon.is_empty() {
        app.icon_file = find_icon(&entry.icon, dirs).map(|p| p.to_string_lossy().into());
    }
    Ok(())
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
    fn exec_lines_become_arguments() {
        assert_eq!(exec_args("firefox %u"), ["firefox"]);
        assert_eq!(exec_args(r#"/usr/bin/steam-runtime -silent %U"#), ["/usr/bin/steam-runtime", "-silent"]);
        assert_eq!(exec_args(r#""/opt/My App/run" --name "a \"b\"" 100%%"#), ["/opt/My App/run", "--name", r#"a "b""#, "100%"]);
    }

    #[test]
    fn a_desktop_file_fills_in_the_app() {
        let dir = std::env::temp_dir().join(format!("sysprog-desktop-{}", std::process::id()));
        fs::create_dir_all(dir.join("applications")).unwrap();
        fs::create_dir_all(dir.join("icons/hicolor/128x128/apps")).unwrap();
        fs::write(
            dir.join("applications/org.example.App.desktop"),
            "[Desktop Entry]\nName=Example\nName[nb]=Eksempel\nExec=example --new %F\nIcon=example\nStartupWMClass=ExampleWin\n[Desktop Action new]\nName=Other\n",
        )
        .unwrap();
        fs::write(dir.join("icons/hicolor/128x128/apps/example.png"), b"png").unwrap();
        let mut app = App { desktop: Some("org.example.App".into()), ..App::default() };
        resolve(&mut app, std::slice::from_ref(&dir)).unwrap();
        assert_eq!(app.name, "Example", "the unlocalized name, not an action's");
        assert_eq!(app.command, ["example", "--new"]);
        assert_eq!(app.icon_file.as_deref(), dir.join("icons/hicolor/128x128/apps/example.png").to_str());
        assert!(app.owns("org.example.App") && app.owns("examplewin") && app.owns("example"));
        // What the JSON says wins.
        let mut named = App { desktop: Some("org.example.App".into()), name: "Mine".into(), icon: "files".into(), ..App::default() };
        resolve(&mut named, std::slice::from_ref(&dir)).unwrap();
        assert_eq!((named.name.as_str(), named.icon_file.is_none()), ("Mine", true));
        let mut missing = App { desktop: Some("nope".into()), ..App::default() };
        assert!(resolve(&mut missing, std::slice::from_ref(&dir)).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
