//! Runners: how things open (see docs/runners.md). library.json's `runners`, then ClamShell's own
//! (those whose program is installed): one per RetroArch system with its core installed, Video and Music
//! (mpv), Images (imv), PDF (zathura), Text (`less` in foot), and last Other files (`xdg-open`, any name).
//! A file opens with the first runner one of whose patterns its name matches; a runner's `scan`s put
//! what they find in the library, opened by it.
//!
//! A runner in library.json named as one of ClamShell's takes what it leaves out from it (its patterns,
//! command, metadata), and takes its place: `{ "name": "SNES", "scan": [{ "id": "snes", "dir": "~/Games/snes",
//! "tab": "Games", "group": "SNES" }] }` scans with ClamShell's SNES.

use std::path::Path;

use clamshell_library::pattern;
use serde::Deserialize;

use super::content::Source;

/// A runner, as library.json gives it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    /// Its settings page's title; its settings are kept by it.
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
                runner: self.name.clone(),
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

    /// Whether it opens a file named `name`.
    pub fn opens(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.patterns.iter().any(|p| pattern::matches(&p.to_lowercase(), &name))
    }
}

/// Where RetroArch's cores are.
const CORES: &str = "/usr/lib/libretro";

/// RetroArch's systems: name, core, patterns, libretro's database.
const SYSTEMS: [(&str, &str, &[&str], &str); 9] = [
    ("NES", "nestopia", &["*.nes"], "Nintendo - Nintendo Entertainment System"),
    ("SNES", "snes9x", &["*.sfc", "*.smc"], "Nintendo - Super Nintendo Entertainment System"),
    ("Nintendo 64", "mupen64plus_next", &["*.z64", "*.n64", "*.v64"], "Nintendo - Nintendo 64"),
    ("Game Boy", "gambatte", &["*.gb", "*.gbc"], "Nintendo - Game Boy Color"),
    ("Game Boy Advance", "mgba", &["*.gba"], "Nintendo - Game Boy Advance"),
    ("Nintendo DS", "melonds", &["*.nds"], "Nintendo - Nintendo DS"),
    ("Mega Drive", "genesis_plus_gx", &["*.md", "*.gen", "*.smd"], "Sega - Mega Drive - Genesis"),
    ("PlayStation", "pcsx_rearmed", &["*.cue", "*.chd", "*.m3u", "*.pbp"], "Sony - PlayStation"),
    ("Dreamcast", "flycast", &["*.gdi", "*.cdi"], "Sega - Dreamcast"),
];

/// ClamShell's other runners: name, command, patterns.
const OTHERS: [(&str, &[&str], &[&str]); 6] = [
    ("Video", &["mpv", "{path}"], &["*.mkv", "*.mp4", "*.m4v", "*.avi", "*.webm", "*.mov", "*.mpg", "*.mpeg", "*.ts", "*.wmv", "*.flv"]),
    ("Music", &["mpv", "--force-window=immediate", "{path}"], &["*.mp3", "*.flac", "*.ogg", "*.opus", "*.m4a", "*.wav", "*.aac"]),
    ("Images", &["imv", "{path}"], &["*.jpg", "*.jpeg", "*.png", "*.webp", "*.gif", "*.bmp", "*.tif", "*.tiff", "*.svg"]),
    ("PDF", &["zathura", "{path}"], &["*.pdf", "*.epub", "*.djvu"]),
    (
        "Text",
        &["foot", "less", "{path}"],
        &[
            "*.txt", "*.md", "*.log", "*.nfo", "*.json", "*.toml", "*.yaml", "*.yml", "*.ini", "*.conf", "*.cfg", "*.csv", "*.xml", "*.sh", "*.py", "*.rs",
            "*.c", "*.h",
        ],
    ),
    ("Other files", &["xdg-open", "{path}"], &["*"]),
];

/// ClamShell's own runners, those whose program (and core) is installed, in order (Other files last).
/// `installed`: whether a program is on the PATH; `core`: whether a core's file is there.
pub fn builtin(installed: &dyn Fn(&str) -> bool, core: &dyn Fn(&Path) -> bool) -> Vec<Runner> {
    let strings = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let mut runners = Vec::new();
    if installed("retroarch") {
        for (name, core_name, patterns, database) in SYSTEMS {
            let file = Path::new(CORES).join(format!("{core_name}_libretro.so"));
            if core(&file) {
                let launch = vec!["retroarch".into(), "-f".into(), "-L".into(), file.to_string_lossy().into_owned(), "{path}".into()];
                runners.push(Runner {
                    name: name.into(),
                    patterns: strings(patterns),
                    launch,
                    metadata: Some(format!("libretro:{database}")),
                    scan: Vec::new(),
                });
            }
        }
    }
    for (name, launch, patterns) in OTHERS {
        if launch.first().is_some_and(|p| installed(p)) {
            runners.push(Runner { name: name.into(), patterns: strings(patterns), launch: strings(launch), ..Runner::default() });
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
    let mut runners: Vec<Runner> = config
        .iter()
        .map(|p| match builtin.iter().find(|b| b.name == p.name) {
            Some(b) => Runner {
                name: p.name.clone(),
                patterns: if p.patterns.is_empty() { b.patterns.clone() } else { p.patterns.clone() },
                launch: if p.launch.is_empty() { b.launch.clone() } else { p.launch.clone() },
                metadata: p.metadata.clone().or_else(|| b.metadata.clone()),
                scan: p.scan.clone(),
            },
            None => p.clone(),
        })
        .collect();
    for p in runners.iter().filter(|p| p.launch.is_empty()) {
        problems.push(format!("{}: no launch command (and ClamShell has no runner of that name)", p.name));
    }
    runners.retain(|p| !p.launch.is_empty());
    runners.extend(builtin.into_iter().filter(|b| !config.iter().any(|p| p.name == b.name)));
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
            r#"[{ "name": "SNES", "scan": [{ "id": "snes", "dir": "~/snes", "tab": "Games", "group": "SNES", "match": ["*.zip", "*.sfc"] }] },
                { "name": "Text", "match": ["*.txt"], "launch": ["kate", "{path}"] },
                { "name": "Broken" }]"#,
        )
        .unwrap();
        let (runners, problems) = all(&config, builtin(&|_| true, &|_| true));
        assert_eq!(names(&runners)[..2], ["SNES", "Text"]);
        assert_eq!(problems.len(), 1, "Broken has no command");
        assert!(!names(&runners).contains(&"Broken"));
        assert_eq!(runners.iter().filter(|p| p.name == "Text").count(), 1, "library.json's instead of ClamShell's");
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
