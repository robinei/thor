//! The edit mode's form for one source or runner (see [`super::edit`]): its fields, by its kind, each
//! shown as text, set by typing (text, a list of patterns, a command), by choosing among options (a kind,
//! a place, a runner), or by a tap (on or off). Lists and commands are typed as words, a word with spaces
//! in double quotes.

use clamshell_library::BuiltinApp;
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

/// A source or runner borrowed, as it is read and drawn (see [`Item`]).
#[derive(Debug, Clone, Copy)]
pub enum ItemRef<'a> {
    Source(&'a Source),
    Runner(&'a Runner),
}

/// A field's value as it shows, borrowed: text, words as typed (see [`words`]), or on or off.
#[derive(Debug, Clone, Copy)]
pub enum Value<'a> {
    Text(&'a str),
    Words(&'a [String]),
    OnOff(bool),
}

impl std::fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Text(t) => f.write_str(t),
            Value::Words(list) => write!(f, "{}", Words(list)),
            Value::OnOff(on) => f.write_str(if *on { "on" } else { "off" }),
        }
    }
}

impl<'a> ItemRef<'a> {
    /// Its fields, in order.
    pub fn fields(self) -> &'static [Field] {
        use Field::*;
        match self {
            ItemRef::Source(Source::Dir(_)) => &[Kind, Id, Dir, Place, Runner, Match, Also, Items, Metadata],
            ItemRef::Source(Source::Browse(_)) => &[Kind, Dir, Within, Name, Trash],
            ItemRef::Source(Source::Desktop(_)) => &[Kind, Id, Name, Categories, Place, Runner, Metadata, Hide],
            ItemRef::Source(Source::Steam(_)) => &[Kind, Id, Name, Place, Runner, Metadata, Hide],
            ItemRef::Source(Source::Removable(_)) => &[Kind, Label, Uuid, Fs, Within, Name, Hide],
            ItemRef::Source(Source::Builtin(_)) => &[Kind, Builtin, Place],
            ItemRef::Runner(_) => &[Kind, Id, Name, Match, Launch, Keymap, BiosDir, BiosFiles],
        }
    }

    /// Field `f`'s value as it shows.
    pub fn value(self, config: &'a Config, f: Field) -> Value<'a> {
        let text = |o: &'a Option<String>| Value::Text(o.as_deref().unwrap_or(""));
        let place = |id: &'a str| Value::Text(config.places.iter().find(|p| p.id == id).map_or(id, |p| p.name.as_str()));
        let place_opt = |o: &'a Option<String>| o.as_deref().map_or(Value::Text(""), place);
        let none = Value::Text("");
        match (self, f) {
            (ItemRef::Source(s), Field::Kind) => Value::Text(SOURCE_KINDS.iter().find(|(k, _)| *k == source_kind(s)).map_or("", |(_, l)| l)),
            (ItemRef::Runner(r), Field::Kind) => Value::Text(runner_kind(r.kind)),
            (ItemRef::Source(Source::Dir(d)), f) => match f {
                Field::Id => Value::Text(&d.id),
                Field::Dir => Value::Text(&d.dir),
                Field::Place => place(&d.place),
                Field::Runner => text(&d.runner),
                Field::Match => Value::Words(&d.patterns),
                Field::Also => Value::Words(&d.also),
                Field::Items => Value::Words(&d.items),
                Field::Metadata => text(&d.metadata),
                _ => none,
            },
            (ItemRef::Source(Source::Browse(b)), f) => match f {
                Field::Dir => Value::Text(&b.dir),
                Field::Within => place(&b.within),
                Field::Name => Value::Text(&b.name),
                Field::Trash => Value::OnOff(b.trash),
                _ => none,
            },
            (ItemRef::Source(Source::Desktop(r) | Source::Steam(r)), f) => match f {
                Field::Id => text(&r.id),
                Field::Name => text(&r.name),
                Field::Categories => text(&r.categories),
                Field::Place => place_opt(&r.place),
                Field::Runner => text(&r.runner),
                Field::Metadata => text(&r.metadata),
                Field::Hide => Value::OnOff(r.hide),
                _ => none,
            },
            (ItemRef::Source(Source::Removable(m)), f) => match f {
                Field::Label => text(&m.label),
                Field::Uuid => text(&m.uuid),
                Field::Fs => text(&m.fs),
                Field::Within => place_opt(&m.within),
                Field::Name => Value::Text(m.name.as_deref().unwrap_or("{label}")),
                Field::Hide => Value::OnOff(m.hide),
                _ => none,
            },
            (ItemRef::Source(Source::Builtin(b)), f) => match f {
                Field::Builtin => Value::Text(&b.id),
                Field::Place => place(&b.place),
                _ => none,
            },
            (ItemRef::Runner(r), f) => match f {
                Field::Id => Value::Text(&r.id),
                Field::Name => Value::Text(&r.name),
                Field::Match => Value::Words(&r.patterns),
                Field::Launch => Value::Words(&r.launch),
                Field::Keymap => text(&r.keymap),
                Field::BiosDir => Value::Text(r.bios.as_ref().map_or("", |b| b.dir.as_str())),
                Field::BiosFiles => r.bios.as_ref().map_or(none, |b| Value::Words(&b.files)),
                _ => none,
            },
        }
    }
}

impl ItemRef<'_> {
    /// Visits chosen field `f`'s options, each as its value and its label, borrowed (a runner without an
    /// id has one made from its name).
    pub fn each_option(self, config: &Config, f: Field, mut visit: impl FnMut(&str, &str)) {
        match (self, f) {
            (ItemRef::Source(_), Field::Kind) => SOURCE_KINDS.iter().for_each(|(k, l)| visit(k, l)),
            (ItemRef::Runner(_), Field::Kind) => ["files", "desktop", "steam"].iter().for_each(|k| visit(k, k)),
            (_, Field::Place | Field::Within) => config.places.iter().for_each(|p| visit(&p.id, &p.name)),
            (ItemRef::Source(s), Field::Runner) => {
                // A scan's runner opens files; a desktop or Steam source's is one of that type.
                let kind = match s {
                    Source::Desktop(_) => Some(RunnerKind::Desktop),
                    Source::Steam(_) => Some(RunnerKind::Steam),
                    _ => None,
                };
                if kind.is_none() {
                    visit("", "By the file's name");
                }
                for r in config.runners.iter().filter(|r| r.kind == kind) {
                    let made;
                    let id = if r.id.is_empty() {
                        made = r.effective_id();
                        made.as_str()
                    } else {
                        r.id.as_str()
                    };
                    visit(id, if r.name.is_empty() { id } else { &r.name });
                }
            }
            (_, Field::Builtin) => BuiltinApp::ENTRIES.iter().for_each(|app| visit(app.name(), app.title())),
            _ => {}
        }
    }
}

impl Item {
    /// Its fields, in order.
    pub fn fields(&self) -> &'static [Field] {
        self.as_ref().fields()
    }

    /// Field `f`'s value as it shows, and as it is typed again.
    pub fn value(&self, config: &Config, f: Field) -> String {
        self.as_ref().value(config, f).to_string()
    }

    /// It borrowed, to read.
    pub fn as_ref(&self) -> ItemRef<'_> {
        match self {
            Item::Source(s) => ItemRef::Source(s),
            Item::Runner(r) => ItemRef::Runner(r),
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
        let mut all = Vec::new();
        self.as_ref().each_option(config, f, |value, label| all.push((value.to_owned(), label.to_owned())));
        all
    }

    /// Sets chosen field `f` to `value`, one of its options. A source's new type starts it over, keeping
    /// its place.
    pub fn choose(&mut self, f: Field, value: &str) {
        let some = (!value.is_empty()).then(|| value.to_owned());
        match (self, f) {
            // The type it has already: nothing to start over.
            (Item::Source(s), Field::Kind) if value == source_kind(s) => {}
            (Item::Source(s), Field::Kind) => {
                let place = source_place(s).map(str::to_owned).unwrap_or_default();
                *s = match value {
                    "dir" => Source::Dir(Scan { place, ..Scan::default() }),
                    "browse" => Source::Browse(BrowseRoot { within: place, ..BrowseRoot::default() }),
                    "desktop" => Source::Desktop(Rule { place: Some(place), ..Rule::default() }),
                    "steam" => Source::Steam(Rule { place: Some(place), ..Rule::default() }),
                    "removable" => Source::Removable(MountRule { within: Some(place), ..MountRule::default() }),
                    // (The first that isn't the home, which is there already.)
                    _ => {
                        let app = BuiltinApp::ENTRIES.into_iter().find(|app| !crate::builtin_apps::definition(*app).home).unwrap_or_default();
                        Source::Builtin(BuiltinPlace { id: app.name().into(), place })
                    }
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

/// Words as they are typed: separated by spaces, one with a space (or none at all) in double quotes, and
/// a `"` or `\\` in one with a `\\` before it.
pub fn words(list: &[String]) -> String {
    Words(list).to_string()
}

/// Words as they are typed (see [`words`]), written without being collected first.
pub struct Words<'a>(pub &'a [String]);

impl std::fmt::Display for Words<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, w) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            let quoted = w.contains(' ') || w.is_empty();
            if quoted {
                f.write_str("\"")?;
            }
            for c in w.chars() {
                if c == '\\' || c == '"' {
                    f.write_str("\\")?;
                }
                write!(f, "{c}")?;
            }
            if quoted {
                f.write_str("\"")?;
            }
        }
        Ok(())
    }
}

/// Typed words back into a list (see [`words`]).
pub fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut any) = (false, false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            // The next character as it is.
            '\\' => {
                word.extend(chars.next());
                any = true;
            }
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
    /// Quotes and backslashes in a word come back as they were.
    #[test]
    fn words_with_quotes_round_trip() {
        let argv: Vec<String> = ["sh", "-c", "echo \"hi\"", "--title=\"X\"", "a\\b", "", "two words"].map(String::from).to_vec();
        assert_eq!(split(&words(&argv)), argv);
    }

    /// The type a source has, chosen again, leaves it as it is.
    #[test]
    fn choosing_a_sources_own_type_keeps_it() {
        let scan = Scan { id: "snes".into(), dir: "~/Roms/snes".into(), place: "snes".into(), ..Scan::default() };
        let mut item = Item::Source(Source::Dir(scan.clone()));
        item.choose(Field::Kind, "dir");
        assert_eq!(item, Item::Source(Source::Dir(scan)));
    }

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
        let editor = crate::Editor { purpose: crate::bottom::editor::Purpose::FormField(Field::Launch, 1), text: "mpv ".into() };
        assert_eq!(editor.snippets().first(), Some(&"{path}"));
        s.applied(Event::EditOnKeyboard(editor));
        tap(&mut s, Id::Editor(EditorId::Insert(0)));
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("mpv {path}"));
    }
}
