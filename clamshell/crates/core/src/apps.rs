//! The dock's apps, from a JSON file (`~/.config/clamshell/launcher.json`):
//!
//! ```json
//! { "apps": [
//!     { "desktop": "steam" },
//!     { "desktop": "foot", "multiple": true }
//! ] }
//! ```
//!
//! An app is a desktop file (`desktop`: its id, giving the name, command, icon and window class), or
//! named here with a command (program and arguments, no shell); fields given here
//! override the desktop file's. `icon` picks one of the drawn icons ([`ICONS`]) instead of the desktop
//! file's; with neither, the name's initial shows. `multiple`: a "+" starts another instance. Without
//! the file, Steam and a terminal. (Shelf has no tile: Home goes there.)

use serde::Deserialize;

/// The icon names the launcher draws; any other shows the app's initial.
pub const ICONS: [&str; 8] = ["shelf", "gamepad", "terminal", "browser", "files", "music", "video", "settings"];

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub desktop: Option<String>,
    #[serde(default)]
    pub multiple: bool,
    /// The icon image from the desktop file (filled in when resolving it); also its image cache key.
    #[serde(skip)]
    pub icon_file: Option<std::sync::Arc<str>>,
    /// See [`App::key`]; set when read.
    #[serde(skip)]
    pub key: String,
}

impl App {
    /// A short stable name for it, for its processes' scope (`clamshell-app-<key>-…`): the desktop id or
    /// the name, letters and digits only (no '-').
    pub fn key(&self) -> &str {
        &self.key
    }

    fn with_key(mut self) -> App {
        let source = self.desktop.as_deref().unwrap_or(&self.name);
        self.key = source.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        self
    }

}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    apps: Vec<App>,
}

/// The apps when there is no file.
pub fn defaults() -> Vec<App> {
    // Steam's own client (store, downloads, friends): games it starts become the game all the same.
    let app = |name: &str, icon: &str, command: &[&str]| App {
        name: name.into(),
        icon: icon.into(),
        command: command.iter().map(|&s| s.into()).collect(),
        ..App::default()
    };
    vec![app("Steam", "gamepad", &["steam"]), App { multiple: true, ..app("Terminal", "terminal", &["foot"]) }]
    .into_iter()
    .map(App::with_key)
    .collect()
}

/// shelf's own window.
pub const SHELF_APP_ID: &str = "clamshell.top";

/// The apps in `json`, or what is wrong with it. Desktop files are read afterwards (that is I/O).
pub fn parse(json: &str) -> Result<Vec<App>, String> {
    let file: File = serde_json::from_str(json).map_err(|e| e.to_string())?;
    for app in &file.apps {
        let label = app.desktop.clone().unwrap_or_else(|| app.name.clone());
        if app.name.is_empty() && app.desktop.is_none() {
            return Err("an app needs a name or a desktop file".into());
        }
        if app.command.is_empty() && app.desktop.is_none() {
            return Err(format!("{label}: needs a command or a desktop file"));
        }
    }
    Ok(file.apps.into_iter().map(App::with_key).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_apps_and_says_what_is_wrong() {
        let apps = parse(r#"{"apps": [{"name": "Steam", "icon": "gamepad", "command": ["steam"]}, {"name": "Files", "command": ["thunar", "~"]}]}"#);
        assert_eq!(apps.as_ref().map(Vec::len), Ok(2));
        assert_eq!(apps.as_ref().ok().and_then(|a| a.get(1)).map(|a| a.icon.as_str()), Some(""), "no icon: the initial");
        assert!(parse(r#"{"apps": [{"name": "X"}]}"#).is_err_and(|e| e.contains("needs a command")));
        assert!(parse(r#"{"apps": [{"icon": "files"}]}"#).is_err_and(|e| e.contains("name or a desktop")));
        let foot = parse(r#"{"apps": [{"desktop": "org.codeberg.dnkl.foot", "multiple": true}]}"#);
        let foot = foot.as_ref().ok().and_then(|a| a.first());
        assert!(foot.is_some_and(|f| f.multiple && f.key() == "org_codeberg_dnkl_foot"));
        assert!(parse(r#"{"apps": [{"name": "X", "command": ["x"], "builtin": "shelf"}]}"#).is_err(), "no built-ins: Home is shelf");
        assert!(parse(r#"{"apps": [{"name": "X", "comand": ["x"]}]}"#).is_err_and(|e| e.contains("comand")), "typos are reported");
        assert!(parse("{").is_err());
        assert_eq!(defaults().len(), 2);
    }
}
