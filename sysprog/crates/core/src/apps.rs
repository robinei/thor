//! The launcher's apps, from a JSON file (`~/.config/sysprog/launcher.json`):
//!
//! ```json
//! { "apps": [
//!     { "name": "Shelf", "icon": "shelf", "builtin": "shelf" },
//!     { "name": "Steam", "icon": "gamepad", "command": ["steam", "-gamepadui"] },
//!     { "name": "Terminal", "icon": "terminal", "command": ["foot"] }
//! ] }
//! ```
//!
//! Each app has a name, an icon (one of [`ICONS`]; otherwise its initial), and either a command (program
//! and arguments, no shell) or a built-in action. Without the file, those three.

use serde::Deserialize;

/// The icon names the launcher draws; any other shows the app's initial.
pub const ICONS: [&str; 8] = ["shelf", "gamepad", "terminal", "browser", "files", "music", "video", "settings"];

/// What the program does itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Builtin {
    /// Brings shelf's window to the front.
    Shelf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub builtin: Option<Builtin>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    apps: Vec<App>,
}

/// The apps when there is no file.
pub fn defaults() -> Vec<App> {
    let app = |name: &str, icon: &str, command: &[&str], builtin| App {
        name: name.into(),
        icon: icon.into(),
        command: command.iter().map(|&s| s.into()).collect(),
        builtin,
    };
    vec![
        app("Shelf", "shelf", &[], Some(Builtin::Shelf)),
        app("Steam", "gamepad", &["steam", "-gamepadui"], None),
        app("Terminal", "terminal", &["foot"], None),
    ]
}

/// The apps in `json`, or what is wrong with it.
pub fn parse(json: &str) -> Result<Vec<App>, String> {
    let file: File = serde_json::from_str(json).map_err(|e| e.to_string())?;
    for app in &file.apps {
        match (app.command.is_empty(), app.builtin.is_some()) {
            (true, false) => return Err(format!("{}: needs a command or a builtin", app.name)),
            (false, true) => return Err(format!("{}: a command or a builtin, not both", app.name)),
            _ => {}
        }
    }
    Ok(file.apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_apps_and_says_what_is_wrong() {
        let apps = parse(r#"{"apps": [{"name": "Shelf", "icon": "shelf", "builtin": "shelf"}, {"name": "Files", "command": ["thunar", "~"]}]}"#);
        assert_eq!(apps.as_ref().map(Vec::len), Ok(2));
        assert_eq!(apps.as_ref().ok().and_then(|a| a.get(1)).map(|a| a.icon.as_str()), Some(""), "no icon: the initial");
        assert!(parse(r#"{"apps": [{"name": "X"}]}"#).is_err_and(|e| e.contains("needs a command")));
        assert!(parse(r#"{"apps": [{"name": "X", "command": ["x"], "builtin": "shelf"}]}"#).is_err());
        assert!(parse(r#"{"apps": [{"name": "X", "comand": ["x"]}]}"#).is_err_and(|e| e.contains("comand")), "typos are reported");
        assert!(parse("{").is_err());
        assert_eq!(defaults().len(), 3);
    }
}
