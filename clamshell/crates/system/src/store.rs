//! What ClamShell keeps, as files in its state dir (~/.local/share/clamshell). They are JSON that a person
//! can read and edit. No file both grows and is rewritten whole:
//!
//! - Each thing the state saves whole (see [`Save`]) has a file of its own. It is replaced by writing a
//!   new file and renaming it over the old one, so a write cut short by a crash or power loss leaves the
//!   last complete one.
//!   - `settings/<screen>.json` holds one settings screen's own settings and environment. The files are
//!     `all.json`, `runner-<key>.json` for a runner's level and `entry-<key>.json` for an entry. The key
//!     is the one the entry's units and art are named by (see `clamshell_library::key_of`), and the
//!     screen's id is inside the file. A screen with nothing set has no file.
//!   - `keymaps/<name>.json` holds one keymap, named after it. A keymap owned by an entry or a runner level
//!     is named like that owner's settings file. With no folder, the initial keymaps are used.
//!   - `window-rules.json`, `dock.json`, `autostart.json`, `favourites.json`, `resume.json` (where shelf
//!     and the second screen were), `sticks.json` (the sticks' calibrations, by controller),
//!     `brightness.json`, `clock.json`, and `first-start.json` (whether the first start is over, see
//!     `clamshell_core::device`).
//! - `plays.jsonl` has one line per play: the entry's id, the start in seconds since the epoch and the
//!   duration in seconds. Lines are appended, and a line cut short is skipped.
//!
//! What was fetched about an entry, and a video's length, are kept in the art cache, in a file per entry
//! beside its images (`crate::library::meta::cache`).
//!
//! ClamShell's own writes go through a thread of their own ([`Writer`]) so the disk doesn't hold up the
//! screens. A file that doesn't make sense when read back is logged and left out.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use clamshell_core::bottom::screen::{WindowRule, WindowRules};
use clamshell_core::keymap::{Keymap, Keymaps};
use clamshell_core::levels::Brightness;
use clamshell_core::settings::{Setting, Settings};
use clamshell_core::{Dock, Resume, Save};
use clamshell_library::{Plays, key_of};
use serde::de::DeserializeOwned;

use crate::os::fs::{read_json, write_json};
use serde::{Deserialize, Serialize};

/// A settings screen's file.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Screen {
    /// The screen's id: an entry's id, or `runner:<name>` for a runner's level. All has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    /// The settings by name. For All, only those that aren't at their default.
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
    /// Each collection's place: the ids of its open groups, then the id of its selected entry.
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
struct FirstStartFile {
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
    /// The settings screens' files as last written, by name, so only those that changed are written.
    screens: Option<HashMap<String, Screen>>,
    /// The keymaps' files as last written, for the same reason.
    keymaps: Option<HashMap<String, Keymap>>,
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        Store { dir: dir.to_path_buf(), screens: None, keymaps: None }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Saves something the state keeps, replacing what was saved before.
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
            Save::FirstStartDone => write_json(&self.file("first-start.json"), &FirstStartFile { done: true }),
            Save::Sticks(c) => write_json(&self.file("sticks.json"), c),
        }
    }

    /// Every entry's plays, summed up.
    pub fn plays(&self) -> HashMap<String, Plays> {
        let mut out: HashMap<String, Plays> = HashMap::new();
        let path = self.file("plays.jsonl");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    log::debug!("{}: {e}", path.display());
                } else {
                    log::warn!("{}: {e}", path.display());
                }
                return out;
            }
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Play>(line) {
                Ok(p) => out.entry(p.id).or_default().add(p.start, p.duration),
                Err(e) => log::warn!("plays.jsonl: {e}: {line}"),
            }
        }
        out
    }

    /// Adds a play of `id`. The line is appended whole in one write and flushed to the disk.
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

    /// The settings and environment. Unknown settings, and values a setting doesn't take, are skipped.
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

    /// The settings screens' files on disk, by name.
    fn screens_on_disk(&mut self) -> &HashMap<String, Screen> {
        let dir = self.file("settings");
        self.screens.get_or_insert_with(|| read_folder(&dir))
    }

    /// Saves the settings as one file per screen, writing only the files that changed. Files of screens
    /// that no longer have anything set are removed.
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

    /// The keymaps, by id, or none if they were never saved. A keymap that doesn't read back is logged
    /// and skipped.
    pub fn keymaps(&self) -> Option<Keymaps> {
        let dir = self.file("keymaps");
        if !dir.is_dir() {
            return None;
        }
        let mut list: Vec<Keymap> = read_folder(&dir).into_values().collect();
        list.sort_by_key(|k| k.id);
        Some(Keymaps::from_list(list))
    }

    /// Saves each keymap as a file, writing only the files that changed. Files of removed keymaps are
    /// deleted.
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

    /// Where shelf and the second screen were, or none if that was never saved.
    pub fn resume(&self) -> Option<Resume> {
        let r: ResumeFile = read_json(&self.file("resume.json"))?;
        Some(Resume { collection: r.collection, tab: r.tab, places: r.places })
    }

    /// Whether the first start is over. If not, the Device view opens at the start.
    pub fn first_start_done(&self) -> bool {
        read_json::<FirstStartFile>(&self.file("first-start.json")).is_some_and(|f| f.done)
    }

    /// The sticks' calibrations, by controller.
    pub fn sticks(&self) -> clamshell_core::device::sticks::Calibrations {
        read_json(&self.file("sticks.json")).unwrap_or_default()
    }

    /// Whether clocks show 12 hours. Otherwise, or if never saved, they show 24.
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

/// The JSON files in a folder that read as `T`, by name.
fn read_folder<T: DeserializeOwned>(dir: &Path) -> HashMap<String, T> {
    let entries = fs::read_dir(dir)
        .inspect_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                log::debug!("{}: {e}", dir.display());
            } else {
                log::warn!("{}: {e}", dir.display());
            }
        })
        .into_iter()
        .flatten();
    let names = entries.filter_map(|e| e.inspect_err(|e| log::warn!("{}: {e}", dir.display())).ok()).filter_map(|e| e.file_name().into_string().ok());
    names.filter(|n| n.ends_with(".json")).filter_map(|n| Some((n.clone(), read_json::<T>(&dir.join(&n))?))).collect()
}

/// Changes a folder's files from `before` to `now`. Files that changed are written and files no longer in
/// `now` are removed.
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

/// A keymap's file name. It is the keymap's name (see [`Keymaps::unique_name`]), except that a keymap
/// owned by an entry or a runner level is named like the owner's settings file ([`screen_file`]).
fn keymap_file(k: &Keymap) -> String {
    match &k.owner {
        Some(owner) => screen_file(owner),
        None => format!("{}.json", k.name),
    }
}

/// A settings screen's file name: `runner-<key>.json` for a runner's level, else `entry-<key>.json`.
fn screen_file(id: &str) -> String {
    match id.strip_prefix(clamshell_library::ids::RUNNER) {
        Some(runner) => format!("runner-{}.json", key_of(runner)),
        None => format!("entry-{}.json", key_of(id)),
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

/// Makes the state's writes in order on a thread of their own. When dropped, it finishes the writes
/// already sent. Its thread is joined, which [`crate::os::worker::Worker`] doesn't do, so the last writes
/// are made before the program exits.
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
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("store: its thread panicked");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_core::keymap::{Binding, Button, Input, Output, PadOut};
    use clamshell_core::levels::Share;

    fn dir(name: &str) -> clamshell_core::kit::testdir::TempDir {
        clamshell_core::kit::testdir::TempDir::new(&format!("store-{name}"))
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
        let fps = keymaps.add("FPS").unwrap();
        let own = keymaps.make_own(1, &zelda).unwrap();
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

        // With Zelda's override and environment cleared, its file is gone and All's file is untouched.
        let all = dir.join("settings/all.json");
        let written = fs::metadata(&all).unwrap().modified().unwrap();
        settings.set(Some(&zelda), Setting::Power, None);
        settings.set_env(Some(zelda.clone()), "");
        store.save(&Save::Settings(settings.clone())).unwrap();
        assert!(!dir.join(format!("settings/entry-{}.json", key_of(&zelda))).exists());
        assert_eq!(fs::metadata(&all).unwrap().modified().unwrap(), written, "unchanged: not written");
        assert_eq!(Store::open(&dir).settings(), settings);
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
    }
}
