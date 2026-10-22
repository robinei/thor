//! Enqueue (see docs/runners.md): what the details show added to what its runner plays. A runner that
//! declares `enqueue` (mpv: `loadfile <file> append-play`) takes more while it runs, by its launch's
//! control socket. The details' entry (or, in the Files tab, the entries selected) goes to the launch of
//! its runner most recently in front; with none running, it is launched with them all.
//!
//! A file is itself; a folder its playlist (an `.m3u` in it, so nothing is queued twice), else its files
//! of that runner, all the way down, in list order (see [`clamshell_library::Library::queue_files`]).

use std::path::PathBuf;
use std::sync::Arc;

use clamshell_library::RunnerOf;

use crate::notice::NoticeKind;
use crate::running::Running;
use crate::{Effect, State};

impl State {
    /// The running entry of runner `key` most recently in front (in a scope of ours: not known by app id).
    pub fn enqueue_target(&self, key: &str) -> Option<&Running> {
        let session = &self.session;
        let runner_of = |r: &Running| r.entry.as_ref().and_then(|g| g.runner.as_ref()).is_some_and(|o| &*o.key == key);
        let candidates = session.running.iter().filter(|r| r.app_id.is_none() && runner_of(r));
        // (Never in front: after those that were, the last started first.)
        candidates.rev().min_by_key(|r| session.recent.iter().position(|w| *w == r.workspace()).unwrap_or(usize::MAX))
    }

    /// Enqueue pressed: the entries added to the running launch of their runner, else launched.
    pub(crate) fn enqueue_selected(&mut self) -> Vec<Effect> {
        let Some((game, true)) = self.file_target() else {
            return vec![];
        };
        let Some(runner) = game.runner.clone().filter(|r| r.enqueue.is_some()) else {
            return vec![];
        };
        let library = self.shelf.library();
        let index_of = |id: &str| library.entries.iter().position(|g| &*g.id == id);
        // (The selection, in the order picked, while there is one; the folder open is what plays them.)
        let (entry, ids): (Arc<str>, Vec<Arc<str>>) = match self.shelf.group().filter(|_| !self.files.selected.is_empty()) {
            Some(folder) => (folder.id.clone(), self.files.selected.clone()),
            None => (game.id.clone(), vec![game.id.clone()]),
        };
        let files: Vec<PathBuf> = ids.iter().filter_map(|id| index_of(id)).flat_map(|i| library.queue_files(i, &runner.key)).collect();
        if files.is_empty() {
            return vec![];
        }
        let alone = entry == game.id && !game.is_group();
        self.files.selected.clear();
        match self.enqueue_target(&runner.key) {
            Some(r) => {
                let (key, title) = (r.key.clone(), r.entry.as_ref().map_or_else(String::new, |g| g.title.clone()));
                let lines = files.iter().filter_map(|f| line(&runner, f)).collect();
                self.notify(NoticeKind::Enqueued(files.len(), title));
                vec![Effect::Enqueue { key, lines }]
            }
            // (A file alone: opened as A opens it, where it was left included.)
            None if alone => self.play(entry),
            None => {
                let entry = self.shelf.entry_of_file(&entry).unwrap_or(entry);
                self.launch_with(entry, Some(runner.command_for(&files)))
            }
        }
    }
}

/// The line that adds `file` to a launch of `runner`: its `enqueue`, `{path}` the file as a JSON string's
/// contents.
fn line(runner: &RunnerOf, file: &std::path::Path) -> Option<String> {
    let quoted = serde_json::to_string(&file.to_string_lossy()).ok()?;
    let escaped = quoted.strip_prefix('"')?.strip_suffix('"')?;
    Some(runner.enqueue.as_ref()?.replace("{path}", escaped))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::{state_with, tap};
    use crate::{Collection, Entry, Id, Launch, Library, ShelfId};

    fn music() -> RunnerOf {
        RunnerOf {
            command: vec!["mpv".into(), "--input-ipc-server={socket}".into(), "{paths}".into()].into(),
            enqueue: Some(r#"{"command": ["loadfile", "{path}", "append-play"]}"#.into()),
            ..RunnerOf::new("music", "Music", true)
        }
    }

    /// Music: an album of two songs, and a song alone.
    fn music_state() -> State {
        let song = |id: &str, file: &str| Entry { file: Some(file.into()), runner: Some(music()), ..Entry::new(id, id) };
        let album = Entry { items: vec![1, 2], ..Entry::new("album", "Album") };
        let (mut a, mut b) = (song("a", "/m/Album/a.mp3"), song("b", "/m/Album/b.mp3"));
        (a.parent, b.parent) = (Some(0), Some(0));
        let album = Entry { runner: Some(music()), ..album };
        let games = vec![album, a, b, song("single", "/m/single.mp3")];
        state_with(Library { collections: vec![Collection::new("Music", vec![0, 3])], entries: games, ..Library::default() })
    }

    #[test]
    fn enqueue_plays_with_nothing_playing_then_adds_to_what_plays() {
        let mut s = music_state();
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("album"));
        assert!(s.actions().contains(&crate::actions::Action::Enqueue));
        let fx = tap(&mut s, Id::Shelf(ShelfId::Enqueue));
        let launch = fx.iter().find_map(|e| match e {
            Effect::Launch { entry: game, launch, .. } => Some((game.to_string(), launch.clone())),
            _ => None,
        });
        let argv = ["mpv", "--input-ipc-server={socket}", "/m/Album/a.mp3", "/m/Album/b.mp3"].map(String::from).to_vec();
        assert_eq!(launch, Some(("album".into(), Launch::Program { argv, workdir: Some("/m/Album".into()) })), "the album's songs: {fx:?}");
        crate::testkit::start(&mut s, "album");
        tap(&mut s, Id::Shelf(ShelfId::Entry("single".into())));
        let fx = tap(&mut s, Id::Shelf(ShelfId::Enqueue));
        let key = s.session().running("album").unwrap().key.clone();
        let line = r#"{"command": ["loadfile", "/m/single.mp3", "append-play"]}"#.to_owned();
        assert_eq!(fx, vec![Effect::Enqueue { key, lines: vec![line] }], "added to the album playing");
    }

    #[test]
    fn a_line_has_the_path_json_escaped() {
        let runner = RunnerOf { enqueue: Some(r#"{"command": ["loadfile", "{path}", "append-play"]}"#.into()), ..RunnerOf::new("music", "Music", true) };
        assert_eq!(
            line(&runner, std::path::Path::new("/m/A \"b\"\\c.mp3")).as_deref(),
            Some(r#"{"command": ["loadfile", "/m/A \"b\"\\c.mp3", "append-play"]}"#)
        );
    }
}
