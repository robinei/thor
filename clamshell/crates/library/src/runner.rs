//! Runners are the programs an entry's files open with, such as mpv or an emulator. Each has its own
//! settings.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{Launch, ids};

/// The key of runner `id`, such as `runner:snes`. The runner's settings, environment and keymap are kept
/// under it.
pub fn runner_key(id: &str) -> Arc<str> {
    format!("{}{id}", ids::RUNNER).into()
}

/// An entry's runner.
///
/// - `key` is the [`runner_key`] of its id, so it stays the same if the runner is renamed.
/// - `name` is shown.
/// - `item_settings` says whether its entries have settings of their own, rather than only the runner's.
/// - `command` is its command, with `{path}`, `{paths}` and `{socket}` to fill in.
/// - `enqueue`, if the runner can take more files while it runs, is what to write to a launch's control
///   socket to add a file. `{path}` in it is replaced by the file, escaped as JSON string contents.
/// - `handover` says whether its launches hand their app over (see the system crate's
///   `Runner::handover`).
/// - `stop` is the command that ends a launch, with `{id}` for its windows' app id. Empty means none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerOf {
    pub key: Arc<str>,
    pub name: Arc<str>,
    pub item_settings: bool,
    pub command: Arc<[String]>,
    pub enqueue: Option<Arc<str>>,
    pub handover: bool,
    pub stop: Arc<[String]>,
    /// Commands that pause and resume all its entries together, because they share one container (as
    /// Waydroid's do). Empty means none.
    pub pause: Arc<[String]>,
    pub resume: Arc<[String]>,
    /// A command that succeeds while the app on screen keeps the screen on in a way the compositor doesn't
    /// see, such as an Android app playing a video or using the camera. Empty means none.
    pub awake: Arc<[String]>,
}

impl RunnerOf {
    pub fn new(id: &str, name: &str, item_settings: bool) -> RunnerOf {
        RunnerOf {
            key: runner_key(id),
            name: name.into(),
            item_settings,
            command: Arc::new([]),
            enqueue: None,
            handover: false,
            stop: Arc::new([]),
            pause: Arc::new([]),
            resume: Arc::new([]),
            awake: Arc::new([]),
        }
    }

    /// The runner's command for `files`, run in the first file's folder. `{paths}` takes all the files and
    /// `{path}` the first. `{socket}` is left for the launcher to fill in.
    pub fn command_for(&self, files: &[PathBuf]) -> Launch {
        let path = |p: &PathBuf| p.to_string_lossy().into_owned();
        let mut argv = Vec::new();
        for arg in self.command.iter() {
            match arg.as_str() {
                "{paths}" => argv.extend(files.iter().map(path)),
                _ => argv.push(arg.replace("{path}", &files.first().map(path).unwrap_or_default())),
            }
        }
        Launch::Program { argv, workdir: files.first().and_then(|f| f.parent()).map(std::path::Path::to_path_buf) }
    }
}
