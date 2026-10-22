//! What ClamShell keeps, as files in its state dir (~/.local/share/clamshell), JSON a person can read and
//! edit. No file grows and is written again whole:
//!
//! - What the state saves whole (see [`Save`]): one file each, replaced by writing a new one and renaming
//!   it over the old (a write cut short by a crash or power loss leaves the last complete one):
//!   - `settings/<screen>.json`: a settings screen's own settings and environment, one file per screen:
//!     `all.json`, `runner-<key>.json` (a runner's level), `entry-<key>.json` (an entry), the key as the
//!     entry's units and art are named (see `clamshell_library::key_of`), the screen's id in the file;
//!     a screen with nothing set has none;
//!   - `keymaps/<name>.json`: a keymap each, named as it is (an entry's or a runner level's own: as its
//!     settings screen's file); no folder: the initial ones;
//!   - `window-rules.json`, `dock.json`, `autostart.json`, `favourites.json`, `resume.json` (where shelf and
//!     the second screen were), `sticks.json` (the sticks' calibrations, by controller), `brightness.json`,
//!     `clock.json`, `setup.json` (the setup done: see `clamshell_core::account`).
//! - `plays.jsonl`: an entry's plays, a line each (its id, start in seconds since the epoch, duration in
//!   seconds), appended; a line cut short is skipped.
//!
//! What was fetched about an entry, and a video's length, are kept in the art cache, a file per entry
//! beside its images ([`Cache`]).
//!
//! ClamShell's own writes go through a thread of their own ([`Writer`]): the disk mustn't hold up the
//! screens. A file read back that doesn't make sense is logged, and left out.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use clamshell_core::bottom_screen::{WindowRule, WindowRules};
use clamshell_core::keymap::{Keymap, Keymaps};
use clamshell_core::levels::Brightness;
use clamshell_core::settings::{Setting, Settings};
use clamshell_core::{Dock, Resume, Save};
use clamshell_library::{Meta, Plays, key_of};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A video's length and a frame of it, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub seconds: Option<u32>,
    pub frame: Option<PathBuf>,
}

/// What was fetched about an entry, from where (as [`crate::meta::How::source`] says) and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub source: String,
    pub at: u64,
    pub meta: Meta,
}

/// Writes `bytes` to `path` whole: to a file beside it, flushed to the disk, then renamed over it (and
/// the rename made to last).
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no folder"))?;
    fs::create_dir_all(dir)?;
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut file = fs::File::create(&part)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&part, path)?;
    fs::File::open(dir)?.sync_all()
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    text.push('\n');
    write_file(path, text.as_bytes())
}

/// A file read as `T`: none if it isn't there, or doesn't read (logged).
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path)
        .map_err(|e| {
            if e.kind() != io::ErrorKind::NotFound {
                log::warn!("{}: {e}", path.display())
            }
        })
        .ok()?;
    serde_json::from_str(&text).map_err(|e| log::warn!("{}: {e}", path.display())).ok()
}

/// A settings screen's file.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Screen {
    /// The screen's id: an entry's, a runner level's (`runner:<name>`); none for All.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    /// Its settings by name (All: those that aren't their default).
    #[serde(default)]
    settings: BTreeMap<String, u16>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    env: String,
}

#[derive(Serialize, Deserialize)]
struct Rule {
    entry: String,
    app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ResumeFile {
    collection: String,
    tab: String,
    /// Each collection's place: its open groups' ids, then its selected entry's.
    places: Vec<(String, Vec<String>)>,
}

#[derive(Serialize, Deserialize)]
struct BrightnessFile {
    #[serde(default)]
    level: Option<u8>,
    /// Each backlight's share of the level (0-100).
    #[serde(default)]
    shares: BTreeMap<String, u8>,
}

#[derive(Serialize, Deserialize)]
struct ClockFile {
    /// 12 or 24.
    hours: u8,
}

#[derive(Serialize, Deserialize)]
struct SetupFile {
    done: bool,
}

#[derive(Serialize, Deserialize)]
struct Play {
    id: String,
    start: u64,
    duration: u64,
}

/// The state dir's files.
pub struct Store {
    dir: PathBuf,
    /// The settings screens' files as last written (by name), to write only those that changed.
    screens: Option<HashMap<String, Screen>>,
    /// The keymaps' files as last written, likewise.
    keymaps: Option<HashMap<String, Keymap>>,
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        Store { dir: dir.to_path_buf(), screens: None, keymaps: None }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Saves what the state keeps, replacing what was saved of it.
    pub fn save(&mut self, save: &Save) -> io::Result<()> {
        let ids = |ids: &[Arc<str>]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        match save {
            Save::Settings(s) => self.save_settings(s),
            Save::Keymaps(k) => self.save_keymaps(k),
            Save::Favourites(list) => write_json(&self.file("favourites.json"), list),
            Save::Resume(r) => {
                write_json(&self.file("resume.json"), &ResumeFile { collection: r.collection.clone(), tab: r.tab.clone(), places: r.places.clone() })
            }
            Save::Brightness(b) => write_json(
                &self.file("brightness.json"),
                &BrightnessFile { level: b.level, shares: b.shares.iter().map(|s| (s.backlight.clone(), s.share)).collect() },
            ),
            Save::TwelveHour(on) => write_json(&self.file("clock.json"), &ClockFile { hours: if *on { 12 } else { 24 } }),
            Save::WindowRules(r) => {
                let rules: Vec<Rule> = r.list().iter().map(|r| Rule { entry: r.owner.to_string(), app_id: r.app_id.clone(), title: r.title.clone() }).collect();
                write_json(&self.file("window-rules.json"), &rules)
            }
            Save::Dock(list) => write_json(&self.file("dock.json"), &ids(list)),
            Save::Autostart(list) => write_json(&self.file("autostart.json"), &ids(list)),
            Save::SetupDone => write_json(&self.file("setup.json"), &SetupFile { done: true }),
            Save::Sticks(c) => write_json(&self.file("sticks.json"), c),
        }
    }

    /// Every entry's plays, summed up.
    pub fn plays(&self) -> HashMap<String, Plays> {
        let mut out: HashMap<String, Plays> = HashMap::new();
        let Ok(text) = fs::read_to_string(self.file("plays.jsonl")) else {
            return out;
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Play>(line) {
                Ok(p) => out.entry(p.id).or_default().add(p.start, p.duration),
                Err(e) => log::warn!("plays.jsonl: {e}: {line}"),
            }
        }
        out
    }

    /// Adds a play of `id`: a line appended (whole, in one write), flushed to the disk.
    pub fn add_play(&self, id: &str, start: u64, duration: u64) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut line = serde_json::to_string(&Play { id: id.to_owned(), start, duration }).map_err(io::Error::other)?;
        line.push('\n');
        let mut file = fs::OpenOptions::new().create(true).append(true).open(self.file("plays.jsonl"))?;
        file.write_all(line.as_bytes())?;
        file.sync_data()
    }

    pub fn favourites(&self) -> HashSet<String> {
        read_json::<Vec<String>>(&self.file("favourites.json")).unwrap_or_default().into_iter().collect()
    }

    /// The settings and environment (settings it doesn't know, or values they don't take, skipped).
    pub fn settings(&mut self) -> Settings {
        let mut s = Settings::default();
        for screen in self.screens_on_disk().values() {
            let id: Option<Arc<str>> = screen.id.as_deref().map(Arc::from);
            for (name, &value) in &screen.settings {
                match Setting::from_name(name) {
                    Some(setting) if setting.accepts(value) => s.set(id.as_ref(), setting, Some(value)),
                    _ => log::warn!("settings: {name} = {value} isn't a setting it takes: skipped"),
                }
            }
            if !screen.env.is_empty() {
                s.set_env(id, &screen.env);
            }
        }
        s
    }

    /// The settings screens' files there are, by name.
    fn screens_on_disk(&mut self) -> &HashMap<String, Screen> {
        let dir = self.file("settings");
        self.screens.get_or_insert_with(|| read_folder(&dir))
    }

    /// The settings as one file per screen, written where it changed; the files of screens with nothing
    /// set any more removed.
    fn save_settings(&mut self, s: &Settings) -> io::Result<()> {
        let mut screens: HashMap<String, Screen> = HashMap::new();
        let global = Screen {
            id: None,
            settings: Setting::ALL.iter().filter(|&&k| s.global.get(k) != k.default_value()).map(|&k| (k.name().to_owned(), s.global.get(k))).collect(),
            env: s.global_env.clone(),
        };
        screens.insert("all.json".into(), global);
        let ids = s.overrides.keys().chain(s.env.keys());
        for id in ids {
            let settings =
                s.overrides.get(id).map(|o| Setting::ALL.iter().filter_map(|&k| Some((k.name().to_owned(), o.get(k)?))).collect()).unwrap_or_default();
            let env = s.env.get(id).cloned().unwrap_or_default();
            screens.insert(screen_file(id), Screen { id: Some(id.to_string()), settings, env });
        }
        screens.retain(|_, screen| !screen.settings.is_empty() || !screen.env.is_empty());
        let before = std::mem::take(self.screens_on_disk_mut());
        write_folder(&self.file("settings"), &before, &screens)?;
        self.screens = Some(screens);
        Ok(())
    }

    fn screens_on_disk_mut(&mut self) -> &mut HashMap<String, Screen> {
        self.screens_on_disk();
        self.screens.get_or_insert_default()
    }

    /// The keymaps, by id; none if they were never saved (a keymap that doesn't read back is skipped,
    /// logged).
    pub fn keymaps(&self) -> Option<Keymaps> {
        let dir = self.file("keymaps");
        if !dir.is_dir() {
            return None;
        }
        let mut list: Vec<Keymap> = read_folder(&dir).into_values().collect();
        list.sort_by_key(|k| k.id);
        Some(Keymaps::from_list(list))
    }

    /// The keymaps as a file each, written where it changed; the files of those gone removed.
    fn save_keymaps(&mut self, k: &Keymaps) -> io::Result<()> {
        let dir = self.file("keymaps");
        fs::create_dir_all(&dir)?;
        let before = self.keymaps.take().unwrap_or_else(|| read_folder(&dir));
        let files: HashMap<String, Keymap> = k.list().iter().map(|k| (keymap_file(k), k.clone())).collect();
        write_folder(&dir, &before, &files)?;
        self.keymaps = Some(files);
        Ok(())
    }

    pub fn window_rules(&self) -> WindowRules {
        let rules: Vec<Rule> = read_json(&self.file("window-rules.json")).unwrap_or_default();
        WindowRules::from_list(rules.into_iter().map(|r| WindowRule { owner: r.entry.into(), app_id: r.app_id, title: r.title }).collect())
    }

    /// The dock's pinned entries, in order.
    pub fn dock(&self) -> Dock {
        Dock::pinned(read_json::<Vec<String>>(&self.file("dock.json")).unwrap_or_default().into_iter().map(Arc::from).collect())
    }

    /// The entries started with ClamShell.
    pub fn autostart(&self) -> Vec<Arc<str>> {
        read_json::<Vec<String>>(&self.file("autostart.json")).unwrap_or_default().into_iter().map(Arc::from).collect()
    }

    /// Where shelf and the second screen were; none if that was never saved.
    pub fn resume(&self) -> Option<Resume> {
        let r: ResumeFile = read_json(&self.file("resume.json"))?;
        Some(Resume { collection: r.collection, tab: r.tab, places: r.places })
    }

    /// Whether the setup was done (else it opens at the start).
    pub fn setup_done(&self) -> bool {
        read_json::<SetupFile>(&self.file("setup.json")).is_some_and(|f| f.done)
    }

    /// The sticks' calibrations, by controller.
    pub fn sticks(&self) -> clamshell_core::sticks::Calibrations {
        read_json(&self.file("sticks.json")).unwrap_or_default()
    }

    /// Whether clocks show 12 hours (else, or unsaved, 24).
    pub fn twelve_hour(&self) -> bool {
        read_json::<ClockFile>(&self.file("clock.json")).is_some_and(|c| c.hours == 12)
    }

    pub fn brightness(&self) -> Brightness {
        let Some(file) = read_json::<BrightnessFile>(&self.file("brightness.json")) else {
            return Brightness::default();
        };
        let mut b = Brightness { level: file.level.map(|l| l.min(100)), shares: Vec::new() };
        for (backlight, share) in file.shares {
            b.set_share(&backlight, share.min(100));
        }
        b
    }
}

/// A folder's JSON files that read as `T`, by name.
fn read_folder<T: DeserializeOwned>(dir: &Path) -> HashMap<String, T> {
    let names = fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok());
    names.filter(|n| n.ends_with(".json")).filter_map(|n| Some((n.clone(), read_json::<T>(&dir.join(&n))?))).collect()
}

/// A folder's files made `now` from `before` (as they were): those that changed written, those gone
/// removed.
fn write_folder<T: Serialize + PartialEq>(dir: &Path, before: &HashMap<String, T>, now: &HashMap<String, T>) -> io::Result<()> {
    for (name, value) in now {
        if before.get(name) != Some(value) {
            write_json(&dir.join(name), value)?;
        }
    }
    for name in before.keys().filter(|n| !now.contains_key(*n)) {
        fs::remove_file(dir.join(name))?;
    }
    Ok(())
}

/// A keymap's file name: its name (see [`Keymaps::unique_name`]); an entry's or a runner level's own as
/// its settings screen's ([`screen_file`]).
fn keymap_file(k: &Keymap) -> String {
    match &k.owner {
        Some(owner) => screen_file(owner),
        None => format!("{}.json", k.name),
    }
}

/// A settings screen's file name: `runner-<key>.json` for a runner's level, else `entry-<key>.json`.
fn screen_file(id: &str) -> String {
    match id.strip_prefix("runner:") {
        Some(runner) => format!("runner-{}.json", key_of(runner)),
        None => format!("entry-{}.json", key_of(id)),
    }
}

/// What was fetched about an entry, and a video's length: a file per entry in the art cache, beside its
/// images (`<key>-meta.json`, `<key>-probe.json`; the entry's id in the file). Each is written whole by
/// one thread (the metadata's, the folder browser's).
pub struct Cache {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct MetaFile {
    id: String,
    source: String,
    fetched: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shot: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    year: Option<u16>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    developer: String,
}

#[derive(Serialize, Deserialize)]
struct ProbeFile {
    id: String,
    #[serde(default)]
    seconds: Option<u32>,
    #[serde(default)]
    frame: Option<PathBuf>,
}

impl Cache {
    pub fn open(dir: &Path) -> Cache {
        Cache { dir: dir.to_path_buf() }
    }

    /// The files whose names end in `suffix`, read.
    fn read_all<T: DeserializeOwned>(&self, suffix: &str) -> Vec<(PathBuf, T)> {
        let entries = fs::read_dir(&self.dir).into_iter().flatten().flatten();
        entries.map(|e| e.path()).filter(|p| p.to_str().is_some_and(|p| p.ends_with(suffix))).filter_map(|p| Some((p.clone(), read_json(&p)?))).collect()
    }

    /// What was fetched about each entry: from where, when, and what.
    pub fn metas(&self) -> HashMap<String, Fetched> {
        self.read_all::<MetaFile>("-meta.json")
            .into_iter()
            .map(|(_, m)| {
                let meta = Meta { cover: m.cover, shot: m.shot, summary: m.summary, year: m.year, developer: m.developer, duration: None };
                (m.id, Fetched { source: m.source, at: m.fetched, meta })
            })
            .collect()
    }

    /// What was fetched about `id` (nothing: a miss), from `source`, at `at`.
    pub fn save_meta(&self, id: &str, source: &str, at: u64, meta: &Meta) -> io::Result<()> {
        let file = MetaFile {
            id: id.to_owned(),
            source: source.to_owned(),
            fetched: at,
            cover: meta.cover.clone(),
            shot: meta.shot.clone(),
            summary: meta.summary.clone(),
            year: meta.year,
            developer: meta.developer.clone(),
        };
        write_json(&self.dir.join(format!("{}-meta.json", key_of(id))), &file)
    }

    /// The videos' lengths (none: ffprobe couldn't tell) and frames, by id.
    pub fn probes(&self) -> HashMap<String, Probe> {
        self.read_all::<ProbeFile>("-probe.json").into_iter().map(|(_, p)| (p.id, Probe { seconds: p.seconds, frame: p.frame })).collect()
    }

    pub fn save_probe(&self, id: &str, seconds: Option<u32>, frame: Option<&Path>) -> io::Result<()> {
        write_json(&self.dir.join(format!("{}-probe.json", key_of(id))), &ProbeFile { id: id.to_owned(), seconds, frame: frame.map(Path::to_path_buf) })
    }

    /// The misses (fetched, nothing found) forgotten: fetched again. How many.
    pub fn forget_misses(&self) -> usize {
        let misses = self
            .read_all::<MetaFile>("-meta.json")
            .into_iter()
            .filter(|(_, m)| m.cover.is_none() && m.shot.is_none() && m.summary.is_empty() && m.year.is_none() && m.developer.is_empty());
        misses.filter(|(path, _)| fs::remove_file(path).is_ok()).count()
    }
}

/// A write for the [`Writer`].
pub enum Write {
    Save(Save),
    /// A play of entry `id`.
    Play {
        id: Arc<str>,
        start: u64,
        duration: u64,
    },
}

/// The state's writes, made in order on a thread of their own. Dropped, it finishes those sent first.
pub struct Writer {
    tx: Option<Sender<Write>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn spawn(mut store: Store) -> io::Result<Writer> {
        let (tx, rx) = mpsc::channel::<Write>();
        let thread = std::thread::Builder::new().name("store".into()).spawn(move || {
            for write in rx {
                let written = match &write {
                    Write::Save(save) => store.save(save),
                    Write::Play { id, start, duration } => store.add_play(id, *start, *duration),
                };
                if let Err(e) = written {
                    log::warn!("store: {e}");
                }
            }
        })?;
        Ok(Writer { tx: Some(tx), thread: Some(thread) })
    }

    pub fn send(&self, write: Write) {
        if self.tx.as_ref().is_none_or(|tx| tx.send(write).is_err()) {
            log::warn!("store: its thread is gone: not written");
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_core::keymap::{Binding, Button, Input, Output, PadOut};
    use clamshell_core::levels::Share;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clamshell-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_writer_writes_in_order_and_finishes_before_it_is_gone() {
        let dir = dir("writer");
        let writer = Writer::spawn(Store::open(&dir)).unwrap();
        writer.send(Write::Save(Save::Favourites(vec!["a".into()])));
        writer.send(Write::Save(Save::Favourites(vec!["b".into()])));
        writer.send(Write::Play { id: "snes:Zelda.sfc".into(), start: 100, duration: 30 });
        drop(writer);
        let store = Store::open(&dir);
        assert_eq!(store.favourites(), HashSet::from(["b".to_owned()]));
        assert_eq!(store.plays().get("snes:Zelda.sfc"), Some(&Plays { count: 1, seconds: 30, last: 130 }));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn everything_reads_back_as_saved() {
        let dir = dir("all");
        let mut store = Store::open(&dir);
        assert_eq!(store.resume(), None, "nothing saved yet");
        assert_eq!(store.keymaps(), None);

        store.add_play("snes:Zelda.sfc", 100, 30).unwrap();
        store.add_play("snes:Zelda.sfc", 500, 20).unwrap();
        assert_eq!(store.plays().get("snes:Zelda.sfc"), Some(&Plays { count: 2, seconds: 50, last: 520 }));

        store.save(&Save::Favourites(vec!["a".into(), "b".into()])).unwrap();
        store.save(&Save::Favourites(vec!["b".into()])).unwrap();
        assert_eq!(store.favourites(), HashSet::from(["b".to_owned()]));

        let mut keymaps = Keymaps::initial();
        let zelda: Arc<str> = "snes:Zelda.sfc".into();
        let fps = keymaps.add("FPS");
        let own = keymaps.make_own(1, &zelda);
        keymaps.get_mut(fps).unwrap().set(Input::Button(Button::A), Some(Binding::to(Output::Pad(PadOut::Button(Button::Y)))));
        store.save(&Save::Keymaps(keymaps.clone())).unwrap();
        let back = store.keymaps().unwrap();
        assert_eq!(back, keymaps);
        assert_eq!(back.own_of(&zelda).map(|k| k.id), Some(own));
        let mut files: Vec<String> = fs::read_dir(dir.join("keymaps")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        assert_eq!(files, ["FPS.json", "Nintendo layout.json", &format!("entry-{}.json", key_of(&zelda))]);
        keymaps.rename(fps, "Racing");
        store.save(&Save::Keymaps(keymaps.clone())).unwrap();
        assert!(dir.join("keymaps/Racing.json").exists() && !dir.join("keymaps/FPS.json").exists(), "renamed: its file too");
        assert_eq!(store.keymaps().unwrap(), keymaps);
        store.save(&Save::Keymaps(Keymaps::default())).unwrap();
        assert_eq!(store.keymaps(), Some(Keymaps::default()), "none left: none, not the initial ones");

        let rules = WindowRules::from_list(vec![
            WindowRule { owner: zelda.clone(), app_id: "emu".into(), title: Some("Bottom".into()) },
            WindowRule { owner: zelda.clone(), app_id: "emu2".into(), title: None },
        ]);
        store.save(&Save::WindowRules(rules.clone())).unwrap();
        assert_eq!(store.window_rules(), rules);

        store.save(&Save::Dock(vec!["b".into(), "a".into()])).unwrap();
        assert_eq!(store.dock().pinned_ids(), vec![Arc::from("b"), Arc::from("a")]);
        store.save(&Save::Autostart(vec!["a".into()])).unwrap();
        assert_eq!(store.autostart(), vec![Arc::from("a")]);

        let resume = Resume { collection: "TV".into(), tab: "dock".into(), places: vec![("TV".into(), vec!["tv:Show".into(), "tv:Show/1.mkv".into()])] };
        store.save(&Save::Resume(resume.clone())).unwrap();
        assert_eq!(store.resume(), Some(resume));

        let brightness = Brightness { level: Some(40), shares: vec![Share { backlight: "bottom".into(), share: 70 }] };
        store.save(&Save::Brightness(brightness.clone())).unwrap();
        assert_eq!(store.brightness(), brightness);

        assert!(!store.twelve_hour());
        store.save(&Save::TwelveHour(true)).unwrap();
        assert!(store.twelve_hour());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_are_a_file_per_screen_written_where_they_changed() {
        let dir = dir("settings");
        let mut store = Store::open(&dir);
        let zelda: Arc<str> = "snes:Zelda.sfc".into();
        let snes: Arc<str> = "runner:SNES".into();
        let mut settings = Settings::default();
        settings.set(None, Setting::FpsCap, Some(60));
        settings.set(Some(&zelda), Setting::Power, Some(2));
        settings.set(Some(&snes), Setting::Keymap, Some(3));
        settings.set_env(None, "A=1");
        settings.set_env(Some(zelda.clone()), "B=2");
        store.save(&Save::Settings(settings.clone())).unwrap();
        let mut names: Vec<String> = fs::read_dir(dir.join("settings")).unwrap().flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        assert_eq!(names, ["all.json".to_owned(), format!("entry-{}.json", key_of(&zelda)), format!("runner-{}.json", key_of("SNES"))]);
        assert_eq!(Store::open(&dir).settings(), settings);

        // Zelda's override and environment taken back: its file gone; All's left as it was.
        let all = dir.join("settings/all.json");
        let written = fs::metadata(&all).unwrap().modified().unwrap();
        settings.set(Some(&zelda), Setting::Power, None);
        settings.set_env(Some(zelda.clone()), "");
        store.save(&Save::Settings(settings.clone())).unwrap();
        assert!(!dir.join(format!("settings/entry-{}.json", key_of(&zelda))).exists());
        assert_eq!(fs::metadata(&all).unwrap().modified().unwrap(), written, "unchanged: not written");
        assert_eq!(Store::open(&dir).settings(), settings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_doesnt_read_back_is_skipped() {
        let dir = dir("skip");
        fs::create_dir_all(dir.join("settings")).unwrap();
        fs::write(dir.join("settings/all.json"), r#"{"settings": {"fps": 33, "nonsense": 1}}"#).unwrap();
        fs::write(dir.join("settings/entry-x.json"), r#"{"id": "x", "settings": {"power": 2}}"#).unwrap();
        fs::write(dir.join("settings/broken.json"), "{").unwrap();
        fs::write(dir.join("plays.jsonl"), "{\"id\": \"a\", \"start\": 1, \"duration\": 2}\n{\"id\": \"a\", \"sta").unwrap();
        let mut store = Store::open(&dir);
        let s = store.settings();
        assert_eq!(s.global.get(Setting::FpsCap), Setting::FpsCap.default_value());
        assert_eq!(s.of_in("x", &[]).get(Setting::Power), 2);
        assert_eq!(store.plays().get("a").map(|p| p.count), Some(1), "a line cut short is skipped");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_keeps_a_file_per_entry() {
        let dir = dir("cache");
        let cache = Cache::open(&dir);
        let meta = Meta { cover: Some("/c/x-cover.png".into()), year: Some(1994), summary: "S".into(), ..Meta::default() };
        cache.save_meta("x", "libretro:SNES", 5, &meta).unwrap();
        cache.save_meta("y", "steam:1", 6, &Meta::default()).unwrap();
        let metas = cache.metas();
        assert_eq!(metas.get("x"), Some(&Fetched { source: "libretro:SNES".into(), at: 5, meta }));
        assert_eq!(metas.get("y").map(|f| &f.meta), Some(&Meta::default()), "a miss");
        assert_eq!(cache.forget_misses(), 1);
        assert!(!cache.metas().contains_key("y") && cache.metas().contains_key("x"));
        cache.save_probe("ep", Some(2700), Some(Path::new("/c/ep-frame.jpg"))).unwrap();
        cache.save_probe("odd", None, None).unwrap();
        let probes = cache.probes();
        assert_eq!(probes.get("ep"), Some(&Probe { seconds: Some(2700), frame: Some("/c/ep-frame.jpg".into()) }));
        assert_eq!(probes.get("odd").map(|p| p.seconds), Some(None));
        assert!(dir.join(format!("{}-meta.json", key_of("x"))).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
