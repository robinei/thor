//! ClamShell's database, `clamshell.db` in the state dir (SQLite, write-ahead logged, so a write cut
//! short by a crash or power loss leaves the last complete one): play time, favourites, the entries'
//! settings and environment, the keymaps, the window rules, the dock's pins, where shelf and the second
//! screen were, and the screens' brightness. Each is saved whole, replacing what was there, in one
//! transaction. Play time is added a row per play, by the launcher's thread on a connection of its own.
//!
//! Tables:
//!
//! - `plays(id, start, duration)`: an entry's plays (start in seconds since the epoch, duration in
//!   seconds).
//! - `favourites(id)`.
//! - `settings(entry, name, value)`, `env(entry, text)`: the global ones under the entry `''`.
//! - `keymaps(position, keymap)`: a keymap as JSON (clamshell-core's `Keymap`, through serde).
//! - `window_rules(position, entry, app_id, title)`.
//! - `dock(position, id)`: the pinned entries.
//! - `places(collection, path)`: shelf's place in each collection, a JSON list of ids (the open groups,
//!   then the selected entry).
//! - `shares(backlight, share)`: each screen's share of the brightness.
//! - `kv(key, value)`: `collection` and `tab` (where shelf and the second screen were), `brightness`
//!   (the level), `keymaps` (present once the keymaps were saved: without it, the initial ones).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use clamshell_core::bottom_screen::{WindowRule, WindowRules};
use clamshell_core::keymap::{Keymap, Keymaps};
use clamshell_core::levels::Brightness;
use clamshell_core::settings::{Setting, Settings};
use clamshell_core::{Dock, Resume};
use clamshell_library::Plays;
use rusqlite::{Connection, OptionalExtension, params};

pub use rusqlite::Error;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS plays (id TEXT NOT NULL, start INTEGER NOT NULL, duration INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS plays_id ON plays (id);
CREATE TABLE IF NOT EXISTS favourites (id TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS settings (entry TEXT NOT NULL, name TEXT NOT NULL, value INTEGER NOT NULL, PRIMARY KEY (entry, name));
CREATE TABLE IF NOT EXISTS env (entry TEXT PRIMARY KEY, text TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS keymaps (position INTEGER PRIMARY KEY, keymap TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS window_rules (position INTEGER PRIMARY KEY, entry TEXT NOT NULL, app_id TEXT NOT NULL, title TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS dock (position INTEGER PRIMARY KEY, id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS places (collection TEXT PRIMARY KEY, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS shares (backlight TEXT PRIMARY KEY, share INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// The entry the global settings and environment are kept under.
const GLOBAL: &str = "";

/// A connection to the database.
pub struct Store {
    db: Connection,
}

impl Store {
    /// Opens `file` (its folder and tables made if missing).
    pub fn open(file: &Path) -> Result<Store, Error> {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = Connection::open(file)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.execute_batch(SCHEMA)?;
        Ok(Store { db })
    }

    /// Every entry's plays, summed up.
    pub fn plays(&self) -> Result<HashMap<String, Plays>, Error> {
        let mut out: HashMap<String, Plays> = HashMap::new();
        let mut stmt = self.db.prepare("SELECT id, start, duration FROM plays")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (id, start, duration) = row?;
            out.entry(id).or_default().add(u64::try_from(start).unwrap_or(0), u64::try_from(duration).unwrap_or(0));
        }
        Ok(out)
    }

    /// Adds a play of `id`.
    pub fn add_play(&self, id: &str, start: u64, duration: u64) -> Result<(), Error> {
        let n = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        self.db.execute("INSERT INTO plays (id, start, duration) VALUES (?1, ?2, ?3)", params![id, n(start), n(duration)])?;
        Ok(())
    }

    pub fn favourites(&self) -> Result<HashSet<String>, Error> {
        let mut stmt = self.db.prepare("SELECT id FROM favourites")?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }

    pub fn save_favourites(&mut self, ids: &[String]) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM favourites", [])?;
        for id in ids {
            tx.execute("INSERT OR IGNORE INTO favourites (id) VALUES (?1)", [id])?;
        }
        tx.commit()
    }

    /// The settings and environment (settings it doesn't know, or values they don't take, skipped).
    pub fn settings(&self) -> Result<Settings, Error> {
        let mut s = Settings::default();
        let mut stmt = self.db.prepare("SELECT entry, name, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (entry, name, value) = row?;
            let (Some(setting), Ok(value)) = (Setting::from_name(&name), u16::try_from(value)) else {
                continue;
            };
            if !setting.accepts(value) {
                continue;
            }
            let entry: Option<Arc<str>> = (entry != GLOBAL).then(|| entry.into());
            s.set(entry.as_ref(), setting, Some(value));
        }
        let mut stmt = self.db.prepare("SELECT entry, text FROM env")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (entry, text) = row?;
            s.set_env((entry != GLOBAL).then(|| entry.into()), &text);
        }
        Ok(s)
    }

    pub fn save_settings(&mut self, s: &Settings) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM settings", [])?;
        tx.execute("DELETE FROM env", [])?;
        {
            let mut setting = tx.prepare("INSERT INTO settings (entry, name, value) VALUES (?1, ?2, ?3)")?;
            for &k in &Setting::ALL {
                setting.execute(params![GLOBAL, k.name(), s.global.get(k)])?;
            }
            for (entry, o) in &s.overrides {
                for &k in &Setting::ALL {
                    if let Some(v) = o.get(k) {
                        setting.execute(params![&**entry, k.name(), v])?;
                    }
                }
            }
            let mut env = tx.prepare("INSERT INTO env (entry, text) VALUES (?1, ?2)")?;
            if !s.global_env.is_empty() {
                env.execute(params![GLOBAL, s.global_env])?;
            }
            for (entry, text) in &s.env {
                env.execute(params![&**entry, text])?;
            }
        }
        tx.commit()
    }

    /// The keymaps, none if they were never saved (a keymap that doesn't read back is skipped, logged).
    pub fn keymaps(&self) -> Result<Option<Keymaps>, Error> {
        if self.get("keymaps")?.is_none() {
            return Ok(None);
        }
        let mut stmt = self.db.prepare("SELECT keymap FROM keymaps ORDER BY position")?;
        let rows: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let list = rows
            .iter()
            .filter_map(|json| serde_json::from_str::<Keymap>(json).map_err(|e| log::warn!("store: a keymap doesn't read back: {e}")).ok())
            .collect();
        Ok(Some(Keymaps::from_list(list)))
    }

    pub fn save_keymaps(&mut self, keymaps: &Keymaps) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM keymaps", [])?;
        for (i, k) in (0i64..).zip(keymaps.list()) {
            let json = serde_json::to_string(k).map_err(|e| Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("INSERT INTO keymaps (position, keymap) VALUES (?1, ?2)", params![i, json])?;
        }
        tx.execute("INSERT OR REPLACE INTO kv (key, value) VALUES ('keymaps', '1')", [])?;
        tx.commit()
    }

    pub fn window_rules(&self) -> Result<WindowRules, Error> {
        let mut stmt = self.db.prepare("SELECT entry, app_id, title FROM window_rules ORDER BY position")?;
        let rules = stmt.query_map([], |r| Ok(WindowRule { owner: r.get::<_, String>(0)?.into(), app_id: r.get(1)?, title: r.get(2)? }))?;
        Ok(WindowRules::from_list(rules.collect::<Result<_, _>>()?))
    }

    pub fn save_window_rules(&mut self, rules: &WindowRules) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM window_rules", [])?;
        for (i, r) in (0i64..).zip(rules.list()) {
            tx.execute("INSERT INTO window_rules (position, entry, app_id, title) VALUES (?1, ?2, ?3, ?4)", params![i, &*r.owner, r.app_id, r.title])?;
        }
        tx.commit()
    }

    /// The dock's pinned entries, in order.
    pub fn dock(&self) -> Result<Dock, Error> {
        let mut stmt = self.db.prepare("SELECT id FROM dock ORDER BY position")?;
        let ids = stmt.query_map([], |r| Ok(Arc::from(r.get::<_, String>(0)?)))?;
        Ok(Dock::pinned(ids.collect::<Result<_, _>>()?))
    }

    pub fn save_dock(&mut self, ids: &[Arc<str>]) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM dock", [])?;
        for (i, id) in (0i64..).zip(ids) {
            tx.execute("INSERT INTO dock (position, id) VALUES (?1, ?2)", params![i, &**id])?;
        }
        tx.commit()
    }

    /// Where shelf and the second screen were; none if that was never saved.
    pub fn resume(&self) -> Result<Option<Resume>, Error> {
        let (Some(collection), Some(tab)) = (self.get("collection")?, self.get("tab")?) else {
            return Ok(None);
        };
        let mut stmt = self.db.prepare("SELECT collection, path FROM places ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut places = Vec::new();
        for row in rows {
            let (name, path) = row?;
            if let Ok(ids) = serde_json::from_str::<Vec<String>>(&path) {
                places.push((name, ids));
            }
        }
        Ok(Some(Resume { collection, tab, places }))
    }

    pub fn save_resume(&mut self, r: &Resume) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        tx.execute("INSERT OR REPLACE INTO kv (key, value) VALUES ('collection', ?1), ('tab', ?2)", params![r.collection, r.tab])?;
        tx.execute("DELETE FROM places", [])?;
        for (name, ids) in &r.places {
            let path = serde_json::to_string(ids).map_err(|e| Error::ToSqlConversionFailure(Box::new(e)))?;
            tx.execute("INSERT OR REPLACE INTO places (collection, path) VALUES (?1, ?2)", params![name, path])?;
        }
        tx.commit()
    }

    pub fn brightness(&self) -> Result<Brightness, Error> {
        let mut b = Brightness { level: self.get("brightness")?.and_then(|l| l.parse::<u8>().ok()).map(|l| l.min(100)), shares: Vec::new() };
        let mut stmt = self.db.prepare("SELECT backlight, share FROM shares")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (backlight, share) = row?;
            b.set_share(&backlight, u8::try_from(share.clamp(0, 100)).unwrap_or(100));
        }
        Ok(b)
    }

    pub fn save_brightness(&mut self, b: &Brightness) -> Result<(), Error> {
        let tx = self.db.transaction()?;
        match b.level {
            Some(level) => tx.execute("INSERT OR REPLACE INTO kv (key, value) VALUES ('brightness', ?1)", [level.to_string()])?,
            None => tx.execute("DELETE FROM kv WHERE key = 'brightness'", [])?,
        };
        tx.execute("DELETE FROM shares", [])?;
        for s in &b.shares {
            tx.execute("INSERT INTO shares (backlight, share) VALUES (?1, ?2)", params![s.backlight, s.share])?;
        }
        tx.commit()
    }

    fn get(&self, key: &str) -> Result<Option<String>, Error> {
        self.db.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_core::keymap::{Binding, Button, Input, Output, PadOut};
    use clamshell_core::levels::Share;

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("clamshell-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Store::open(&dir.join("clamshell.db")).unwrap(), dir)
    }

    #[test]
    fn everything_reads_back_as_saved() {
        let (mut db, dir) = store("all");
        assert_eq!(db.resume().unwrap(), None, "nothing saved yet");
        assert_eq!(db.keymaps().unwrap(), None);

        db.add_play("snes:Zelda.sfc", 100, 30).unwrap();
        db.add_play("snes:Zelda.sfc", 500, 20).unwrap();
        assert_eq!(db.plays().unwrap().get("snes:Zelda.sfc"), Some(&Plays { count: 2, seconds: 50, last: 520 }));

        db.save_favourites(&["a".into(), "b".into()]).unwrap();
        db.save_favourites(&["b".into()]).unwrap();
        assert_eq!(db.favourites().unwrap(), HashSet::from(["b".to_owned()]));

        let mut settings = Settings::default();
        let zelda: Arc<str> = "snes:Zelda.sfc".into();
        settings.set(None, Setting::FpsCap, Some(60));
        settings.set(Some(&zelda), Setting::Power, Some(2));
        settings.set_env(None, "A=1");
        settings.set_env(Some(zelda.clone()), "B=2");
        db.save_settings(&settings).unwrap();
        assert_eq!(db.settings().unwrap(), settings);

        let mut keymaps = Keymaps::initial();
        let fps = keymaps.add("FPS");
        let own = keymaps.make_own(1, &zelda);
        keymaps.get_mut(fps).unwrap().set(Input::Button(Button::A), Some(Binding::to(Output::Pad(PadOut::Button(Button::Y)))));
        db.save_keymaps(&keymaps).unwrap();
        let back = db.keymaps().unwrap().unwrap();
        assert_eq!(back, keymaps);
        assert_eq!(back.own_of(&zelda).map(|k| k.id), Some(own));
        db.save_keymaps(&Keymaps::default()).unwrap();
        assert_eq!(db.keymaps().unwrap(), Some(Keymaps::default()), "none left: none, not the initial ones");

        let rules = WindowRules::from_list(vec![WindowRule { owner: zelda.clone(), app_id: "emu".into(), title: "Bottom".into() }]);
        db.save_window_rules(&rules).unwrap();
        assert_eq!(db.window_rules().unwrap(), rules);

        db.save_dock(&["b".into(), "a".into()]).unwrap();
        assert_eq!(db.dock().unwrap().pinned_ids(), vec![Arc::from("b"), Arc::from("a")]);

        let resume = Resume { collection: "TV".into(), tab: "dock".into(), places: vec![("TV".into(), vec!["tv:Show".into(), "tv:Show/1.mkv".into()])] };
        db.save_resume(&resume).unwrap();
        assert_eq!(db.resume().unwrap(), Some(resume));

        let brightness = Brightness { level: Some(40), shares: vec![Share { backlight: "bottom".into(), share: 70 }] };
        db.save_brightness(&brightness).unwrap();
        assert_eq!(db.brightness().unwrap(), brightness);

        // Another connection (the launcher's) sees the same.
        let other = Store::open(&dir.join("clamshell.db")).unwrap();
        assert_eq!(other.dock().unwrap().pinned_ids().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_it_doesnt_take_are_skipped() {
        let (db, dir) = store("skip");
        db.db.execute_batch("INSERT INTO settings VALUES ('', 'fps', 33), ('', 'nonsense', 1), ('x', 'power', 2)").unwrap();
        let s = db.settings().unwrap();
        assert_eq!(s.global.get(Setting::FpsCap), Setting::FpsCap.default_value());
        assert_eq!(s.of("x").get(Setting::Power), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
