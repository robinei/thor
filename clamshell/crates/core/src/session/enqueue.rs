//! Enqueue (see docs/runners.md) adds what the details show to what its runner plays. A runner that
//! declares `enqueue`, such as mpv with `loadfile <file> append-play`, takes more while it runs, through
//! its launch's control socket. The details' entry, or in the Files tab the selected entries, goes to the
//! launch of its runner that was most recently in front. If none is running, the runner is launched with
//! all of them.
//!
//! A file is queued as itself. A folder is queued as its playlist, an `.m3u` in it, so that nothing is
//! queued twice. Without one, it is queued as its files for that runner, all the way down, in list order
//! (see [`clamshell_library::Library::queue_files`]).

use std::path::PathBuf;
use std::sync::Arc;

use clamshell_library::RunnerOf;

use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::session::running::Running;
use crate::state::out::Out;
use crate::{Effect, Session, Shelf};

impl Session {
    /// The running entry of runner `key` that was most recently in front. Only entries in a scope of ours
    /// count, not those known by app id.
    pub fn enqueue_target(&self, key: &str) -> Option<&Running> {
        let runner_of = |r: &Running| r.entry.as_ref().and_then(|g| g.runner.as_ref()).is_some_and(|o| &*o.key == key);
        let candidates = self.running.iter().filter(|r| r.app_id.is_none() && runner_of(r));
        // Entries never in front come after those that were, the last started first.
        candidates.rev().min_by_key(|r| self.recent.iter().position(|w| *w == r.workspace()).unwrap_or(usize::MAX))
    }
}

/// What Enqueue adds: the files, in order, for `runner`, and the entry that plays them if it has to be
/// started. `alone` says that entry is the details' entry itself, a file rather than a folder.
#[derive(Debug)]
pub(crate) struct Enqueued {
    pub(crate) entry: Arc<str>,
    pub(crate) alone: bool,
    pub(crate) runner: RunnerOf,
    pub(crate) files: Vec<PathBuf>,
}

impl Shelf {
    /// What Enqueue adds from what the details show, if its runner takes more while it runs. The selected
    /// files are taken: the selection is cleared.
    pub(crate) fn take_enqueued(&mut self) -> Option<Enqueued> {
        let Some((target, true)) = self.file_target() else {
            return None;
        };
        let runner = target.runner.clone().filter(|r| r.enqueue.is_some())?;
        let library = self.library();
        let index_of = |id: &str| library.entries.iter().position(|g| &*g.id == id);
        // While there is a selection, its entries are queued in the order picked, and the open folder is
        // the entry that plays them.
        let picked = self.files().selected();
        let (entry, ids): (Arc<str>, Vec<Arc<str>>) = match self.group().filter(|_| !picked.is_empty()) {
            Some(folder) => (folder.id.clone(), picked.to_vec()),
            None => (target.id.clone(), vec![target.id.clone()]),
        };
        let files: Vec<PathBuf> = ids.iter().filter_map(|id| index_of(id)).flat_map(|i| library.queue_files(i, &runner.key)).collect();
        if files.is_empty() {
            return None;
        }
        let alone = entry == target.id && !target.is_group();
        self.clear_file_selection();
        Some(Enqueued { entry, alone, runner, files })
    }
}

impl Session {
    /// Adds what is `enqueued` to the running launch of its runner that was most recently in front (see
    /// [`Session::enqueue_target`]), saying so in a notice. Returns whether there was one.
    pub(crate) fn enqueue(&self, enqueued: &Enqueued, notice: &mut NoticeSlot, now: u64, out: &mut Out) -> bool {
        let Some(r) = self.enqueue_target(&enqueued.runner.key) else {
            return false;
        };
        let (key, title) = (r.key.clone(), r.entry.as_ref().map_or_else(String::new, |g| g.title.clone()));
        let lines = enqueued.files.iter().filter_map(|f| line(&enqueued.runner, f)).collect();
        notice.show(NoticeKind::Enqueued(enqueued.files.len(), title), now);
        out.effects.push(Effect::Enqueue { key, lines });
        true
    }
}

/// The line that adds `file` to a launch of `runner`: its `enqueue`, with `{path}` replaced by the file as
/// the contents of a JSON string.
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
    use crate::{Collection, Entry, Id, Launch, Library, ShelfId, State};

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
        let entries = vec![album, a, b, song("single", "/m/single.mp3")];
        state_with(Library { collections: vec![Collection::new("Music", vec![0, 3])], entries, ..Library::default() })
    }

    #[test]
    fn enqueue_plays_with_nothing_playing_then_adds_to_what_plays() {
        let mut s = music_state();
        assert_eq!(s.shelf().selected().map(|g| &**g), Some("album"));
        assert!(s.shelf().actions().contains(&crate::shelf::actions::Action::Enqueue));
        let fx = tap(&mut s, Id::Shelf(ShelfId::Enqueue));
        let launch = fx.iter().find_map(|e| match e {
            Effect::Launch { entry, launch, .. } => Some((entry.to_string(), launch.clone())),
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
