//! The Library's edit mode: its tabs and folders, which are library.json's places, edited where they show
//! (docs/top-screens-plan.md). The tabs are the places in no other; a tab's or an open folder's list is the
//! places in it, then the folders sources make there (browsed dirs, removable storage: edited with their
//! sources, so shown dimmed), then a row to make a new folder. The selected place's details have its
//! actions: Rename, New folder, Cut and then Paste to move it, and Remove, for a place nothing is in and
//! nothing uses.
//!
//! After the tabs come Sources and Runners: library.json's sources and runners in their order, each
//! selected showing what it is, with Edit (its form, see [`super::form`]), Move up and down, and Remove (a
//! runner only while no source names it); the last row makes a new one.
//!
//! Each change is made to library.json as read (see [`clamshell_library::config`]) and written; the
//! library is then read again. Until it comes, what was written is what shows (`pending`), so a change
//! shows at once.

use std::sync::Arc;

use clamshell_library::config::{Config, Place, Runner, Scan, Source};

use super::form::{Field, Item, Set};

use crate::bottom::editor::{Editor, Purpose};
use crate::effect::Effect;
use crate::out::Out;
use crate::{Event, Id, LibraryViewId};

/// A target of the edit mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EditId {
    /// A tab, by position among the tabs.
    Tab(u16),
    /// Makes a new tab.
    NewTab,
    /// A row of the list, by position.
    Row(u16),
    /// Out of the open folder.
    Up,
    /// Leaves the edit mode.
    Done,
    /// The selected place's actions.
    Rename,
    Cut,
    /// Moves the place cut into the folder open, or into the tab.
    Paste,
    Remove,
    /// The Sources and Runners tabs.
    Sources,
    Runners,
    /// The selected source's or runner's form, and its place in the order.
    Open,
    MoveUp,
    MoveDown,
    /// The form's field, by position, and the chosen field's option.
    Field(u16),
    Choice(u16),
    /// Writes the form, or leaves it as it was.
    Save,
    Cancel,
}

/// What the edit mode shows: the tabs' places, the sources or the runners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panel {
    #[default]
    Places,
    Sources,
    Runners,
}

/// A source's or runner's form, open (see [`super::form`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// Where it is in its list, or none for a new one.
    pub index: Option<usize>,
    pub item: Item,
    /// The field selected, and whether its options show.
    pub field: usize,
    pub choosing: bool,
}

/// What a typed name is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceName {
    /// The place with this id, renamed.
    Rename(String),
    /// A new place in this one, or a new tab.
    New(Option<String>),
}

/// A row of the edit mode's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row<'a> {
    /// A declared place in the tab or folder open.
    Place(&'a Place),
    /// A folder a source makes there, by its title.
    Made(&'a str),
    /// Makes a new folder there.
    New,
}

/// The edit mode's own state (see the module's doc).
#[derive(Debug, Clone, Default)]
pub struct Edit {
    panel: Panel,
    /// The form open over the sources or runners.
    form: Option<Form>,
    /// The tab, by position among the tabs.
    tab: usize,
    /// The folders opened in it, outermost first, by id.
    open: Vec<String>,
    /// The row selected.
    cursor: usize,
    /// The place cut, to be moved where it is pasted.
    cut: Option<String>,
    /// library.json as last written, until the library read after it comes.
    pending: Option<Arc<Config>>,
}

/// The tabs: the places in no other, in their order.
pub fn tabs(config: &Config) -> impl Iterator<Item = &Place> {
    config.places.iter().filter(|p| p.within.is_none())
}

/// The places in place `id`, in their order.
fn children<'a>(config: &'a Config, id: &'a str) -> impl Iterator<Item = &'a Place> + 'a {
    config.places.iter().filter(move |p| p.within.as_deref() == Some(id))
}

/// The folders sources make in place `id`: browsed dirs' and removable storage's, by title.
fn made<'a>(config: &'a Config, id: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    config.sources.iter().filter_map(move |s| match s {
        Source::Browse(b) if b.within == id => Some(b.name.as_str()),
        Source::Removable(r) if r.within.as_deref() == Some(id) && !r.hide => Some(r.name.as_deref().unwrap_or("{label}")),
        _ => None,
    })
}

/// Whether anything uses place `id`: a place in it, or a source putting entries there.
pub fn used(config: &Config, id: &str) -> bool {
    children(config, id).next().is_some()
        || config.sources.iter().any(|s| match s {
            Source::Dir(scan) => scan.place == id,
            Source::Browse(b) => b.within == id,
            Source::Desktop(r) | Source::Steam(r) => r.place.as_deref() == Some(id),
            Source::Removable(r) => r.within.as_deref() == Some(id),
            Source::Builtin(b) => b.place == id,
        })
}

/// A new place's id: its name in lowercase with what isn't a letter or a digit as `-`, made unique with
/// a number.
fn new_id(config: &Config, name: &str) -> String {
    let base: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "place" } else { base };
    let taken = |id: &str| config.places.iter().any(|p| p.id == id);
    (1..).map(|n| if n == 1 { base.to_owned() } else { format!("{base}-{n}") }).find(|id| !taken(id)).unwrap_or_default()
}

impl Edit {
    /// library.json as it shows: as last written, or else as `read`.
    pub fn config<'a>(&'a self, read: Option<&'a Arc<Config>>) -> Option<&'a Config> {
        self.pending.as_deref().or(read.map(|c| &**c))
    }

    /// The library was read again: what shows is it.
    pub(crate) fn library_read(&mut self) {
        self.pending = None;
    }

    pub fn tab(&self) -> usize {
        self.tab
    }

    pub fn panel(&self) -> Panel {
        self.panel
    }

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    /// How many rows the sources' or runners' list has: one each, and one to make a new one.
    pub fn listed(&self, config: &Config) -> usize {
        match self.panel {
            Panel::Sources => config.sources.len() + 1,
            Panel::Runners => config.runners.len() + 1,
            Panel::Places => 0,
        }
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn cut(&self) -> Option<&str> {
        self.cut.as_deref()
    }

    /// Selects row `i`.
    pub(crate) fn select(&mut self, i: usize) {
        self.cursor = i;
    }

    /// The tab's id, or the open folder's.
    pub fn here<'a>(&'a self, config: &'a Config) -> Option<&'a str> {
        self.open.last().map(String::as_str).or_else(|| tabs(config).nth(self.tab).map(|p| p.id.as_str()))
    }

    /// The open folders, outermost first.
    pub fn open<'a>(&'a self, config: &'a Config) -> impl Iterator<Item = &'a Place> + 'a {
        self.open.iter().filter_map(|id| config.places.iter().find(|p| &p.id == id))
    }

    /// The list's row `i`.
    pub fn row<'a>(&self, config: &'a Config, here: &'a str, i: usize) -> Option<Row<'a>> {
        let places = children(config, here).map(Row::Place);
        let made = made(config, here).map(Row::Made);
        places.chain(made).chain(std::iter::once(Row::New)).nth(i)
    }

    /// How many rows the list has.
    pub fn rows(&self, config: &Config, here: &str) -> usize {
        children(config, here).count() + made(config, here).count() + 1
    }

    /// The place selected: the row's, or else the tab or folder open.
    pub fn selected<'a>(&'a self, config: &'a Config) -> Option<&'a Place> {
        let here = self.here(config)?;
        match self.row(config, here, self.cursor) {
            Some(Row::Place(p)) => Some(p),
            _ => config.places.iter().find(|p| p.id == here),
        }
    }

    /// A tap on one of the edit mode's targets, with library.json as `read`. Returns whether it leaves.
    pub(crate) fn tap(&mut self, id: EditId, read: Option<&Arc<Config>>, out: &mut Out) -> bool {
        let Some(config) = self.config(read).cloned() else {
            return matches!(id, EditId::Done);
        };
        let here = self.here(&config).map(str::to_owned);
        if self.panel != Panel::Places || self.form.is_some() {
            if let Some(leave) = self.list_tap(id, &config, out) {
                return leave;
            }
            return false;
        }
        match id {
            EditId::Done => return true,
            EditId::Sources | EditId::Runners => self.show_panel(id),
            EditId::Tab(i) => {
                (self.tab, self.cursor) = (usize::from(i), 0);
                self.open.clear();
            }
            EditId::Up => {
                if let Some(left) = self.open.pop() {
                    let here = self.here(&config).map(str::to_owned).unwrap_or_default();
                    self.cursor = children(&config, &here).position(|p| p.id == left).unwrap_or(0);
                }
            }
            EditId::Row(i) => {
                let i = usize::from(i);
                match here.as_deref().and_then(|h| self.row(&config, h, i)) {
                    // A second tap on the selected folder opens it.
                    Some(Row::Place(p)) if self.cursor == i => {
                        self.open.push(p.id.clone());
                        self.cursor = 0;
                    }
                    Some(Row::New) => out.events.push(name_editor(PlaceName::New(here.clone()))),
                    _ => self.cursor = i,
                }
            }
            EditId::NewTab => out.events.push(name_editor(PlaceName::New(None))),
            EditId::Rename => {
                if let Some(p) = self.selected(&config) {
                    out.events.push(Event::EditOnKeyboard(Editor { purpose: Purpose::PlaceName(PlaceName::Rename(p.id.clone())), text: p.name.clone() }));
                }
            }
            EditId::Cut => self.cut = self.selected(&config).map(|p| p.id.clone()),
            EditId::Paste => {
                let Some(cut) = self.cut.take() else { return false };
                // Not into itself or a place in it.
                let inside =
                    |id: &str| std::iter::successors(Some(id.to_owned()), |at| config.places.iter().find(|p| &p.id == at)?.within.clone()).any(|at| at == cut);
                if here.as_deref().is_some_and(inside) {
                    return false;
                }
                let mut changed = config.clone();
                if let Some(p) = changed.places.iter_mut().find(|p| p.id == cut) {
                    // Pasted on a tab's top, it goes in that tab; in an open folder, in it.
                    p.within.clone_from(&here);
                }
                self.write(changed, out);
            }
            EditId::Open | EditId::MoveUp | EditId::MoveDown | EditId::Field(_) | EditId::Choice(_) | EditId::Save | EditId::Cancel => {}
            EditId::Remove => {
                let Some(id) = self.selected(&config).map(|p| p.id.clone()) else { return false };
                if used(&config, &id) || config.places.iter().any(|p| p.id == id && p.kind.is_some()) {
                    return false;
                }
                let mut changed = config.clone();
                changed.places.retain(|p| p.id != id);
                self.cursor = self.cursor.saturating_sub(1);
                self.write(changed, out);
            }
        }
        false
    }

    /// Shows the Sources or Runners panel.
    fn show_panel(&mut self, id: EditId) {
        self.panel = if id == EditId::Sources { Panel::Sources } else { Panel::Runners };
        (self.cursor, self.form) = (0, None);
    }

    /// A tap on the Sources or Runners panel, or on the form open. Returns whether it leaves the edit
    /// mode, or none if the tap isn't the panel's.
    fn list_tap(&mut self, id: EditId, config: &Config, out: &mut Out) -> Option<bool> {
        let len = self.listed(config);
        let sources = self.panel == Panel::Sources;
        match id {
            EditId::Done => return Some(true),
            EditId::Tab(i) => {
                (self.panel, self.form, self.tab, self.cursor) = (Panel::Places, None, usize::from(i), 0);
                self.open.clear();
            }
            EditId::Sources | EditId::Runners => self.show_panel(id),
            EditId::Row(i) if self.form.is_none() => {
                let i = usize::from(i);
                if i == self.cursor || i + 1 == len {
                    self.cursor = i;
                    self.open_form(config);
                } else {
                    self.cursor = i;
                }
            }
            EditId::Open if self.form.is_none() => self.open_form(config),
            EditId::MoveUp | EditId::MoveDown if self.form.is_none() && self.cursor + 1 < len => {
                let to = if id == EditId::MoveUp { self.cursor.checked_sub(1)? } else { self.cursor + 1 };
                if to + 1 >= len {
                    return Some(false);
                }
                let mut changed = config.clone();
                if sources {
                    changed.sources.swap(self.cursor, to);
                } else {
                    changed.runners.swap(self.cursor, to);
                }
                self.cursor = to;
                self.write(changed, out);
            }
            EditId::Remove if self.form.is_none() && self.cursor + 1 < len => {
                let mut changed = config.clone();
                if sources {
                    changed.sources.remove(self.cursor);
                } else {
                    // A runner a source names stays: the source would open nothing.
                    let named = changed.runners.get(self.cursor).map(|r| r.id.clone()).unwrap_or_default();
                    if runner_used(config, &named) {
                        return Some(false);
                    }
                    changed.runners.remove(self.cursor);
                }
                self.cursor = self.cursor.min(len.saturating_sub(2));
                self.write(changed, out);
            }
            EditId::Field(i) => {
                let form = self.form.as_mut()?;
                let i = usize::from(i);
                let field = *form.item.fields().get(i)?;
                match field.set() {
                    Set::Typed => {
                        form.field = i;
                        let text = form.item.value(config, field);
                        out.events.push(Event::EditOnKeyboard(Editor { purpose: Purpose::FormField(field), text }));
                    }
                    Set::Chosen => (form.field, form.choosing) = (i, !(form.choosing && form.field == i)),
                    Set::Toggled => form.item.toggle(field),
                }
            }
            EditId::Choice(j) => {
                let form = self.form.as_mut()?;
                let field = *form.item.fields().get(form.field)?;
                let (value, _) = form.item.options(config, field).into_iter().nth(usize::from(j))?;
                form.item.choose(field, &value);
                form.choosing = false;
            }
            EditId::Cancel => self.form = None,
            EditId::Save => {
                let form = self.form.take()?;
                let mut changed = config.clone();
                match (form.item, form.index) {
                    (Item::Source(s), Some(i)) => *changed.sources.get_mut(i)? = s,
                    (Item::Source(s), None) => changed.sources.push(s),
                    (Item::Runner(r), Some(i)) => *changed.runners.get_mut(i)? = r,
                    (Item::Runner(r), None) => changed.runners.push(r),
                }
                self.write(changed, out);
            }
            _ => return None,
        }
        Some(false)
    }

    /// Opens the form of the source or runner selected, or a new one's on the last row. A new scan goes
    /// in the tab or folder last open.
    fn open_form(&mut self, config: &Config) {
        let (index, item) = match self.panel {
            Panel::Sources => match config.sources.get(self.cursor) {
                Some(s) => (Some(self.cursor), Item::Source(s.clone())),
                None => {
                    let place = self.here(config).unwrap_or_default().to_owned();
                    (None, Item::Source(Source::Dir(Scan { place, ..Scan::default() })))
                }
            },
            Panel::Runners => match config.runners.get(self.cursor) {
                Some(r) => (Some(self.cursor), Item::Runner(r.clone())),
                None => (None, Item::Runner(Runner::default())),
            },
            Panel::Places => return,
        };
        self.form = Some(Form { index, item, field: 0, choosing: false });
    }

    /// A field of the form open, typed.
    pub(crate) fn field_typed(&mut self, field: Field, text: &str) {
        if let Some(form) = &mut self.form {
            form.item.type_in(field, text);
        }
    }

    /// A name typed for `what`.
    pub(crate) fn named(&mut self, what: PlaceName, name: &str, read: Option<&Arc<Config>>, out: &mut Out) {
        let name = name.trim();
        let Some(mut changed) = self.config(read).cloned().filter(|_| !name.is_empty()) else {
            return;
        };
        match what {
            PlaceName::Rename(id) => {
                let Some(p) = changed.places.iter_mut().find(|p| p.id == id) else { return };
                name.clone_into(&mut p.name);
            }
            PlaceName::New(within) => {
                let id = new_id(&changed, name);
                // A new tab goes before ClamShell's Tools, a new folder after its siblings.
                let at = match &within {
                    None => changed.places.iter().position(|p| p.kind == Some(clamshell_library::config::OwnTab::Tools)).unwrap_or(changed.places.len()),
                    Some(_) => changed.places.len(),
                };
                changed.places.insert(at, Place { id, name: name.to_owned(), within, ..Place::default() });
            }
        }
        self.write(changed, out);
    }

    /// Writes `changed`, which shows until the library is read again.
    fn write(&mut self, changed: Config, out: &mut Out) {
        let changed = Arc::new(changed);
        out.effects.push(Effect::WriteLibrary(changed.clone()));
        self.pending = Some(changed);
    }
}

/// Whether a source names runner `id`.
pub fn runner_used(config: &Config, id: &str) -> bool {
    config.sources.iter().any(|s| match s {
        Source::Dir(d) => d.runner.as_deref() == Some(id),
        Source::Desktop(r) | Source::Steam(r) => r.runner.as_deref() == Some(id),
        _ => false,
    })
}

/// The editor for a place's name, empty.
fn name_editor(what: PlaceName) -> Event {
    Event::EditOnKeyboard(Editor { purpose: Purpose::PlaceName(what), text: String::new() })
}

impl From<EditId> for Id {
    fn from(id: EditId) -> Id {
        Id::LibraryView(LibraryViewId::Edit(id))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clamshell_library::config::BrowseRoot;

    fn config() -> Arc<Config> {
        let place = |id: &str, name: &str, within: Option<&str>| Place { id: id.into(), name: name.into(), within: within.map(Into::into), ..Place::default() };
        Arc::new(Config {
            places: vec![
                place("games", "Games", None),
                place("snes", "SNES", Some("games")),
                place("rpg", "RPG", Some("snes")),
                place("files", "Files", None),
                Place { kind: Some(clamshell_library::config::OwnTab::Tools), ..place("tools", "Tools", None) },
            ],
            sources: vec![Source::Browse(BrowseRoot { dir: "~".into(), within: "files".into(), name: "Home".into(), trash: false })],
            ..Config::default()
        })
    }

    fn written(out: &Out) -> Arc<Config> {
        out.effects.iter().find_map(|e| if let Effect::WriteLibrary(c) = e { Some(c.clone()) } else { None }).unwrap()
    }

    #[test]
    fn a_tab_lists_its_folders_those_sources_make_and_a_new_row() {
        let read = config();
        let mut edit = Edit::default();
        let c = edit.config(Some(&read)).unwrap();
        assert_eq!(tabs(c).map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Games", "Files", "Tools"]);
        assert_eq!(edit.rows(c, "games"), 2);
        assert_eq!(edit.row(c, "files", 0), Some(Row::Made("Home")));
        assert_eq!(edit.row(c, "files", 1), Some(Row::New));
        let mut out = Out::default();
        edit.tap(EditId::Row(0), Some(&read), &mut out);
        assert_eq!(edit.here(&read), Some("snes"), "a tap on the selected row opens it");
        edit.tap(EditId::Up, Some(&read), &mut out);
        assert_eq!((edit.here(&read), edit.cursor()), (Some("games"), 0));
    }

    /// A rename, a new folder and a new tab are written, and show at once, until the library is read.
    #[test]
    fn names_are_written_and_show_at_once() {
        let read = config();
        let mut edit = Edit::default();
        let mut out = Out::default();
        edit.named(PlaceName::Rename("snes".into()), " Super NES ", Some(&read), &mut out);
        assert_eq!(written(&out).places.iter().find(|p| p.id == "snes").unwrap().name, "Super NES");
        edit.named(PlaceName::New(Some("games".into())), "Game Cube", Some(&read), &mut out);
        edit.named(PlaceName::New(None), "Music", Some(&read), &mut out);
        let c = edit.config(Some(&read)).unwrap();
        assert!(c.places.iter().any(|p| p.id == "game-cube" && p.within.as_deref() == Some("games")));
        assert_eq!(tabs(c).map(|p| p.id.as_str()).collect::<Vec<_>>(), ["games", "files", "music", "tools"], "before Tools");
        edit.named(PlaceName::New(None), "Music", Some(&read), &mut out);
        assert!(edit.config(Some(&read)).unwrap().places.iter().any(|p| p.id == "music-2"), "ids stay unique");
        edit.library_read();
        assert_eq!(edit.config(Some(&read)).unwrap(), &*read, "what was read shows once it comes");
    }

    /// A place moves by Cut and Paste, not into itself; only a place nothing uses is removed.
    #[test]
    fn places_move_and_unused_ones_go() {
        let read = config();
        let mut edit = Edit::default();
        let mut out = Out::default();
        edit.tap(EditId::Cut, Some(&read), &mut out);
        assert_eq!(edit.cut(), Some("snes"), "the row selected");
        edit.tap(EditId::Row(0), Some(&read), &mut out);
        edit.tap(EditId::Paste, Some(&read), &mut out);
        assert!(out.effects.is_empty(), "not into itself");
        edit.tap(EditId::Tab(1), Some(&read), &mut out);
        edit.tap(EditId::Cut, Some(&read), &mut out);
        assert_eq!(edit.cut(), Some("files"), "nothing selected: the tab itself");
        edit.cut = Some("rpg".into());
        edit.tap(EditId::Paste, Some(&read), &mut out);
        assert_eq!(written(&out).places.iter().find(|p| p.id == "rpg").unwrap().within.as_deref(), Some("files"));
        // Removing, from what was read.
        let (mut edit, mut out) = (Edit::default(), Out::default());
        edit.tap(EditId::Remove, Some(&read), &mut out);
        assert!(out.effects.is_empty(), "SNES has RPG in it");
        edit.tap(EditId::Tab(1), Some(&read), &mut out);
        edit.tap(EditId::Remove, Some(&read), &mut out);
        assert!(out.effects.is_empty(), "Files has a browsed folder");
        edit.tap(EditId::Tab(0), Some(&read), &mut out);
        edit.tap(EditId::Row(0), Some(&read), &mut out);
        edit.tap(EditId::Remove, Some(&read), &mut out);
        assert!(!written(&out).places.iter().any(|p| p.id == "rpg"), "RPG is unused");
    }

    /// A source's form: a field typed, a choice made, saved and written in its place; a new source goes
    /// at the end; a source moves down; a runner a source names isn't removed.
    #[test]
    fn sources_and_runners_are_edited_in_forms() {
        use clamshell_library::config::{Runner, Scan};
        let mut read = (*config()).clone();
        read.runners.push(Runner { id: "snes".into(), name: "SNES".into(), ..Runner::default() });
        read.sources.insert(
            0,
            Source::Dir(Scan { id: "snes".into(), dir: "~/Roms/snes".into(), place: "snes".into(), runner: Some("snes".into()), ..Scan::default() }),
        );
        let read = Arc::new(read);
        let mut edit = Edit::default();
        let mut out = Out::default();
        edit.tap(EditId::Sources, Some(&read), &mut out);
        edit.tap(EditId::Open, Some(&read), &mut out);
        let fields = edit.form().unwrap().item.fields();
        let at = |f: Field| fields.iter().position(|x| *x == f).unwrap() as u16;
        edit.tap(EditId::Field(at(Field::Also)), Some(&read), &mut out);
        assert!(
            out.events.iter().any(|e| matches!(e, Event::EditOnKeyboard(Editor { purpose: Purpose::FormField(Field::Also), .. }))),
            "typed on the keyboard"
        );
        edit.field_typed(Field::Also, "*.zip");
        edit.tap(EditId::Field(at(Field::Place)), Some(&read), &mut out);
        assert!(edit.form().unwrap().choosing, "its options show");
        let games = edit.form().unwrap().item.options(&read, Field::Place).iter().position(|(v, _)| v == "games").unwrap() as u16;
        edit.tap(EditId::Choice(games), Some(&read), &mut out);
        edit.tap(EditId::Save, Some(&read), &mut out);
        let c = written(&out);
        assert!(matches!(c.sources.first(), Some(Source::Dir(d)) if d.also == ["*.zip"] && d.place == "games"), "{:?}", c.sources.first());
        edit.library_read();
        // A new one, on the last row.
        let mut out = Out::default();
        let last = edit.listed(&read) as u16 - 1;
        edit.tap(EditId::Row(last), Some(&read), &mut out);
        assert_eq!(edit.form().unwrap().index, None);
        edit.tap(EditId::Save, Some(&read), &mut out);
        assert_eq!(written(&out).sources.len(), read.sources.len() + 1);
        edit.library_read();
        let mut out = Out::default();
        edit.tap(EditId::Row(0), Some(&read), &mut out);
        edit.tap(EditId::MoveDown, Some(&read), &mut out);
        assert!(matches!(written(&out).sources.get(1), Some(Source::Dir(d)) if d.id == "snes"));
        edit.library_read();
        let mut out = Out::default();
        edit.tap(EditId::Runners, Some(&read), &mut out);
        edit.tap(EditId::Remove, Some(&read), &mut out);
        assert!(out.effects.is_empty(), "SNES's scan names it");
    }
}
