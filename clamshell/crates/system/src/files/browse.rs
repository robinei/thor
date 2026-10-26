//! The browse thread. It lists the folder shelf has open, and lists it again whenever the folder changes
//! (seen through inotify) or a removable filesystem is mounted or unmounted.

use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;

use clamshell_state::shelf::files::{Browse, BrowseCommand, FilesEvent};
use clamshell_state::{Browsed, Entry, Event};
use rustix::fs::inotify;

use super::mounts::media_mounts;
use crate::Sink;
use crate::library::content;
use crate::library::runners::{self, Runner};
use crate::os::worker::{Bell, Worker};

/// What a browse thread reads and how it handles videos. `config` is library.json, which says how files
/// open, and `home` is the home dir for `~/`. `art` is the art cache, holding videos' lengths and frames
/// as for scanned entries. `probe` has the videos not read yet read in the background; their results
/// arrive as metadata.
pub(crate) struct Browsing {
    pub(crate) config: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) art: Option<PathBuf>,
    pub(crate) probe: Box<dyn Fn(Vec<crate::library::meta::Wanted>) + Send>,
    /// Where archives' indexes are kept (see [`crate::files::archives`]).
    pub(crate) cache: Option<PathBuf>,
    /// ClamShell's runtime folder, where archives are mounted.
    pub(crate) runtime: Option<PathBuf>,
}

/// Starts the browse thread, which lists and watches the folder each command names (see [`Browsing`]).
pub(crate) fn spawn_browse(browsing: Browsing, sink: impl Sink) -> io::Result<Worker<BrowseCommand>> {
    let Browsing { config, home, art, probe, cache, runtime } = browsing;
    let bell = Bell::new()?;
    let theirs = bell.clone();
    Worker::spawn_belled(clamshell_state::Thread::Browse.name(), bell, move |commands| {
        let bell = &theirs;
        let mut archives = crate::files::archives::Mounts::new(runtime.as_deref(), cache.clone());
        // The videos' lengths and frames are read from this at each listing that has videos.
        let store = art.as_deref().map(crate::library::meta::cache::Cache::open);
        let mut in_use: Vec<PathBuf> = Vec::new();
        let notify = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).map_err(|e| log::warn!("browse: inotify: {e}")).ok();
        let flags = inotify::WatchFlags::CREATE
            | inotify::WatchFlags::DELETE
            | inotify::WatchFlags::MOVED_FROM
            | inotify::WatchFlags::MOVED_TO
            | inotify::WatchFlags::CLOSE_WRITE
            | inotify::WatchFlags::ATTRIB
            | inotify::WatchFlags::DELETE_SELF
            | inotify::WatchFlags::MOVE_SELF;
        // The open folder, the dir listed for it, and its watch.
        let mut open: Option<(Browse, PathBuf, Option<i32>)> = None;
        let mut mounts = media_mounts();
        // When the mount table changes, the kernel signals POLLPRI on this file.
        let mount_table = fs::File::open("/proc/self/mounts").map_err(|e| log::warn!("browse: mounts: {e}")).ok();
        // For an archive, `dir` is its mount.
        let list = |b: &Browse, dir: &Path| {
            let config = config.as_deref().map(content::read_config).unwrap_or_default().0;
            let (runners, _) = runners::in_force(&config.runners);
            let mut entries = list(dir, &b.folder, &runners, home.as_deref(), b.hidden);
            let wanted = known_videos(&mut entries, store.as_ref());
            if !wanted.is_empty() {
                probe(wanted);
            }
            sink.send(Event::Files(FilesEvent::Listed { folder: b.folder.clone(), entries }))
        };
        loop {
            // Sleep until a command comes, or the folder or the mounts change.
            let (folder_changed, mounts_changed) = wait(bell, notify.as_ref(), mount_table.as_ref());
            // A command and a change may come together, so the change is looked at too.
            match commands.try_recv() {
                Ok(BrowseCommand::InUse(files)) => {
                    in_use = files;
                    let open_dir = open.as_ref().map(|(b, ..)| b.dir.as_path());
                    archives.keep(&in_use.iter().map(PathBuf::as_path).chain(open_dir).collect::<Vec<_>>());
                    bell.ring();
                }
                Ok(BrowseCommand::Open(browse)) => {
                    if let (Some(fd), Some((_, _, Some(wd)))) = (notify.as_ref(), open.take())
                        && let Err(e) = inotify::remove_watch(fd, wd)
                    {
                        log::debug!("browse: removing the watch: {e}");
                    }
                    if let Some(fd) = notify.as_ref() {
                        changed(fd);
                    }
                    let Some(b) = browse else { continue };
                    // An archive is mounted and its mount listed. If it can't be mounted, the listing is
                    // empty and the user is told why.
                    let dir = match b.dir.is_file() {
                        true => archives.mount(&b.dir).unwrap_or_else(|e| {
                            let _ = sink.send(Event::Files(FilesEvent::Failed(e)));
                            PathBuf::new()
                        }),
                        false => b.dir.clone(),
                    };
                    let wd = notify
                        .as_ref()
                        .and_then(|fd| inotify::add_watch(fd, dir.as_path(), flags).map_err(|e| log::debug!("browse: {}: {e}", dir.display())).ok());
                    if list(&b, &dir).is_err() {
                        return;
                    }
                    archives.keep(&in_use.iter().map(PathBuf::as_path).chain([b.dir.as_path()]).collect::<Vec<_>>());
                    open = Some((b, dir, wd));
                    // Another command may be waiting, and the bell was cleared for all of them.
                    bell.ring();
                }
                Err(TryRecvError::Empty) => {
                    if folder_changed
                        && let (Some(fd), Some((b, dir, _))) = (notify.as_ref(), open.as_ref())
                        && changed(fd)
                        && list(b, dir).is_err()
                    {
                        return;
                    }
                }
                Err(TryRecvError::Disconnected) => return,
            }
            if mounts_changed {
                let now = media_mounts();
                if now != mounts {
                    mounts = now;
                    if sink.send(Event::Files(FilesEvent::Mounts)).is_err() {
                        return;
                    }
                }
            }
        }
    })
}

/// Waits for the browse thread's bell, the folder's inotify events or a change of the mounts. Returns
/// whether the folder changed and whether the mounts changed.
fn wait(bell: &Bell, notify: Option<&rustix::fd::OwnedFd>, mounts: Option<&fs::File>) -> (bool, bool) {
    use rustix::event::{PollFd, PollFlags};
    let mut fds = vec![PollFd::from_borrowed_fd(bell.fd(), PollFlags::IN)];
    let folder = notify.map(|fd| {
        fds.push(PollFd::new(fd, PollFlags::IN));
        fds.len() - 1
    });
    let table = mounts.map(|f| {
        fds.push(PollFd::new(f, PollFlags::PRI));
        fds.len() - 1
    });
    if let Err(e) = rustix::event::poll(&mut fds, None) {
        log::debug!("browse: waiting: {e}");
        // If interrupted, act as if anything changed.
        return (true, true);
    }
    bell.clear();
    let came = |i: Option<usize>, what: PollFlags| i.and_then(|i| fds.get(i)).is_some_and(|f| f.revents().intersects(what));
    (came(folder, PollFlags::IN), came(table, PollFlags::PRI | PollFlags::ERR))
}

/// Whether inotify has events. Reads them all.
fn changed(fd: &rustix::fd::OwnedFd) -> bool {
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buf);
    let mut any = false;
    while reader.next().is_ok() {
        any = true;
    }
    any
}

/// The entries of `dir`, the dir of folder `folder`, with ids made of the folder's id, `/` and their
/// names. Folders come first, then the rest by name. Hidden entries (`.name`) are included only if
/// `hidden`. A picture is its own art. A file opens with the first of `runners` that opens its name, with
/// `~/` in its command expanded against `home`.
fn list(dir: &Path, folder: &str, runners: &[Runner], home: Option<&Path>, hidden: bool) -> Vec<Entry> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) => {
            if e.kind() == io::ErrorKind::NotFound {
                log::debug!("browse: {}: {e}", dir.display());
            } else {
                log::warn!("browse: {}: {e}", dir.display());
            }
            return Vec::new();
        }
    };
    let mut games: Vec<Entry> = read
        .filter_map(|e| e.inspect_err(|e| log::debug!("browse: {}: {e}", dir.display())).ok())
        .filter_map(|e| {
            let name = e
                .file_name()
                .into_string()
                .inspect_err(|n| log::debug!("browse: {}: not UTF-8: skipped", n.display()))
                .ok()
                .filter(|n| hidden || !n.starts_with('.'))?;
            let path = e.path();
            // For a link, this is what it points at.
            let meta = fs::metadata(&path).inspect_err(|e| log::debug!("browse: {}: {e}", path.display())).ok();
            let is_dir = meta.as_ref().is_some_and(fs::Metadata::is_dir);
            let size = meta.as_ref().filter(|m| m.is_file()).map(fs::Metadata::len);
            let modified = meta.and_then(|m| m.modified().ok()).map_or(0, crate::time::clock::unix_secs_at);
            let mut game = Entry::new(&format!("{folder}/{name}"), &name);
            // An archive is browsed as a folder through its mount. A runner that opens it is kept for
            // Open in.
            let archive = !is_dir && crate::files::archives::is_archive(&name);
            // For an archive, only a runner whose own patterns match it counts, as for a ROM's zip, and
            // not a catch-all runner.
            let opener = runners::open(runners, &path, home).filter(|_| !is_dir);
            if let Some((launch, runner)) = opener.filter(|_| !archive || runners.iter().find(|r| r.opens(&name)).is_some_and(|r| r.opens_as_its_own(&name))) {
                game.launch = launch;
                game.runner = Some(runner);
            }
            if is_dir {
                (game.runner, game.playlist) = enqueued(&path, runners, home);
            }
            // Its picture is found as for a scanned entry. Without one, a glyph shows what it is.
            game.box_front = content::picture_of(&path);
            if game.box_front.is_none() {
                game.glyph = Some(
                    if is_dir {
                        "folder"
                    } else if archive {
                        "archive"
                    } else {
                        "file"
                    }
                    .into(),
                );
            }
            let locked = archive || (is_dir && rustix::fs::access(&path, rustix::fs::Access::WRITE_OK).is_err());
            game.file = Some(path);
            game.browsed = Some(Browsed { folder: is_dir || archive, archive, locked, size, modified, ..Browsed::default() });
            Some(game)
        })
        .collect();
    games.sort_by(|a, b| (!a.is_group(), &a.sort_key, &a.title).cmp(&(!b.is_group(), &b.sort_key, &b.title)));
    games
}

/// How enqueueing the unlisted folder `dir` works, from a look at its first names: the runner that can
/// enqueue its first enqueueable file, and its playlist, an `.m3u` in it. `~/` in the runner's commands
/// is expanded against `home`.
fn enqueued(dir: &Path, runners: &[Runner], home: Option<&Path>) -> (Option<clamshell_library::RunnerOf>, Option<PathBuf>) {
    let (mut runner, mut playlist) = (None, None);
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) => {
            log::debug!("browse: {}: {e}", dir.display());
            return (None, None);
        }
    };
    for e in read.flatten().take(PEEK) {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with('.') {
            continue;
        }
        if playlist.is_none() && (name.ends_with(".m3u") || name.ends_with(".m3u8")) {
            playlist = Some(e.path());
        }
        if runner.is_none() {
            runner = runners::opener(runners, &name).filter(|r| r.enqueue.is_some()).map(|r| r.of(home));
        }
        if runner.is_some() && playlist.is_some() {
            break;
        }
    }
    let playlist = playlist.filter(|_| runner.is_some());
    (runner, playlist)
}

/// How many names of a folder [`enqueued`] looks at.
const PEEK: usize = 256;

/// Fills in the lengths and frames of the mpv videos among `entries` from the cache, which has those read
/// before, here or as scanned entries. Returns the videos the cache doesn't have, to be read.
fn known_videos(entries: &mut [Entry], store: Option<&crate::library::meta::cache::Cache>) -> Vec<crate::library::meta::Wanted> {
    if !entries.iter().any(|g| g.mpv_file().is_some()) {
        return Vec::new();
    }
    let probes = store.map(crate::library::meta::cache::Cache::probes).unwrap_or_default();
    let mut wanted = Vec::new();
    for g in entries.iter_mut() {
        let Some(file) = g.mpv_file().map(Path::to_path_buf) else {
            continue;
        };
        match probes.get(&*g.id).filter(|p| p.frame.as_ref().is_none_or(|f| f.exists())) {
            Some(p) => {
                g.playback.duration = p.seconds;
                g.details.shot.clone_from(&p.frame);
                // A video's frame is its picture, not the file glyph.
                g.glyph = None;
            }
            None => {
                g.glyph = None;
                wanted.push(crate::library::meta::Wanted {
                    id: g.id.clone(),
                    key: g.key.clone(),
                    title: g.title.clone(),
                    file: Some(file),
                    how: crate::library::meta::How::Duration,
                    art: true,
                });
            }
        }
    }
    wanted
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_has_folders_first_and_files_open_by_the_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir(dir.join("Zelda")).unwrap();
        for f in ["a.SFC", "b.txt", ".hidden", "cover.png"] {
            fs::write(dir.join(f), "x").unwrap();
        }
        fs::write(dir.join("tool.desktop"), "[Desktop Entry]\nType=Application\nName=Tool\nExec=tool --open %U\n").unwrap();
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "name": "SNES", "match": ["*.sfc"], "launch": ["retroarch", "{path}"] },
                { "type": "desktop", "name": "Desktop", "match": ["*.desktop"] },
                { "name": "Other files", "match": ["*"], "launch": ["xdg-open", "{path}"] }]"#,
        )
        .unwrap();
        let (runners, _) = runners::checked(&config, &|_| true);
        let games = list(dir, "browse:Files/Home", &runners, None, false);
        let names: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(names, ["Zelda", "a.SFC", "b.txt", "cover.png", "tool.desktop"]);
        assert_eq!(&*games[0].id, "browse:Files/Home/Zelda");
        assert!(games[0].is_group());
        let argv = |g: &Entry| match &g.launch {
            clamshell_state::Launch::Program { argv, .. } => argv.clone(),
            _ => vec![],
        };
        assert_eq!(argv(&games[1])[0], "retroarch", "by the runner's pattern, any case");
        assert_eq!(games[1].runner.as_ref().map(|r| (&*r.key, &*r.name)), Some(("runner:snes", "SNES")));
        assert_eq!(argv(&games[2])[0], "xdg-open");
        assert_eq!(games[3].box_front.as_deref(), Some(dir.join("cover.png").as_path()), "a picture is its own art");
        assert_eq!(games[2].browsed.as_ref().unwrap().size, Some(1));
        assert_eq!(argv(&games[4]), ["tool", "--open"], "a desktop file: its own command");
        // Pictures as in a scan: a folder's cover in it, a file's beside it, an image itself, else a glyph.
        fs::write(dir.join("Zelda/cover.jpg"), "x").unwrap();
        fs::write(dir.join("b.jpg"), "x").unwrap();
        let games = list(dir, "browse:Files/Home", &runners, None, false);
        let by = |name: &str| games.iter().find(|g| g.title == name).unwrap();
        assert_eq!(by("Zelda").box_front.as_deref(), Some(dir.join("Zelda/cover.jpg").as_path()));
        assert_eq!(by("b.txt").box_front.as_deref(), Some(dir.join("b.jpg").as_path()), "beside it");
        assert_eq!(by("b.jpg").box_front.as_deref(), Some(dir.join("b.jpg").as_path()), "itself");
        assert_eq!((by("a.SFC").box_front.as_ref(), by("a.SFC").glyph.as_deref()), (None, Some("file")));
        assert_eq!(by("tool.desktop").runner.as_ref().map(|r| &*r.name), Some("Desktop"));
    }
}
