//! Runners decide how things open (see docs/runners.md). They are library.json's `runners`, and only
//! those: nothing is opened by a runner the file doesn't list.
//!
//! A file opens with the first runner that has a pattern matching its name. That includes a scan's
//! entries, unless the scan names its runner. Two types of runner find their items themselves, each item
//! with its own command. `"type": "desktop"` covers the apps and games from desktop files, and a
//! `.desktop` file that is opened. `"type": "steam"` covers Steam's installed games. Without such a
//! runner, those aren't in the library.
//!
//! A runner's settings, environment and keymap are kept by its `id`. Its `name` is only shown and may
//! change. Without an `id`, the id is the name in lowercase with spaces as `-`.

use std::path::Path;

use clamshell_library::{Launch, pattern};

pub(crate) use clamshell_library::config::{Runner, RunnerKind as Kind};

/// What a runner does with what it opens (see the module's doc).
pub(crate) trait Opens {
    fn command(&self, path: &Path, home: Option<&Path>) -> Launch;
    fn title(&self) -> &str;
    fn of(&self, home: Option<&Path>) -> clamshell_library::RunnerOf;
    fn template(&self, home: Option<&Path>) -> Vec<String>;
    fn wrap(&self, id: &str, exec: &Launch, home: Option<&Path>) -> Option<Launch>;
    fn opens(&self, name: &str) -> bool;
    fn opens_as_its_own(&self, name: &str) -> bool;
}

impl Opens for Runner {
    /// Its command for file `path`, run in the file's folder, with `{path}` filled in and `~/` as the home
    /// dir. A desktop file uses its own command, and a Steam runner has none.
    fn command(&self, path: &Path, home: Option<&Path>) -> Launch {
        match self.kind {
            Some(Kind::Desktop) => crate::library::desktop::command(path).unwrap_or_default(),
            Some(Kind::Steam) => Launch::None,
            None => clamshell_library::RunnerOf { command: self.template(home).into(), ..clamshell_library::RunnerOf::new(&self.id, &self.name, true) }
                .command_for(&[path.to_path_buf()]),
        }
    }

    /// Its title, as its settings page shows it.
    fn title(&self) -> &str {
        &self.name
    }

    /// The runner in the form an entry keeps it, with `~/` expanded to the home dir in its commands.
    fn of(&self, home: Option<&Path>) -> clamshell_library::RunnerOf {
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

    /// Its command with `~/` expanded against `home`, if given, and `{path}`, `{paths}` and `{socket}`
    /// left in.
    fn template(&self, home: Option<&Path>) -> Vec<String> {
        self.launch.iter().map(|a| super::content::expand(a, home).to_string_lossy().into_owned()).collect()
    }

    /// A desktop entry's command as this runner launches it: the runner's `launch` filled in with the
    /// desktop file's `id` and its own command `exec`, and `~/` as the home dir. None if the runner has no
    /// `launch`, in which case the entry's own command is used.
    fn wrap(&self, id: &str, exec: &Launch, home: Option<&Path>) -> Option<Launch> {
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
    fn opens(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.patterns.iter().any(|p| pattern::matches(&p.to_lowercase(), &name))
    }

    /// Whether it opens file name `name` by a specific pattern, not the catch-all `*`.
    fn opens_as_its_own(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.patterns.iter().filter(|p| p.as_str() != "*").any(|p| pattern::matches(&p.to_lowercase(), &name))
    }
}

/// A command with `~/` expanded against `home`, if given.
fn expanded(args: &[String], home: Option<&Path>) -> std::sync::Arc<[String]> {
    args.iter().map(|a| super::content::expand(a, home).to_string_lossy().into_owned()).collect()
}

/// library.json's runners, checked, and their problems. Each gets an id, from its name if it has none,
/// and a name, from its id if it has none. A runner with neither, or one without a type and without a
/// command, is left out. A runner whose program isn't installed is kept but reported; `installed` says
/// whether a program is on the PATH.
pub(crate) fn checked(config: &[Runner], installed: &dyn Fn(&str) -> bool) -> (Vec<Runner>, Vec<String>) {
    let mut problems = Vec::new();
    let mut runners = Vec::new();
    for r in config {
        let id = r.effective_id();
        let name = if r.name.is_empty() { id.clone() } else { r.name.clone() };
        match (id.is_empty(), r.kind, r.launch.first()) {
            (true, _, _) => problems.push(format!("a runner without an id or a name: {}", r.launch.join(" "))),
            (false, None, None) => problems.push(format!("{name}: no launch command")),
            (false, None, Some(_)) if r.enqueue.is_some() && !r.launch.iter().any(|a| a.contains("{socket}")) => {
                problems.push(format!("{name}: enqueue needs {{socket}} in its launch command"));
                runners.push(Runner { id, name, enqueue: None, ..r.clone() });
            }
            (false, None, Some(_)) if r.enqueue.as_deref().is_some_and(|e| !json_with_path(e)) => {
                problems.push(format!("{name}: enqueue must be mpv's JSON form, {{path}} inside a string"));
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

/// Whether an `enqueue` is JSON with `{path}` only inside its strings, as mpv's JSON commands are: the
/// file's name goes in as a JSON string's contents, so it stays one argument. mpv also takes text
/// commands, where `;` starts another and `run` starts a program: a file named `a; run ...` would run it.
fn json_with_path(enqueue: &str) -> bool {
    enqueue.contains("{path}") && serde_json::from_str::<serde_json::Value>(&enqueue.replace("{path}", "x")).is_ok_and(|v| v.is_object())
}

/// Whether `program` is on the PATH.
pub(crate) fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// The runners in force here: library.json's, checked against the PATH (see [`checked`]).
pub(crate) fn in_force(config: &[Runner]) -> (Vec<Runner>, Vec<String>) {
    checked(config, &on_path)
}

/// The first runner that opens a file named `name`.
pub(crate) fn opener<'a>(runners: &'a [Runner], name: &str) -> Option<&'a Runner> {
    runners.iter().find(|p| p.opens(name))
}

/// The command and runner for file `path`, from the first of `runners` that opens its name.
pub(crate) fn open(runners: &[Runner], path: &Path, home: Option<&Path>) -> Option<(Launch, clamshell_library::RunnerOf)> {
    let runner = opener(runners, &path.file_name()?.to_string_lossy())?;
    Some((runner.command(path, home), runner.of(home)))
}

/// The runner of a type, if there is one.
pub(crate) fn of_kind(runners: &[Runner], kind: Kind) -> Option<&Runner> {
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

    /// An `enqueue` in mpv's text form, or with `{path}` outside a string, would let a file's name add
    /// commands: it is a problem, and the runner is kept without it.
    #[test]
    fn enqueue_must_be_json_with_the_path_in_a_string() {
        assert!(json_with_path(r#"{"command": ["loadfile", "{path}", "append-play"]}"#));
        for text in ["loadfile {path}", r#"{"command": ["loadfile", {path}]}"#, r#"["loadfile", "{path}"]"#, r#"{"command": ["playlist-next"]}"#] {
            assert!(!json_with_path(text), "{text}");
        }
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "name": "Video", "match": ["*.mkv"], "launch": ["mpv", "--input-ipc-server={socket}", "{paths}"], "enqueue": "loadfile {path}" }]"#,
        )
        .unwrap();
        let (runners, problems) = checked(&config, &|_| true);
        assert_eq!(problems, ["Video: enqueue must be mpv's JSON form, {path} inside a string"]);
        assert!(runners.iter().all(|r| r.of(None).enqueue.is_none()), "kept, without it");
    }

    #[test]
    fn enqueue_needs_a_socket_to_write_to() {
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "name": "Music", "match": ["*.mp3"], "launch": ["mpv", "{paths}"], "enqueue": "{\"command\": [\"loadfile\", \"{path}\"]}" },
                { "name": "Video", "match": ["*.mkv"], "launch": ["mpv", "--input-ipc-server={socket}", "{paths}"], "enqueue": "{\"command\": [\"loadfile\", \"{path}\"]}" }]"#,
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
