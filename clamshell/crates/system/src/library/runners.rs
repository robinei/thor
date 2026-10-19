//! Runners: how things open (see docs/runners.md), library.json's `runners`, every one declared there
//! (nothing is opened by a runner it doesn't list). A file opens with the first runner one of whose
//! patterns its name matches (a scan's entries too, unless the scan names its runner). Two
//! types of runner find their items themselves, each item with its own command: `"type": "desktop"` the
//! desktop files' apps and games (and a `.desktop` file opened), `"type": "steam"` Steam's installed games;
//! without one, those aren't in the library.
//!
//! A runner's `id` keeps its settings, environment and own keymap (its `name` is only shown, and may
//! change); without one, its name in lowercase, spaces as `-`.

use std::path::Path;

use clamshell_library::{Launch, pattern};
use serde::Deserialize;

/// A runner, as library.json gives it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    /// What its settings, environment and own keymap are kept by (see the module's doc).
    #[serde(default)]
    pub id: String,
    /// Its settings page's title.
    #[serde(default)]
    pub name: String,
    /// One that finds its items itself (see the module's doc); none: it opens files by its command.
    #[serde(default, rename = "type")]
    pub kind: Option<Kind>,
    /// The file names it opens (one name each: `*.sfc`; any case).
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,
    /// The command, `{path}` the file. A desktop runner's, if it has one, replaces its entries' own
    /// (their desktop file's): `{id}` the desktop file's name, `{exec}` its command (as arguments of
    /// their own): to wrap it, or launch otherwise (the Android runner's script, docs/android.md).
    #[serde(default)]
    pub launch: Vec<String>,
    /// A keymap of ClamShell's it comes with (`viewer`: the D-pad, A, B and the shoulders as keys), its own
    /// until one is chosen for it.
    #[serde(default)]
    pub keymap: Option<String>,
    /// Whether its entries have settings of their own (a settings tab), beside the runner's; else only
    /// the runner's. On unless said (a browsed file or a folder has none either way).
    #[serde(default)]
    pub item_settings: Option<bool>,
    /// What to write to a launch's control socket (its command's `{socket}`) to add a file to it while
    /// it runs (`{path}` in it the file, as a JSON string's contents): its entries can be enqueued.
    #[serde(default)]
    pub enqueue: Option<String>,
    /// Whether its launches hand their app over to something running outside them and end (a desktop
    /// runner's: Waydroid's apps, whose windows are the Waydroid session's): its entries run on by their
    /// windows, found by their app id (the desktop file's name).
    #[serde(default)]
    pub handover: bool,
    /// For a runner that hands its apps over: the command that ends one (Force kill, and Quit once its
    /// windows are closed), `{id}` its windows' app id.
    #[serde(default)]
    pub stop: Vec<String>,
    /// For a runner whose entries share one container (Waydroid's): the commands that pause it all
    /// (In the background: Pauses hidden, set for the runner) and resume it.
    #[serde(default)]
    pub pause: Vec<String>,
    #[serde(default)]
    pub resume: Vec<String>,
    /// A command that succeeds while the runner's app shown keeps the screen on, which the compositor
    /// doesn't hear (Android's keep-screen-on: a video, the camera); asked before the screens dim.
    #[serde(default)]
    pub awake: Vec<String>,
}

/// A runner that finds its items itself, each with its own command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The desktop files' apps and games, by their `Exec`; a `.desktop` file opened from the Files tab.
    Desktop,
    /// Steam's installed games, by Steam.
    Steam,
}

impl Runner {
    /// Its command for file `path` (`{path}` it, `~/` the home dir), in the file's folder: a desktop
    /// file's its own, nothing for a Steam runner.
    pub fn command(&self, path: &Path, home: Option<&Path>) -> Launch {
        match self.kind {
            Some(Kind::Desktop) => crate::desktop::command(path).unwrap_or_default(),
            Some(Kind::Steam) => Launch::None,
            None => clamshell_library::RunnerOf { command: self.template(home).into(), ..clamshell_library::RunnerOf::new(&self.id, &self.name, true) }
                .command_for(&[path.to_path_buf()]),
        }
    }

    /// The runner that opens an entry of it, as the entry keeps it (`~/` the home dir in its commands).
    pub fn of(&self, home: Option<&Path>) -> clamshell_library::RunnerOf {
        clamshell_library::RunnerOf {
            command: self.template(home).into(),
            enqueue: self.enqueue.as_deref().map(Into::into),
            handover: self.handover,
            stop: expanded(&self.stop, home),
            pause: expanded(&self.pause, home),
            resume: expanded(&self.resume, home),
            awake: expanded(&self.awake, home),
            ..clamshell_library::RunnerOf::new(&self.id, &self.name, self.item_settings.unwrap_or(true))
        }
    }

    /// Its command with `~/` against `home` (or as it is), `{path}`, `{paths}` and `{socket}` left in.
    fn template(&self, home: Option<&Path>) -> Vec<String> {
        self.launch.iter().map(|a| super::content::expand(a, home).to_string_lossy().into_owned()).collect()
    }

    /// A desktop entry's command as this runner launches it (desktop file `id`, its own command
    /// `exec`; `~/` the home dir): its `launch` filled in, if it has one; else none (the entry's own).
    pub fn wrap(&self, id: &str, exec: &Launch, home: Option<&Path>) -> Option<Launch> {
        if self.kind != Some(Kind::Desktop) || self.launch.is_empty() {
            return None;
        }
        let (own, workdir) = match exec {
            Launch::Program { argv, workdir } => (argv.as_slice(), workdir.clone()),
            _ => (&[][..], None),
        };
        let mut argv = Vec::new();
        for arg in self.template(home) {
            match arg.as_str() {
                "{exec}" => argv.extend(own.iter().cloned()),
                _ => argv.push(arg.replace("{id}", id)),
            }
        }
        Some(Launch::Program { argv, workdir })
    }

    /// Whether it opens a file named `name`.
    pub fn opens(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.patterns.iter().any(|p| pattern::matches(&p.to_lowercase(), &name))
    }
}

/// A command with `~/` against `home` (or as it is).
fn expanded(args: &[String], home: Option<&Path>) -> std::sync::Arc<[String]> {
    args.iter().map(|a| super::content::expand(a, home).to_string_lossy().into_owned()).collect()
}

/// library.json's runners, each with its id (else its name's) and its name (else its id); what is wrong
/// with them: one without an id or a name, or (not of a type) without a command, is left out; one whose
/// program isn't installed (`installed`: whether a program is on the PATH) is said so.
pub fn checked(config: &[Runner], installed: &dyn Fn(&str) -> bool) -> (Vec<Runner>, Vec<String>) {
    let mut problems = Vec::new();
    let mut runners = Vec::new();
    for r in config {
        let id = if r.id.is_empty() { r.name.to_lowercase().replace(' ', "-") } else { r.id.clone() };
        let name = if r.name.is_empty() { id.clone() } else { r.name.clone() };
        match (id.is_empty(), r.kind, r.launch.first()) {
            (true, _, _) => problems.push(format!("a runner without an id or a name: {}", r.launch.join(" "))),
            (false, None, None) => problems.push(format!("{name}: no launch command")),
            (false, None, Some(_)) if r.enqueue.is_some() && !r.launch.iter().any(|a| a.contains("{socket}")) => {
                problems.push(format!("{name}: enqueue needs {{socket}} in its launch command"));
                runners.push(Runner { id, name, enqueue: None, ..r.clone() });
            }
            (false, None, Some(program)) if !program.contains('/') && !installed(program) => {
                problems.push(format!("{name}: {program} isn't installed"));
                runners.push(Runner { id, name, ..r.clone() });
            }
            (false, _, _) => runners.push(Runner { id, name, ..r.clone() }),
        }
    }
    (runners, problems)
}

/// Whether `program` is on the PATH.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// The runners in force here: library.json's, checked (see [`checked`]).
pub fn in_force(config: &[Runner]) -> (Vec<Runner>, Vec<String>) {
    checked(config, &on_path)
}

/// The runner that opens a file named `name`: the first that does.
pub fn opener<'a>(runners: &'a [Runner], name: &str) -> Option<&'a Runner> {
    runners.iter().find(|p| p.opens(name))
}

/// File `path` opened by the first of `runners` that opens its name: its command and runner.
pub fn open(runners: &[Runner], path: &Path, home: Option<&Path>) -> Option<(Launch, clamshell_library::RunnerOf)> {
    let runner = opener(runners, &path.file_name()?.to_string_lossy())?;
    Some((runner.command(path, home), runner.of(home)))
}

/// The runner of a type, if there is one.
pub fn of_kind(runners: &[Runner], kind: Kind) -> Option<&Runner> {
    runners.iter().find(|r| r.kind == Some(kind))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn runners_are_library_jsons_checked() {
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "id": "snes", "name": "SNES", "match": ["*.sfc"], "launch": ["retroarch", "{path}"] },
                { "name": "Text Viewer", "match": ["*.txt"], "launch": ["kate", "{path}"] },
                { "type": "desktop", "id": "desktop", "name": "Apps", "match": ["*.desktop"] },
                { "name": "Broken" },
                { "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }]"#,
        )
        .unwrap();
        let (runners, problems) = checked(&config, &|p| p != "kate");
        let ids: Vec<&str> = runners.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["snes", "text-viewer", "desktop", "other-files"], "Broken left out");
        assert_eq!(problems, ["Text Viewer: kate isn't installed", "Broken: no launch command"]);
        assert_eq!(opener(&runners, "Zelda.SFC").map(|r| r.id.as_str()), Some("snes"));
        assert_eq!(opener(&runners, "game.zip").map(|r| r.id.as_str()), Some("other-files"));
        let (launch, runner) = open(&runners, Path::new("/g/Zelda.sfc"), None).unwrap();
        assert_eq!(
            (launch, &*runner.key),
            (Launch::Program { argv: vec!["retroarch".into(), "/g/Zelda.sfc".into()], workdir: Some("/g".into()) }, "runner:snes")
        );
        assert_eq!(opener(&runners, "app.desktop").map(|r| r.kind), Some(Some(Kind::Desktop)));
        assert!(of_kind(&runners, Kind::Steam).is_none(), "no Steam runner: no Steam games");
    }

    #[test]
    fn a_desktop_runners_launch_wraps_its_entries_commands() {
        let runner = |launch: &[&str]| Runner { kind: Some(Kind::Desktop), launch: launch.iter().map(|a| (*a).to_owned()).collect(), ..Runner::default() };
        let exec = Launch::Program { argv: vec!["waydroid".into(), "app".into(), "launch".into(), "org.x".into()], workdir: None };
        let args = |l: Option<Launch>| match l {
            Some(Launch::Program { argv, .. }) => argv,
            _ => vec![],
        };
        assert_eq!(args(runner(&["clamshell-android", "{id}"]).wrap("waydroid.org.x", &exec, None)), ["clamshell-android", "waydroid.org.x"]);
        assert_eq!(args(runner(&["gamemoderun", "{exec}"]).wrap("x", &exec, None)), ["gamemoderun", "waydroid", "app", "launch", "org.x"]);
        assert!(runner(&[]).wrap("x", &exec, None).is_none(), "none: the entry's own");
        let home = Path::new("/home/thor");
        assert_eq!(args(runner(&["~/.local/bin/a", "{id}"]).wrap("x", &exec, Some(home))), ["/home/thor/.local/bin/a", "x"], "~/ the home dir");
    }

    #[test]
    fn enqueue_needs_a_socket_to_write_to() {
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "name": "Music", "match": ["*.mp3"], "launch": ["mpv", "{paths}"], "enqueue": "loadfile {path}" },
                { "name": "Video", "match": ["*.mkv"], "launch": ["mpv", "--input-ipc-server={socket}", "{paths}"], "enqueue": "loadfile {path}" }]"#,
        )
        .unwrap();
        let (runners, problems) = checked(&config, &|_| true);
        assert_eq!(problems, ["Music: enqueue needs {socket} in its launch command"]);
        assert_eq!(runners.iter().map(|r| r.of(None).enqueue.is_some()).collect::<Vec<_>>(), [false, true], "Music kept, without it");
        let (launch, _) = open(&runners, Path::new("/v/a.mkv"), None).unwrap();
        let argv = ["mpv", "--input-ipc-server={socket}", "/v/a.mkv"].map(String::from).to_vec();
        assert_eq!(launch, Launch::Program { argv, workdir: Some("/v".into()) }, "{{paths}}: the one file; {{socket}} the launcher's");
    }
}
