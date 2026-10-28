//! The edit mode's form for one source or runner (see [`super::edit`]): its fields, by its kind, each
//! shown as text, set by typing (text, a list of patterns, a command), by choosing among options (a kind,
//! a place, a runner), or by a tap (on or off). Lists and commands are typed as words, a word with spaces
//! in double quotes.

use clamshell_library::config::{BrowseRoot, BuiltinPlace, Config, MountRule, Rule, Runner, RunnerKind, Scan, Source};

/// What a form edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Source(Source),
    Runner(Runner),
}

/// A field of a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// A source's type, or a runner's (files, desktop, Steam).
    Kind,
    Id,
    Name,
    Dir,
    /// Where a source's entries go: a place.
    Place,
    /// The place a source's own folder is in.
    Within,
    Runner,
    Match,
    Also,
    Items,
    Metadata,
    Categories,
    Label,
    Uuid,
    Fs,
    Hide,
    Trash,
    /// Which of ClamShell's screens a builtin source places.
    Builtin,
    /// A runner's command.
    Launch,
    Keymap,
    /// Where a runner's emulator looks for its BIOS files, and their names.
    BiosDir,
    BiosFiles,
}

/// How a field is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    Typed,
    Chosen,
    Toggled,
}

impl Field {
    /// Its label in the form.
    pub fn label(self) -> &'static str {
        match self {
            Field::Kind => "Type",
            Field::Id => "Id",
            Field::Name => "Name",
            Field::Dir => "Folder",
            Field::Place => "Goes in",
            Field::Within => "Its folder is in",
            Field::Runner => "Opened by",
            Field::Match => "Files",
            Field::Also => "Also",
            Field::Items => "Items in folders",
            Field::Metadata => "Pictures from",
            Field::Categories => "Categories",
            Field::Label => "Label",
            Field::Uuid => "UUID",
            Field::Fs => "Filesystem",
            Field::Hide => "Hidden",
            Field::Trash => "The Trash",
            Field::Builtin => "Screen",
            Field::Launch => "Command",
            Field::Keymap => "Keymap",
            Field::BiosDir => "BIOS folder",
            Field::BiosFiles => "BIOS files",
        }
    }

    /// What it may be picked as in the Library (see [`super::pick`]), if anything. A command's file is
    /// its program.
    pub fn picks(self) -> Option<super::pick::Takes> {
        match self {
            Field::Dir | Field::BiosDir => Some(super::pick::Takes::Folder),
            Field::Launch => Some(super::pick::Takes::Program),
            _ => None,
        }
    }

    /// What a pick for it asks for.
    pub fn asks(self) -> &'static str {
        match self {
            Field::Launch => "Choose the program",
            Field::BiosDir => "Choose the BIOS folder",
            _ => "Choose the folder",
        }
    }

    pub fn set(self) -> Set {
        match self {
            Field::Kind | Field::Place | Field::Within | Field::Runner | Field::Builtin => Set::Chosen,
            Field::Hide | Field::Trash => Set::Toggled,
            _ => Set::Typed,
        }
    }
}

/// A source's type, as library.json names it.
pub fn source_kind(s: &Source) -> &'static str {
    match s {
        Source::Dir(_) => "dir",
        Source::Browse(_) => "browse",
        Source::Desktop(_) => "desktop",
        Source::Steam(_) => "steam",
        Source::Removable(_) => "removable",
        Source::Builtin(_) => "builtin",
    }
}

/// The source types, as they are offered.
const SOURCE_KINDS: [(&str, &str); 6] = [
    ("dir", "Files in a folder"),
    ("desktop", "Apps"),
    ("steam", "Steam games"),
    ("browse", "A browsable folder"),
    ("removable", "Removable storage"),
    ("builtin", "Library or Device"),
];

impl Item {
    /// Its fields, in order.
    pub fn fields(&self) -> &'static [Field] {
        use Field::*;
        match self {
            Item::Source(Source::Dir(_)) => &[Kind, Id, Dir, Place, Runner, Match, Also, Items, Metadata],
            Item::Source(Source::Browse(_)) => &[Kind, Dir, Within, Name, Trash],
            Item::Source(Source::Desktop(_)) => &[Kind, Id, Name, Categories, Place, Runner, Metadata, Hide],
            Item::Source(Source::Steam(_)) => &[Kind, Id, Name, Place, Runner, Metadata, Hide],
            Item::Source(Source::Removable(_)) => &[Kind, Label, Uuid, Fs, Within, Name, Hide],
            Item::Source(Source::Builtin(_)) => &[Kind, Builtin, Place],
            Item::Runner(_) => &[Kind, Id, Name, Match, Launch, Keymap, BiosDir, BiosFiles],
        }
    }

    /// Field `f`'s value as it shows, and as it is typed again.
    pub fn value(&self, config: &Config, f: Field) -> String {
        let opt = |o: &Option<String>| o.clone().unwrap_or_default();
        let place = |id: &str| config.places.iter().find(|p| p.id == id).map_or_else(|| id.to_owned(), |p| p.name.clone());
        let place_opt = |o: &Option<String>| o.as_deref().map(place).unwrap_or_default();
        match (self, f) {
            (Item::Source(s), Field::Kind) => SOURCE_KINDS.iter().find(|(k, _)| *k == source_kind(s)).map_or("", |(_, l)| l).to_owned(),
            (Item::Runner(r), Field::Kind) => runner_kind(r.kind).to_owned(),
            (Item::Source(Source::Dir(d)), f) => match f {
                Field::Id => d.id.clone(),
                Field::Dir => d.dir.clone(),
                Field::Place => place(&d.place),
                Field::Runner => opt(&d.runner),
                Field::Match => words(&d.patterns),
                Field::Also => words(&d.also),
                Field::Items => words(&d.items),
                Field::Metadata => opt(&d.metadata),
                _ => String::new(),
            },
            (Item::Source(Source::Browse(b)), f) => match f {
                Field::Dir => b.dir.clone(),
                Field::Within => place(&b.within),
                Field::Name => b.name.clone(),
                Field::Trash => on_off(b.trash),
                _ => String::new(),
            },
            (Item::Source(Source::Desktop(r) | Source::Steam(r)), f) => match f {
                Field::Id => opt(&r.id),
                Field::Name => opt(&r.name),
                Field::Categories => opt(&r.categories),
                Field::Place => place_opt(&r.place),
                Field::Runner => opt(&r.runner),
                Field::Metadata => opt(&r.metadata),
                Field::Hide => on_off(r.hide),
                _ => String::new(),
            },
            (Item::Source(Source::Removable(m)), f) => match f {
                Field::Label => opt(&m.label),
                Field::Uuid => opt(&m.uuid),
                Field::Fs => opt(&m.fs),
                Field::Within => place_opt(&m.within),
                Field::Name => m.name.clone().unwrap_or_else(|| "{label}".into()),
                Field::Hide => on_off(m.hide),
                _ => String::new(),
            },
            (Item::Source(Source::Builtin(b)), f) => match f {
                Field::Builtin => b.id.clone(),
                Field::Place => place(&b.place),
                _ => String::new(),
            },
            (Item::Runner(r), f) => match f {
                Field::Id => r.id.clone(),
                Field::Name => r.name.clone(),
                Field::Match => words(&r.patterns),
                Field::Launch => words(&r.launch),
                Field::Keymap => opt(&r.keymap),
                Field::BiosDir => r.bios.as_ref().map(|b| b.dir.clone()).unwrap_or_default(),
                Field::BiosFiles => r.bios.as_ref().map(|b| words(&b.files)).unwrap_or_default(),
                _ => String::new(),
            },
        }
    }

    /// Sets typed field `f` to `text`.
    pub fn type_in(&mut self, f: Field, text: &str) {
        let text = text.trim();
        let some = || (!text.is_empty()).then(|| text.to_owned());
        match self {
            Item::Source(Source::Dir(d)) => match f {
                Field::Id => text.clone_into(&mut d.id),
                Field::Dir => text.clone_into(&mut d.dir),
                Field::Match => d.patterns = split(text),
                Field::Also => d.also = split(text),
                Field::Items => d.items = split(text),
                Field::Metadata => d.metadata = some(),
                _ => {}
            },
            Item::Source(Source::Browse(b)) => match f {
                Field::Dir => text.clone_into(&mut b.dir),
                Field::Name => text.clone_into(&mut b.name),
                _ => {}
            },
            Item::Source(Source::Desktop(r) | Source::Steam(r)) => match f {
                Field::Id => r.id = some(),
                Field::Name => r.name = some(),
                Field::Categories => r.categories = some(),
                Field::Metadata => r.metadata = some(),
                _ => {}
            },
            Item::Source(Source::Removable(m)) => match f {
                Field::Label => m.label = some(),
                Field::Uuid => m.uuid = some(),
                Field::Fs => m.fs = some(),
                Field::Name => m.name = some().filter(|n| n != "{label}"),
                _ => {}
            },
            Item::Source(Source::Builtin(_)) => {}
            Item::Runner(r) => match f {
                Field::Id => text.clone_into(&mut r.id),
                Field::Name => text.clone_into(&mut r.name),
                Field::Match => r.patterns = split(text),
                Field::Launch => r.launch = split(text),
                Field::Keymap => r.keymap = some(),
                Field::BiosDir | Field::BiosFiles => {
                    let bios = r.bios.get_or_insert_with(Default::default);
                    match f {
                        Field::BiosDir => text.clone_into(&mut bios.dir),
                        _ => bios.files = split(text),
                    }
                    // Neither: no BIOS needed.
                    if bios.dir.is_empty() && bios.files.is_empty() {
                        r.bios = None;
                    }
                }
                _ => {}
            },
        }
    }

    /// Picked field `f` set to `path`: a command's program replaced, keeping its arguments, or the path.
    pub fn picked(&mut self, f: Field, path: &str) {
        match (self, f) {
            (Item::Runner(r), Field::Launch) => match r.launch.first_mut() {
                Some(program) => path.clone_into(program),
                None => r.launch.push(path.to_owned()),
            },
            (item, f) => item.type_in(f, path),
        }
    }

    /// Flips toggled field `f`.
    pub fn toggle(&mut self, f: Field) {
        match (self, f) {
            (Item::Source(Source::Browse(b)), Field::Trash) => b.trash = !b.trash,
            (Item::Source(Source::Desktop(r) | Source::Steam(r)), Field::Hide) => r.hide = !r.hide,
            (Item::Source(Source::Removable(m)), Field::Hide) => m.hide = !m.hide,
            _ => {}
        }
    }

    /// The options of chosen field `f`, each its value and how it shows.
    pub fn options(&self, config: &Config, f: Field) -> Vec<(String, String)> {
        let places = || config.places.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        match (self, f) {
            (Item::Source(_), Field::Kind) => SOURCE_KINDS.iter().map(|(k, l)| ((*k).to_owned(), (*l).to_owned())).collect(),
            (Item::Runner(_), Field::Kind) => ["files", "desktop", "steam"].iter().map(|k| ((*k).to_owned(), (*k).to_owned())).collect(),
            (_, Field::Place | Field::Within) => places(),
            (Item::Source(s), Field::Runner) => {
                // A scan's runner opens files; a desktop or Steam source's is one of that type.
                let kind = match s {
                    Source::Desktop(_) => Some(RunnerKind::Desktop),
                    Source::Steam(_) => Some(RunnerKind::Steam),
                    _ => None,
                };
                let none = std::iter::once((String::new(), "By the file's name".to_owned())).filter(|_| kind.is_none());
                none.chain(
                    config.runners.iter().filter(|r| r.kind == kind).map(|r| (r.id.clone(), if r.name.is_empty() { r.id.clone() } else { r.name.clone() })),
                )
                .collect()
            }
            (_, Field::Builtin) => [("library", "Library"), ("device", "Device")].iter().map(|(k, l)| ((*k).to_owned(), (*l).to_owned())).collect(),
            _ => Vec::new(),
        }
    }

    /// Sets chosen field `f` to `value`, one of its options. A source's new type starts it over, keeping
    /// its place.
    pub fn choose(&mut self, f: Field, value: &str) {
        let some = (!value.is_empty()).then(|| value.to_owned());
        match (self, f) {
            (Item::Source(s), Field::Kind) => {
                let place = source_place(s).map(str::to_owned).unwrap_or_default();
                *s = match value {
                    "dir" => Source::Dir(Scan { place, ..Scan::default() }),
                    "browse" => Source::Browse(BrowseRoot { within: place, ..BrowseRoot::default() }),
                    "desktop" => Source::Desktop(Rule { place: Some(place), ..Rule::default() }),
                    "steam" => Source::Steam(Rule { place: Some(place), ..Rule::default() }),
                    "removable" => Source::Removable(MountRule { within: Some(place), ..MountRule::default() }),
                    _ => Source::Builtin(BuiltinPlace { id: "device".into(), place }),
                };
            }
            (Item::Runner(r), Field::Kind) => {
                r.kind = match value {
                    "desktop" => Some(RunnerKind::Desktop),
                    "steam" => Some(RunnerKind::Steam),
                    _ => None,
                }
            }
            (Item::Source(s), Field::Place | Field::Within) => match s {
                Source::Dir(d) => value.clone_into(&mut d.place),
                Source::Browse(b) => value.clone_into(&mut b.within),
                Source::Desktop(r) | Source::Steam(r) => r.place = some,
                Source::Removable(m) => m.within = some,
                Source::Builtin(b) => value.clone_into(&mut b.place),
            },
            (Item::Source(Source::Dir(d)), Field::Runner) => d.runner = some,
            (Item::Source(Source::Desktop(r) | Source::Steam(r)), Field::Runner) => r.runner = some,
            (Item::Source(Source::Builtin(b)), Field::Builtin) => value.clone_into(&mut b.id),
            _ => {}
        }
    }
}

/// Where a source puts its entries, or its own folder: a place's id.
pub fn source_place(s: &Source) -> Option<&str> {
    match s {
        Source::Dir(d) => Some(&d.place),
        Source::Browse(b) => Some(&b.within),
        Source::Desktop(r) | Source::Steam(r) => r.place.as_deref(),
        Source::Removable(m) => m.within.as_deref(),
        Source::Builtin(b) => Some(&b.place),
    }
}

/// A runner's type, as it shows.
fn runner_kind(kind: Option<RunnerKind>) -> &'static str {
    match kind {
        None => "files",
        Some(RunnerKind::Desktop) => "desktop",
        Some(RunnerKind::Steam) => "steam",
    }
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.into()
}

/// Words as they are typed: separated by spaces, one with a space in double quotes.
pub fn words(list: &[String]) -> String {
    let quoted = |w: &String| if w.contains(' ') || w.is_empty() { format!("\"{w}\"") } else { w.clone() };
    list.iter().map(quoted).collect::<Vec<_>>().join(" ")
}

/// Typed words back into a list (see [`words`]).
pub fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            ' ' if !quoted => {
                if any {
                    out.push(std::mem::take(&mut word));
                }
                any = false;
            }
            c => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(word);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_library::config::Place;

    #[test]
    fn words_split_and_join_back() {
        let list = vec!["retroarch".to_owned(), "-L".into(), "/a b/core.so".into(), "{path}".into()];
        assert_eq!(words(&list), r#"retroarch -L "/a b/core.so" {path}"#);
        assert_eq!(split(&words(&list)), list);
        assert_eq!(split("  *.sfc   *.smc "), ["*.sfc", "*.smc"]);
    }

    /// A field typed, chosen or toggled sets what it shows; a new type keeps the place.
    #[test]
    fn fields_are_set_as_they_show() {
        let config = Config { places: vec![Place { id: "snes".into(), name: "SNES".into(), ..Place::default() }], ..Config::default() };
        let mut item = Item::Source(Source::Dir(Scan::default()));
        item.type_in(Field::Dir, " ~/Roms/snes ");
        item.choose(Field::Place, "snes");
        item.type_in(Field::Also, "*.zip *.7z");
        assert_eq!(item.value(&config, Field::Place), "SNES");
        assert_eq!(item.value(&config, Field::Also), "*.zip *.7z");
        item.choose(Field::Kind, "desktop");
        assert_eq!(item.fields().first(), Some(&Field::Kind));
        assert_eq!(item.value(&config, Field::Place), "SNES", "kept across the new type");
        item.toggle(Field::Hide);
        assert_eq!(item.value(&config, Field::Hide), "on");
    }

    /// A runner's program is picked into its command, which keeps its arguments; its BIOS folder and
    /// files make its BIOS, and emptied, there is none.
    #[test]
    fn a_runners_program_and_bios_are_set() {
        use clamshell_library::config::Runner;
        let mut item = Item::Runner(Runner { launch: split("retroarch -f {path}"), ..Runner::default() });
        item.picked(Field::Launch, "/usr/bin/dolphin-emu");
        assert_eq!(item.value(&Config::default(), Field::Launch), "/usr/bin/dolphin-emu -f {path}");
        item.picked(Field::BiosDir, "/h/bios");
        item.type_in(Field::BiosFiles, "a.bin b.bin");
        assert!(matches!(&item, Item::Runner(r) if r.bios.as_ref().is_some_and(|b| b.dir == "/h/bios" && b.files == ["a.bin", "b.bin"])));
        item.type_in(Field::BiosDir, "");
        item.type_in(Field::BiosFiles, "");
        assert!(matches!(&item, Item::Runner(r) if r.bios.is_none()));
    }

    /// A command is typed with its placeholders beside the keyboard, and a tap types one in.
    #[test]
    fn a_commands_placeholders_are_typed_in() {
        use crate::testkit::*;
        use crate::{EditorId, Event, Id};
        let mut s = state();
        let editor = crate::Editor { purpose: crate::bottom::editor::Purpose::FormField(Field::Launch), text: "mpv ".into() };
        assert_eq!(editor.snippets().first(), Some(&"{path}"));
        s.applied(Event::EditOnKeyboard(editor));
        tap(&mut s, Id::Editor(EditorId::Insert(0)));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("mpv {path}"));
    }
}
