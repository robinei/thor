//! library.json as data: its places, runners, sources and keys, as read and as written back. What they
//! mean, and how a library is built from them, is clamshell-system's (`library::content`,
//! `library::runners`); docs/runners.md describes the file. The state edits these (the Library's edit
//! mode) and has them written; with the `serde` feature they are read and written.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Config {
    /// The declared places: the tabs and the folders in them, by their ids, with their icons and
    /// pictures. They set the tabs' order and declare ClamShell's own tabs.
    #[cfg_attr(feature = "serde", serde(default))]
    pub places: Vec<Place>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub runners: Vec<Runner>,
    /// Where entries come from, and where they go.
    #[cfg_attr(feature = "serde", serde(default))]
    pub sources: Vec<Source>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub keys: Keys,
}

/// A declared place: a tab, or a folder `in` another place.
///
/// - `id` refers to it elsewhere, as `"place": "snes"`. Its folder's id is `place:snes`, which stays the
///   same as it is renamed or moved.
/// - `name` is its title, and `in` the id of the place it is in; without one, it is a tab.
/// - `icon` is one of the UI's icons by name, such as `gamepad`. A folder's icon can also be an image
///   file, shown in its row.
/// - `picture` is a folder's image file, shown large when it is selected. A tab has none, because
///   something in it is always selected.
/// - `recent: false` keeps what is in it out of Recent, for apps. Its folders inherit this unless they
///   set it themselves.
/// - `type` makes it one of ClamShell's own tabs (`recent`, `favourites`, `tools`).
///
/// Image file paths starting with `~/` are in the home dir; others are relative to library.json's dir.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Place {
    pub id: String,
    pub name: String,
    #[cfg_attr(feature = "serde", serde(default, rename = "in", skip_serializing_if = "Option::is_none"))]
    pub within: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, rename = "type", skip_serializing_if = "Option::is_none"))]
    pub kind: Option<OwnTab>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub icon: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub picture: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub recent: Option<bool>,
}

/// One of ClamShell's own tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(rename_all = "lowercase"))]
pub enum OwnTab {
    /// The entries played last, except those in a place with `recent: false`.
    Recent,
    Favourites,
    /// ClamShell's own tools, such as rescanning the library and fetching what is missing again.
    Tools,
}

/// Where entries come from, by `type`, and where they go. The types are a scanned dir (`dir`), a browsed
/// dir (`browse`), the apps and games from desktop files (`desktop`), Steam's installed games (`steam`),
/// the mounted removable filesystems (`removable`) and ClamShell's own screens (`builtin`).
///
/// For `desktop`, `steam` and `removable`, the sources of a type take their entries in turn. Each takes
/// the remaining ones it matches and puts them at its place, or leaves them out with `hide`. Entries no
/// source takes are left out.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(tag = "type", rename_all = "lowercase"))]
pub enum Source {
    Dir(Scan),
    Browse(BrowseRoot),
    Desktop(Rule),
    Steam(Rule),
    Removable(MountRule),
    Builtin(BuiltinPlace),
}

/// A dir whose entries go in the library.
///
/// `id` names the entries as `<id>:<path in dir>`, and `place`, a declared place's id, says where they
/// show. `match` and `items` say what is an entry: `match` is matched against paths in `dir`, one part
/// per folder level (`*/*.cue`), and `items` against a matched folder's own entries; without `match`, the
/// runner's patterns are used, and `also` adds to either. `metadata` says where their covers and details
/// come from. `runner`, an id, opens them all; without it, each opens with the runner its file name
/// matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Scan {
    pub id: String,
    pub dir: String,
    /// Where its entries go: a declared place's id.
    pub place: String,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub runner: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, rename = "match", skip_serializing_if = "Vec::is_empty"))]
    pub patterns: Vec<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub also: Vec<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub items: Vec<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub metadata: Option<String>,
    /// The removable filesystem `dir` is on: the first mounted one matching this. `dir` is then a path
    /// inside it (`roms/snes`), and the source is away while none is mounted.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub on: Option<On>,
}

/// A removable filesystem, matched by its `label`, `uuid` and `fs`. These are globs in any case, and an
/// unset one matches anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct On {
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub label: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub uuid: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub fs: Option<String>,
}

/// A browsed dir. It is a folder of its own, titled `name`, in the place `in`, listed when it is opened.
/// `trash` marks it as the Trash's `files` dir, whose entries can be restored or deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct BrowseRoot {
    pub dir: String,
    #[cfg_attr(feature = "serde", serde(rename = "in"))]
    pub within: String,
    pub name: String,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "std::ops::Not::not"))]
    pub trash: bool,
}

/// A desktop or Steam source. The remaining desktop file entries or Steam games that match all of its
/// `id`, `name` and `categories` go at `place`, a declared place's id, or are left out with `hide`. The
/// fields are globs in any case, and an unset one matches anything. `categories` matches if any of an
/// entry's categories does; Steam games have none.
///
/// `runner`, an id, is the runner that opens them; by default it is the first runner of the type.
/// `metadata` is where their covers and details come from: `steamgriddb`, or `steam` (the Steam store)
/// for a Steam source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Rule {
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub id: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub name: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub categories: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub place: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub runner: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub metadata: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "std::ops::Not::not"))]
    pub hide: bool,
}

/// A removable source. The remaining mounted removable filesystems that match all of its `label`, `uuid`
/// and `fs` go in the place `in` as their own folders, or are left out with `hide`. The fields are globs
/// in any case, and an unset one matches anything. The folder is titled `name`, where `{label}` is
/// replaced by the filesystem's label, or its UUID if it has none; that is the default name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct MountRule {
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub label: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub uuid: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub fs: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, rename = "in", skip_serializing_if = "Option::is_none"))]
    pub within: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub name: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "std::ops::Not::not"))]
    pub hide: bool,
}

/// One of ClamShell's own screens, by `id` (`library`, `device`), placed at `place`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct BuiltinPlace {
    pub id: String,
    pub place: String,
}

/// A runner, as library.json gives it (see docs/runners.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Runner {
    /// Set for a runner that finds its items itself. Without it, the runner opens files with its command.
    #[cfg_attr(feature = "serde", serde(default, rename = "type", skip_serializing_if = "Option::is_none"))]
    pub kind: Option<RunnerKind>,
    /// What its settings, environment and keymap are kept by. Without one, its name in lowercase with
    /// spaces as `-`.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "String::is_empty"))]
    pub id: String,
    /// Its settings page's title.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "String::is_empty"))]
    pub name: String,
    /// Patterns for the file names it opens, like `*.sfc`. Each matches a single name, in any case.
    #[cfg_attr(feature = "serde", serde(default, rename = "match", skip_serializing_if = "Vec::is_empty"))]
    pub patterns: Vec<String>,
    /// The command, with `{path}` for the file (`{paths}`: all the files; `{socket}`: a control
    /// socket's path). A desktop runner's command, if it has one, replaces the command from each entry's
    /// desktop file: `{id}` is the desktop file's name and `{exec}` its command, as separate arguments.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub launch: Vec<String>,
    /// One of ClamShell's keymaps that it comes with, such as `viewer`, used until another is chosen.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub keymap: Option<String>,
    /// Whether its entries have their own settings tab beside the runner's. Defaults to on.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub item_settings: Option<bool>,
    /// What to write to a launch's control socket to add a file while it runs (`{path}` the file, as JSON
    /// string contents). If set, its entries can be enqueued.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub enqueue: Option<String>,
    /// Whether its launches hand their app over to something running outside them and then end (a desktop
    /// runner such as Waydroid's): its entries are tracked by their windows, found by their app id.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "std::ops::Not::not"))]
    pub handover: bool,
    /// For a runner that hands its apps over, the command that ends one, with `{id}` for its app id.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub stop: Vec<String>,
    /// For a runner whose entries share one container, the commands that pause and resume all of it.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub pause: Vec<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub resume: Vec<String>,
    /// A command that succeeds while its app on screen keeps the screen on in a way the compositor doesn't
    /// see; run before the screens dim.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Vec::is_empty"))]
    pub awake: Vec<String>,
    /// The BIOS files its emulator needs, and the folder it looks for them in.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub bios: Option<Bios>,
}

impl Runner {
    /// The id sources name it by: its own, or else made from its name.
    pub fn effective_id(&self) -> String {
        if self.id.is_empty() { self.name.to_lowercase().replace(' ', "-") } else { self.id.clone() }
    }

    /// Whether `id` is its [`Runner::effective_id`], compared without making it.
    pub fn is_named(&self, id: &str) -> bool {
        if self.id.is_empty() { id.chars().eq(self.name.chars().flat_map(char::to_lowercase).map(|c| if c == ' ' { '-' } else { c })) } else { self.id == id }
    }
}

/// A runner that finds its items itself, each with its own command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(rename_all = "lowercase"))]
pub enum RunnerKind {
    /// The apps and games from desktop files, run by their `Exec`, and a `.desktop` file opened from the
    /// Files tab.
    Desktop,
    /// Steam's installed games, run by Steam.
    Steam,
}

/// A runner's BIOS files (see [`Runner::bios`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Bios {
    /// Where its emulator looks for them; `~` and environment variables as in a source's `dir`.
    pub dir: String,
    pub files: Vec<String>,
}

/// The keys of the metadata services that need one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize), serde(deny_unknown_fields))]
pub struct Keys {
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub tmdb: Option<String>,
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub steamgriddb: Option<String>,
}
