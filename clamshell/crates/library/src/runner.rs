//! Runners: the programs an entry's files open with (mpv, an emulator), each with its own settings.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{Launch, ids};

/// The key of runner `id`: its settings, environment and own keymap are kept by it (`runner:snes`).
pub fn runner_key(id: &str) -> Arc<str> {
    format!("{}{id}", ids::RUNNER).into()
}

/// An entry's runner: its key ([`runner_key`] of its id, which outlives a new name), its name (shown),
/// whether its entries have settings of their own (else only the runner's); its command (`{path}`,
/// `{paths}`, `{socket}` in it to fill in) and what to write to a launch's control socket to add a file
/// to it (`{path}` in it the file, as a JSON string's contents), if it can take more while it runs;
/// whether its launches hand their app over (see the system crate's `Runner::handover`) and the command
/// that ends one (`{id}` its windows' app id; empty: none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerOf {
    pub key: Arc<str>,
    pub name: Arc<str>,
    pub item_settings: bool,
    pub command: Arc<[String]>,
    pub enqueue: Option<Arc<str>>,
    pub handover: bool,
    pub stop: Arc<[String]>,
    /// Commands that pause and resume all its entries together (they share one container: Waydroid's);
    /// empty: none.
    pub pause: Arc<[String]>,
    pub resume: Arc<[String]>,
    /// A command that succeeds while its shown app keeps the screen on in a way the compositor doesn't
    /// hear (Android's: a video, the camera); empty: none.
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

    /// Its command for `files` (several where it takes `{paths}`, else the first), in the first's folder;
    /// `{socket}` left for the launcher.
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
