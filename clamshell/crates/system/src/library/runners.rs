//! Runners: how things open (see docs/runners.md). library.json's `runners`, then ClamShell's own
//! (those whose program is installed): one per RetroArch system with its core installed, Video and Music
//! (mpv), Images (imv), PDF (zathura), Text (`less` in foot), and last Other files (`xdg-open`, any name).
//! A file opens with the first runner one of whose patterns its name matches; a runner's `scan`s put
//! what they find in the library, opened by it.
//!
//! A runner's `id` keeps its settings, environment and own keymap (its `name` is only shown, and may
//! change); without one, its name in lowercase, spaces as `-`. A runner in library.json with the id of one
//! of ClamShell's takes what it leaves out from it (its name, patterns, command, metadata, keymap), and
//! takes its place: `{ "id": "snes", "scan": [{ "id": "snes", "dir": "~/Games/snes", "tab": "Games",
//! "group": "SNES" }] }` scans with ClamShell's SNES.

use std::path::Path;

use clamshell_library::pattern;
use serde::Deserialize;

use super::content::Source;

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
    /// The file names it opens (one name each: `*.sfc`; any case).
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,
    /// The command, `{path}` the file.
    #[serde(default)]
    pub launch: Vec<String>,
    /// Where its entries' covers and details come from (`libretro:<system>`, `tmdb:movie`, `tmdb:tv`), for
    /// a scan without its own.
    #[serde(default)]
    pub metadata: Option<String>,
    /// Dirs whose entries go in the library.
    #[serde(default)]
    pub scan: Vec<Scan>,
    /// A keymap of ClamShell's it comes with (`viewer`: the D-pad, A, B and the shoulders as keys), its own
    /// until one is chosen for it.
    #[serde(default)]
    pub keymap: Option<String>,
}

/// A dir a runner's entries are found in (see [`super::content`] for how): `id` names them (`<id>:<path
/// in dir>`), `tab` and `group` say where they show; `match` (paths in `dir`, a part per folder level:
/// `*/*.cue`) and `items` (a matched folder's own entries) say what is an entry (else the runner's
/// patterns).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    pub id: String,
    pub dir: String,
    pub tab: String,
    #[serde(default)]
    pub group: Option<String>,
    /// The tab's icon.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub metadata: Option<String>,
}

impl Runner {
    /// Its scans, each with what it takes from the runner.
    pub fn sources(&self) -> Vec<Source> {
        self.scan
            .iter()
            .map(|s| Source {
                runner: self.id.clone(),
                runner_name: self.name.clone(),
                tab: s.tab.clone(),
                group: s.group.clone(),
                icon: s.icon.clone(),
                id: s.id.clone(),
                dir: s.dir.clone(),
                patterns: if s.patterns.is_empty() { self.patterns.clone() } else { s.patterns.clone() },
                items: s.items.clone(),
                launch: self.launch.clone(),
                metadata: s.metadata.clone().or_else(|| self.metadata.clone()),
            })
            .collect()
    }

    /// The runner that opens an entry of it, as the entry keeps it.
    pub fn of(&self) -> clamshell_library::RunnerOf {
        clamshell_library::RunnerOf::new(&self.id, &self.name)
    }

    /// Whether it opens a file named `name`.
    pub fn opens(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.patterns.iter().any(|p| pattern::matches(&p.to_lowercase(), &name))
    }
}

/// Where RetroArch's cores are.
const CORES: &str = "/usr/lib/libretro";

/// A RetroArch system: its runner's id and name, its core, its patterns, libretro's database of it.
struct System {
    id: &'static str,
    name: &'static str,
    core: &'static str,
    patterns: &'static [&'static str],
    database: &'static str,
}

/// RetroArch's systems.
const SYSTEMS: [System; 9] = [
    System { id: "nes", name: "NES", core: "nestopia", patterns: &["*.nes"], database: "Nintendo - Nintendo Entertainment System" },
    System { id: "snes", name: "SNES", core: "snes9x", patterns: &["*.sfc", "*.smc"], database: "Nintendo - Super Nintendo Entertainment System" },
    System { id: "n64", name: "Nintendo 64", core: "mupen64plus_next", patterns: &["*.z64", "*.n64", "*.v64"], database: "Nintendo - Nintendo 64" },
    System { id: "gb", name: "Game Boy", core: "gambatte", patterns: &["*.gb", "*.gbc"], database: "Nintendo - Game Boy Color" },
    System { id: "gba", name: "Game Boy Advance", core: "mgba", patterns: &["*.gba"], database: "Nintendo - Game Boy Advance" },
    System { id: "nds", name: "Nintendo DS", core: "melonds", patterns: &["*.nds"], database: "Nintendo - Nintendo DS" },
    System { id: "md", name: "Mega Drive", core: "genesis_plus_gx", patterns: &["*.md", "*.gen", "*.smd"], database: "Sega - Mega Drive - Genesis" },
    System { id: "psx", name: "PlayStation", core: "pcsx_rearmed", patterns: &["*.cue", "*.chd", "*.m3u", "*.pbp"], database: "Sony - PlayStation" },
    System { id: "dc", name: "Dreamcast", core: "flycast", patterns: &["*.gdi", "*.cdi"], database: "Sega - Dreamcast" },
];

/// One of ClamShell's other runners.
struct Builtin {
    id: &'static str,
    name: &'static str,
    launch: &'static [&'static str],
    patterns: &'static [&'static str],
    keymap: Option<&'static str>,
}

/// ClamShell's other runners, in order.
const OTHERS: [Builtin; 6] = [
    Builtin {
        id: "video",
        name: "Video",
        launch: &["mpv", "{path}"],
        patterns: &["*.mkv", "*.mp4", "*.m4v", "*.avi", "*.webm", "*.mov", "*.mpg", "*.mpeg", "*.ts", "*.wmv", "*.flv"],
        keymap: None,
    },
    Builtin {
        id: "music",
        name: "Music",
        launch: &["mpv", "--force-window=immediate", "{path}"],
        patterns: &["*.mp3", "*.flac", "*.ogg", "*.opus", "*.m4a", "*.wav", "*.aac"],
        keymap: None,
    },
    Builtin {
        id: "images",
        name: "Images",
        launch: &["imv", "{path}"],
        patterns: &["*.jpg", "*.jpeg", "*.png", "*.webp", "*.gif", "*.bmp", "*.tif", "*.tiff", "*.svg"],
        keymap: Some("viewer"),
    },
    Builtin { id: "pdf", name: "PDF", launch: &["zathura", "{path}"], patterns: &["*.pdf", "*.epub", "*.djvu"], keymap: Some("viewer") },
    Builtin {
        id: "text",
        name: "Text",
        launch: &["foot", "less", "{path}"],
        patterns: &[
            "*.txt", "*.md", "*.log", "*.nfo", "*.json", "*.toml", "*.yaml", "*.yml", "*.ini", "*.conf", "*.cfg", "*.csv", "*.xml", "*.sh", "*.py", "*.rs",
            "*.c", "*.h",
        ],
        keymap: Some("viewer"),
    },
    Builtin { id: "other", name: "Other files", launch: &["xdg-open", "{path}"], patterns: &["*"], keymap: None },
];

/// ClamShell's own runners, those whose program (and core) is installed, in order (Other files last).
/// `installed`: whether a program is on the PATH; `core`: whether a core's file is there.
pub fn builtin(installed: &dyn Fn(&str) -> bool, core: &dyn Fn(&Path) -> bool) -> Vec<Runner> {
    let strings = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let mut runners = Vec::new();
    if installed("retroarch") {
        for sys in SYSTEMS {
            let file = Path::new(CORES).join(format!("{}_libretro.so", sys.core));
            if core(&file) {
                let launch = vec!["retroarch".into(), "-f".into(), "-L".into(), file.to_string_lossy().into_owned(), "{path}".into()];
                runners.push(Runner {
                    id: sys.id.into(),
                    name: sys.name.into(),
                    patterns: strings(sys.patterns),
                    launch,
                    metadata: Some(format!("libretro:{}", sys.database)),
                    ..Runner::default()
                });
            }
        }
    }
    for b in OTHERS {
        if b.launch.first().is_some_and(|p| installed(p)) {
            let keymap = b.keymap.map(str::to_owned);
            let (id, name) = (b.id.into(), b.name.into());
            runners.push(Runner { id, name, patterns: strings(b.patterns), launch: strings(b.launch), keymap, ..Runner::default() });
        }
    }
    runners
}

/// Whether `program` is on the PATH.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// The runners in force: library.json's (each completed by ClamShell's of its name), then ClamShell's
/// own not named there; what is wrong with library.json's (a runner without a command).
pub fn all(config: &[Runner], builtin: Vec<Runner>) -> (Vec<Runner>, Vec<String>) {
    let mut problems = Vec::new();
    // Each with its id (else its name's), and its name (else ClamShell's of its id, else its id).
    let config: Vec<Runner> = config
        .iter()
        .map(|p| {
            let id = if p.id.is_empty() { p.name.to_lowercase().replace(' ', "-") } else { p.id.clone() };
            Runner { id, ..p.clone() }
        })
        .collect();
    let mut runners: Vec<Runner> = config
        .iter()
        .map(|p| {
            let b = builtin.iter().find(|b| b.id == p.id);
            let or = |mine: &Vec<String>, theirs: fn(&Runner) -> &Vec<String>| match b {
                Some(b) if mine.is_empty() => theirs(b).clone(),
                _ => mine.clone(),
            };
            Runner {
                id: p.id.clone(),
                name: [p.name.as_str(), b.map_or("", |b| b.name.as_str()), p.id.as_str()].into_iter().find(|n| !n.is_empty()).unwrap_or_default().to_owned(),
                patterns: or(&p.patterns, |b| &b.patterns),
                launch: or(&p.launch, |b| &b.launch),
                metadata: p.metadata.clone().or_else(|| b.and_then(|b| b.metadata.clone())),
                scan: p.scan.clone(),
                keymap: p.keymap.clone().or_else(|| b.and_then(|b| b.keymap.clone())),
            }
        })
        .collect();
    for p in runners.iter().filter(|p| p.id.is_empty()) {
        problems.push(format!("a runner without an id or a name: {}", p.launch.join(" ")));
    }
    let known = |id: &str| SYSTEMS.iter().any(|s| s.id == id) || OTHERS.iter().any(|b| b.id == id);
    for p in runners.iter().filter(|p| !p.id.is_empty() && p.launch.is_empty()) {
        match known(&p.id) {
            true => problems.push(format!("{}: ClamShell's runner {} isn't here (its program or core isn't installed)", p.name, p.id)),
            false => problems.push(format!("{}: no launch command (and ClamShell has no runner {})", p.name, p.id)),
        }
    }
    runners.retain(|p| !p.id.is_empty() && !p.launch.is_empty());
    runners.extend(builtin.into_iter().filter(|b| !config.iter().any(|p| p.id == b.id)));
    (runners, problems)
}

/// The runners in force here (ClamShell's own as installed).
pub fn in_force(config: &[Runner]) -> (Vec<Runner>, Vec<String>) {
    all(config, builtin(&on_path, &|f| f.is_file()))
}

/// The runner that opens a file named `name`: the first that does.
pub fn opener<'a>(runners: &'a [Runner], name: &str) -> Option<&'a Runner> {
    runners.iter().find(|p| p.opens(name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn names(runners: &[Runner]) -> Vec<&str> {
        runners.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn clamshells_runners_are_those_installed() {
        let all_there = builtin(&|_| true, &|_| true);
        assert_eq!(names(&all_there).first(), Some(&"NES"));
        assert_eq!(names(&all_there).last(), Some(&"Other files"));
        let some = builtin(&|p| p == "retroarch" || p == "mpv", &|f| f.to_string_lossy().contains("snes9x"));
        assert_eq!(names(&some), ["SNES", "Video", "Music"]);
        assert_eq!(some[0].metadata.as_deref(), Some("libretro:Nintendo - Super Nintendo Entertainment System"));
    }

    #[test]
    fn library_jsons_come_first_and_complete_clamshells() {
        let config: Vec<Runner> = serde_json::from_str(
            r#"[{ "id": "snes", "scan": [{ "id": "snes", "dir": "~/snes", "tab": "Games", "group": "SNES", "match": ["*.zip", "*.sfc"] }] },
                { "id": "text", "name": "Notes", "match": ["*.txt"], "launch": ["kate", "{path}"] },
                { "name": "Broken Thing" }]"#,
        )
        .unwrap();
        let (runners, problems) = all(&config, builtin(&|_| true, &|_| true));
        assert_eq!(names(&runners)[..2], ["SNES", "Notes"], "SNES's name ClamShell's; Text renamed");
        assert_eq!(problems, ["Broken Thing: no launch command (and ClamShell has no runner broken-thing)"]);
        assert_eq!(runners.iter().filter(|p| p.id == "text").count(), 1, "library.json's instead of ClamShell's");
        let snes = &runners[0];
        assert_eq!(snes.launch[0], "retroarch", "ClamShell's command");
        let sources = snes.sources();
        assert_eq!(sources[0].patterns, ["*.zip", "*.sfc"], "the scan's own patterns");
        assert_eq!(sources[0].metadata.as_deref(), Some("libretro:Nintendo - Super Nintendo Entertainment System"));
        assert_eq!(opener(&runners, "Zelda.SFC").map(|p| p.name.as_str()), Some("SNES"));
        assert_eq!(opener(&runners, "game.zip").map(|p| p.name.as_str()), Some("Other files"), "a scan's patterns open nothing");
        assert_eq!(opener(&runners, "notes.txt").map(|p| p.launch[0].as_str()), Some("kate"));
        assert_eq!(opener(&runners, "cover.png").map(|p| p.name.as_str()), Some("Images"));
    }
}
