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
//! Each change is made to library.json as it shows and written; the library is then read again. What
//! shows is the Library's ([`super::LibraryView::config_shown`]): what was written last, until every write
//! has been read back, so a change shows at once and the next is made on it.

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
    /// Chooses the form's field, by position, in the Library (see [`super::pick`]).
    Pick(u16),
    /// Shows the icons to choose the selected place's from, and chooses one of them ([`ICONS`]), the
    /// first being none.
    Icon,
    IconChoice(u16),
    /// Chooses the selected folder's picture in the Library (see [`super::pick`]).
    Picture,
}

/// The UI's icons that a place may have, by name (clamshell-ui's `Icon::named` draws each).
pub const ICONS: [&str; 13] = ["gamepad", "apps", "film", "tv", "music", "star", "clock", "tools", "folder", "archive", "file", "trash", "card"];

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
    /// Which form opened it is, counted, so that a field typed for it goes to it and no other.
    pub opened: u32,
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
    /// The icons show, to choose the selected place's from.
    choosing_icon: bool,
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
    /// Why the last thing asked wasn't done (a name taken, a paste into itself), until something is.
    problem: Option<&'static str>,
    /// How many forms have been opened (see [`Form::opened`]).
    forms: u32,
}

/// Why place name `name` can't be one in `within` (a tab's if none), `id` being the place named, if it
/// is there: empty, with a `/` (the Library finds places by their names' path), or the name of another
/// there, or of a folder the Files tab browses there.
pub fn name_problem(config: &Config, within: Option<&str>, name: &str, id: Option<&str>) -> Option<&'static str> {
    let name = name.trim();
    let same = |other: &str| other.trim().eq_ignore_ascii_case(name);
    if name.is_empty() {
        return Some("A name is needed");
    }
    if name.contains('/') {
        return Some("A name can't have a /");
    }
    let sibling = config.places.iter().any(|p| p.within.as_deref() == within && Some(p.id.as_str()) != id && same(&p.name));
    let browsed =
        config.sources.iter().any(|s| matches!(s, clamshell_library::config::Source::Browse(b) if Some(b.within.as_str()) == within && same(&b.name)));
    (sibling || browsed).then_some("Another there has that name")
}

/// The tabs: the places in no other, in their order.
pub fn tabs(config: &Config) -> impl Iterator<Item = &Place> + Clone {
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

/// library.json as the edit mode shows and changes it (see [`super::LibraryView::config_shown`]): as read,
/// or as last written while a write hasn't been read back. A write is noted as it is made, so the next
/// edit is made on it.
pub(crate) struct ConfigShown<'a> {
    pub(crate) read: Option<&'a Arc<Config>>,
    /// The last write, and how many writes haven't been read back.
    pub(crate) written: &'a mut Option<(Arc<Config>, u32)>,
}

impl ConfigShown<'_> {
    pub(crate) fn get(&self) -> Option<&Arc<Config>> {
        self.written.as_ref().map(|(c, _)| c).or(self.read)
    }

    /// Writes `changed`, which shows until it is read back.
    pub(crate) fn write(&mut self, changed: Config, out: &mut Out) {
        let changed = Arc::new(changed);
        out.effects.push(Effect::WriteLibrary(changed.clone()));
        let unread = self.written.as_ref().map_or(0, |(_, n)| *n) + 1;
        *self.written = Some((changed, unread));
    }
}

impl Edit {
    pub fn tab(&self) -> usize {
        self.tab
    }

    pub fn panel(&self) -> Panel {
        self.panel
    }

    pub fn choosing_icon(&self) -> bool {
        self.choosing_icon
    }

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    /// How many rows the sources' or runners' list has: one each, and one to make a new one.
    /// How many rows its list has: the open folder's, or the sources' or runners'.
    pub fn list_rows(&self, config: &Config) -> usize {
        match self.panel {
            Panel::Places => self.here(config).map_or(0, |here| self.rows(config, here)),
            Panel::Sources | Panel::Runners => self.listed(config),
        }
    }

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
        self.open(config).last().map(|p| p.id.as_str()).or_else(|| tabs(config).nth(self.tab).map(|p| p.id.as_str()))
    }

    /// The open folders, outermost first: those opened that are there still, up to the first that isn't
    /// (removed, or moved out), so a folder gone takes the edit mode back out of it.
    pub fn open<'a>(&'a self, config: &'a Config) -> impl Iterator<Item = &'a Place> + 'a {
        self.open.iter().map_while(|id| config.places.iter().find(|p| &p.id == id))
    }

    /// Why the last thing asked wasn't done, until something is.
    pub fn problem(&self) -> Option<&'static str> {
        self.problem
    }

    /// Why the selected place can't be cut to be moved: ClamShell's own tabs stay where they are.
    pub fn cut_problem(&self, config: &Config) -> Option<&'static str> {
        self.selected(config).filter(|p| p.kind.is_some()).map(|_| "ClamShell's own tabs stay")
    }

    /// Why the place cut can't be pasted here: not into itself or a folder in it, nor next to one of its
    /// name.
    pub fn paste_problem(&self, config: &Config) -> Option<&'static str> {
        let cut = config.places.iter().find(|p| Some(&p.id) == self.cut.as_ref())?;
        let here = self.here(config);
        let inside =
            |id: &str| std::iter::successors(Some(id.to_owned()), |at| config.places.iter().find(|p| &p.id == at)?.within.clone()).any(|at| at == cut.id);
        if here.is_some_and(inside) {
            return Some("Not into itself");
        }
        name_problem(config, here, &cut.name, Some(&cut.id))
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

    /// A tap on one of the edit mode's targets, with library.json as `shown`. Returns whether it leaves.
    pub(crate) fn tap(&mut self, id: EditId, shown: &mut ConfigShown<'_>, out: &mut Out) -> bool {
        let Some(config) = shown.get().map(|c| &**c).cloned() else {
            return matches!(id, EditId::Done);
        };
        // A new ask: the last one's problem is gone, and the folders opened are those still there.
        self.problem = None;
        let open = self.open(&config).count();
        self.open.truncate(open);
        let here = self.here(&config).map(str::to_owned);
        if self.panel != Panel::Places || self.form.is_some() {
            if let Some(leave) = self.list_tap(id, &config, shown, out) {
                return leave;
            }
            return false;
        }
        match id {
            EditId::Done => return true,
            EditId::Sources | EditId::Runners => self.show_panel(id),
            EditId::Tab(i) => {
                (self.tab, self.cursor, self.choosing_icon) = (usize::from(i), 0, false);
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
            EditId::Cut => {
                self.problem = self.cut_problem(&config);
                if self.problem.is_none() {
                    self.cut = self.selected(&config).map(|p| p.id.clone());
                }
            }
            EditId::Paste => {
                // Refused, the cut stays, to be pasted elsewhere.
                self.problem = self.paste_problem(&config);
                if self.problem.is_some() {
                    return false;
                }
                let Some(cut) = self.cut.take() else { return false };
                let mut changed = config.clone();
                if let Some(p) = changed.places.iter_mut().find(|p| p.id == cut) {
                    // Pasted on a tab's top, it goes in that tab; in an open folder, in it.
                    p.within.clone_from(&here);
                }
                shown.write(changed, out);
            }
            EditId::Open
            | EditId::MoveUp
            | EditId::MoveDown
            | EditId::Field(_)
            | EditId::Choice(_)
            | EditId::Save
            | EditId::Cancel
            | EditId::Pick(_)
            | EditId::Picture => {}
            EditId::Icon => self.choosing_icon = !self.choosing_icon,
            EditId::IconChoice(j) => {
                let Some(id) = self.selected(&config).map(|p| p.id.clone()) else { return false };
                let mut changed = config.clone();
                let Some(p) = changed.places.iter_mut().find(|p| p.id == id) else { return false };
                p.icon = ICONS.get(usize::from(j).wrapping_sub(1)).map(|i| (*i).to_owned());
                self.choosing_icon = false;
                shown.write(changed, out);
            }
            EditId::Remove => {
                let Some(id) = self.selected(&config).map(|p| p.id.clone()) else { return false };
                if used(&config, &id) || config.places.iter().any(|p| p.id == id && p.kind.is_some()) {
                    return false;
                }
                let mut changed = config.clone();
                changed.places.retain(|p| p.id != id);
                self.cursor = self.cursor.saturating_sub(1);
                shown.write(changed, out);
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
    fn list_tap(&mut self, id: EditId, config: &Config, shown: &mut ConfigShown<'_>, out: &mut Out) -> Option<bool> {
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
                shown.write(changed, out);
            }
            EditId::Remove if self.form.is_none() && self.cursor + 1 < len => {
                let mut changed = config.clone();
                if sources {
                    changed.sources.remove(self.cursor);
                } else {
                    // A runner a source names stays: the source would open nothing.
                    let named = changed.runners.get(self.cursor).map(Runner::effective_id).unwrap_or_default();
                    if runner_used(config, &named) {
                        return Some(false);
                    }
                    changed.runners.remove(self.cursor);
                }
                self.cursor = self.cursor.min(len.saturating_sub(2));
                shown.write(changed, out);
            }
            EditId::Field(i) => {
                let form = self.form.as_mut()?;
                let i = usize::from(i);
                let field = *form.item.fields().get(i)?;
                match field.set() {
                    Set::Typed => {
                        form.field = i;
                        let text = form.item.value(config, field);
                        out.events.push(Event::EditOnKeyboard(Editor { purpose: Purpose::FormField(field, form.opened), text }));
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
                match saved(config, form.item.clone(), form.index) {
                    Ok(changed) => shown.write(changed, out),
                    // Not saved: the form stays, saying why.
                    Err(why) => {
                        self.problem = Some(why);
                        self.form = Some(form);
                    }
                }
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
        self.forms += 1;
        self.form = Some(Form { opened: self.forms, index, item, field: 0, choosing: false });
    }

    /// Selects the form's field `i`.
    pub(crate) fn select_field(&mut self, i: usize) {
        if let Some(form) = &mut self.form {
            (form.field, form.choosing) = (i, false);
        }
    }

    /// A field of the form open, picked in the Library.
    pub(crate) fn field_picked(&mut self, field: Field, path: &str) {
        if let Some(form) = &mut self.form {
            form.item.picked(field, path);
        }
    }

    /// A field of the form open, typed.
    /// Returns whether it was: the form it was typed for is gone, it isn't.
    pub(crate) fn field_typed(&mut self, field: Field, opened: u32, text: &str) -> bool {
        match &mut self.form {
            Some(form) if form.opened == opened => {
                form.item.type_in(field, text);
                true
            }
            _ => false,
        }
    }

    /// Place `id`'s picture, chosen: written.
    pub(crate) fn set_picture(&mut self, id: &str, path: &str, shown: &mut ConfigShown<'_>, out: &mut Out) {
        let Some(mut changed) = shown.get().map(|c| &**c).cloned() else { return };
        let Some(p) = changed.places.iter_mut().find(|p| p.id == id) else { return };
        p.picture = Some(path.to_owned());
        shown.write(changed, out);
    }

    /// A name typed for `what`.
    pub(crate) fn named(&mut self, what: PlaceName, name: &str, shown: &mut ConfigShown<'_>, out: &mut Out) {
        let name = name.trim();
        let Some(mut changed) = shown.get().map(|c| &**c).cloned() else { return };
        let (within, id) = match &what {
            PlaceName::Rename(id) => (changed.places.iter().find(|p| &p.id == id).and_then(|p| p.within.clone()), Some(id.as_str())),
            PlaceName::New(within) => (within.clone(), None),
        };
        self.problem = name_problem(&changed, within.as_deref(), name, id);
        if self.problem.is_some() {
            return;
        }
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
        shown.write(changed, out);
    }
}

/// library.json with `item` saved at `index` (a new one's at the end), or why it can't be. A scan
/// without an id gets one from its folder, other scans' ids kept apart, since its entries are known by it.
/// A runner's id (or, without one, its name's) must be its own; changed, the sources naming the old one
/// name the new.
fn saved(config: &Config, item: Item, index: Option<usize>) -> Result<Config, &'static str> {
    let mut changed = config.clone();
    match item {
        Item::Source(mut s) => {
            if let Source::Dir(scan) = &mut s {
                let taken = |id: &str| changed.sources.iter().enumerate().any(|(i, o)| Some(i) != index && matches!(o, Source::Dir(d) if d.id == id));
                if scan.id.is_empty() || taken(&scan.id) {
                    let base = scan.dir.trim_end_matches('/').rsplit('/').next().filter(|b| !b.is_empty()).unwrap_or("scan").to_lowercase();
                    scan.id = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| !taken(id)).unwrap_or_default();
                }
            }
            match index {
                Some(i) => *changed.sources.get_mut(i).ok_or("It's gone")? = s,
                None => changed.sources.push(s),
            }
        }
        Item::Runner(mut r) => {
            let id = r.effective_id();
            if id.is_empty() {
                return Err("An id or a name is needed");
            }
            // Written with its id, made from its name if it had none.
            r.id.clone_from(&id);
            if changed.runners.iter().enumerate().any(|(i, o)| Some(i) != index && o.effective_id() == id) {
                return Err("Another runner has that id");
            }
            let old = index.and_then(|i| changed.runners.get(i)).map(Runner::effective_id);
            if let Some(old) = old.filter(|old| *old != id) {
                for s in &mut changed.sources {
                    let runner = match s {
                        Source::Dir(d) => &mut d.runner,
                        Source::Desktop(rule) | Source::Steam(rule) => &mut rule.runner,
                        _ => continue,
                    };
                    if runner.as_deref() == Some(old.as_str()) {
                        *runner = Some(id.clone());
                    }
                }
            }
            match index {
                Some(i) => *changed.runners.get_mut(i).ok_or("It's gone")? = r,
                None => changed.runners.push(r),
            }
        }
    }
    Ok(changed)
}

/// Whether a source names `runner`, by its id or, without one, its name's (see
/// [`clamshell_library::config::Runner::is_named`]).
pub fn runner_in_use(config: &Config, runner: &Runner) -> bool {
    config.sources.iter().any(|s| match s {
        Source::Dir(d) => d.runner.as_deref().is_some_and(|id| runner.is_named(id)),
        Source::Desktop(r) | Source::Steam(r) => r.runner.as_deref().is_some_and(|id| runner.is_named(id)),
        _ => false,
    })
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
        let c = &*read;
        assert_eq!(tabs(c).map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Games", "Files", "Tools"]);
        assert_eq!(edit.rows(c, "games"), 2);
        assert_eq!(edit.row(c, "files", 0), Some(Row::Made("Home")));
        assert_eq!(edit.row(c, "files", 1), Some(Row::New));
        let mut out = Out::default();
        edit.tap(EditId::Row(0), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(edit.here(&read), Some("snes"), "a tap on the selected row opens it");
        edit.tap(EditId::Up, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!((edit.here(&read), edit.cursor()), (Some("games"), 0));
    }

    /// A rename, a new folder and a new tab are written, each made on the one before as it shows (the
    /// Library shows what was written, see `LibraryView::config_shown`).
    #[test]
    fn names_are_written_each_on_the_last() {
        let read = config();
        let mut written = None;
        let mut shown = read.clone();
        let mut edit = Edit::default();
        let mut out = Out::default();
        let mut name = |edit: &mut Edit, what, text: &str, shown: &mut Arc<Config>| {
            edit.named(what, text, &mut ConfigShown { read: Some(&read), written: &mut written }, &mut out);
            *shown = written.as_ref().unwrap().0.clone();
        };
        name(&mut edit, PlaceName::Rename("snes".into()), " Super NES ", &mut shown);
        assert_eq!(shown.places.iter().find(|p| p.id == "snes").unwrap().name, "Super NES");
        name(&mut edit, PlaceName::New(Some("games".into())), "Game Cube", &mut shown);
        name(&mut edit, PlaceName::New(None), "Music", &mut shown);
        assert!(shown.places.iter().any(|p| p.id == "game-cube" && p.within.as_deref() == Some("games")));
        assert!(shown.places.iter().any(|p| p.name == "Super NES"), "the rename kept");
        assert_eq!(tabs(&shown).map(|p| p.id.as_str()).collect::<Vec<_>>(), ["games", "files", "music", "tools"], "before Tools");
        // Renamed, it keeps its id: a new Music's is another.
        name(&mut edit, PlaceName::Rename("music".into()), "Songs", &mut shown);
        name(&mut edit, PlaceName::New(None), "Music", &mut shown);
        assert!(shown.places.iter().any(|p| p.id == "music-2"), "ids stay unique");
    }

    /// A place moves by Cut and Paste, not into itself; only a place nothing uses is removed.
    #[test]
    fn places_move_and_unused_ones_go() {
        let read = config();
        let mut edit = Edit::default();
        let mut out = Out::default();
        edit.tap(EditId::Cut, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(edit.cut(), Some("snes"), "the row selected");
        edit.tap(EditId::Row(0), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Paste, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(out.effects.is_empty(), "not into itself");
        assert_eq!((edit.problem(), edit.cut()), (Some("Not into itself"), Some("snes")), "said, and the cut kept");
        edit.tap(EditId::Tab(1), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Cut, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(edit.cut(), Some("files"), "nothing selected: the tab itself");
        edit.cut = Some("rpg".into());
        edit.tap(EditId::Paste, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(written(&out).places.iter().find(|p| p.id == "rpg").unwrap().within.as_deref(), Some("files"));
        // Removing, from what was read.
        let (mut edit, mut out) = (Edit::default(), Out::default());
        edit.tap(EditId::Remove, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(out.effects.is_empty(), "SNES has RPG in it");
        edit.tap(EditId::Tab(1), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Remove, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(out.effects.is_empty(), "Files has a browsed folder");
        edit.tap(EditId::Tab(0), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Row(0), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Remove, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(!written(&out).places.iter().any(|p| p.id == "rpg"), "RPG is unused");
    }

    /// ClamShell's own tabs aren't cut; a name with a `/`, or one another there has (a place, or a folder
    /// the Files tab browses), isn't given, and why shows; a folder removed while open is left.
    #[test]
    fn places_keep_names_of_their_own_and_own_tabs_stay() {
        let read = config();
        let (mut edit, mut out) = (Edit::default(), Out::default());
        edit.tap(EditId::Tab(2), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Cut, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!((edit.cut(), edit.problem()), (None, Some("ClamShell's own tabs stay")));
        for (what, name) in [
            (PlaceName::Rename("snes".into()), "PS1/PSX"),
            (PlaceName::New(None), "tools"),
            (PlaceName::New(Some("files".into())), "Home"),
            (PlaceName::New(Some("games".into())), "snes"),
        ] {
            edit.named(what, name, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
            assert!(out.effects.is_empty() && edit.problem().is_some(), "{name}");
        }
        edit.named(PlaceName::Rename("snes".into()), "Snes", &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(!out.effects.is_empty(), "its own name, cased otherwise");
        // In SNES, which goes: back out to Games.
        let (mut edit, mut out) = (Edit::default(), Out::default());
        edit.tap(EditId::Row(0), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(edit.here(&read), Some("snes"));
        let mut gone = (*read).clone();
        gone.places.retain(|p| p.id != "snes" && p.id != "rpg");
        assert_eq!(edit.here(&gone), Some("games"));
    }

    /// Save keeps ids apart: a scan without one gets one from its folder, not another scan's; a runner's
    /// must be its own, and changed, the sources naming it follow; a runner known by its name is found used.
    #[test]
    fn saving_keeps_ids_apart() {
        let mut config = (*config()).clone();
        let scan =
            |id: &str, dir: &str| Source::Dir(Scan { id: id.into(), dir: dir.into(), place: "snes".into(), runner: Some("snes".into()), ..Scan::default() });
        config.sources = vec![scan("snes", "~/Roms/snes")];
        config.runners = vec![Runner { id: "snes".into(), ..Runner::default() }, Runner { name: "Game Boy".into(), ..Runner::default() }];
        let c = saved(&config, Item::Source(scan("", "~/Roms/snes/")), None).unwrap();
        assert!(matches!(c.sources.get(1), Some(Source::Dir(d)) if d.id == "snes-2"), "{:?}", c.sources.get(1));
        assert_eq!(saved(&config, Item::Runner(Runner { id: "game-boy".into(), ..Runner::default() }), Some(0)).err(), Some("Another runner has that id"));
        let c = saved(&config, Item::Runner(Runner { id: "sfc".into(), ..Runner::default() }), Some(0)).unwrap();
        assert!(matches!(c.sources.first(), Some(Source::Dir(d)) if d.runner.as_deref() == Some("sfc")), "the source follows");
        config.sources = vec![Source::Dir(Scan { runner: Some("game-boy".into()), ..Scan::default() })];
        assert!(runner_used(&config, &config.runners.get(1).unwrap().effective_id()), "known by its name");
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
        edit.tap(EditId::Sources, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Open, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        let fields = edit.form().unwrap().item.fields();
        let at = |f: Field| fields.iter().position(|x| *x == f).unwrap() as u16;
        edit.tap(EditId::Field(at(Field::Also)), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(
            out.events.iter().any(|e| matches!(e, Event::EditOnKeyboard(Editor { purpose: Purpose::FormField(Field::Also, _), .. }))),
            "typed on the keyboard"
        );
        let opened = edit.form().unwrap().opened;
        assert!(edit.field_typed(Field::Also, opened, "*.zip"));
        assert!(!edit.field_typed(Field::Also, opened + 1, "*.7z"), "for another form: not taken");
        edit.tap(EditId::Field(at(Field::Place)), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(edit.form().unwrap().choosing, "its options show");
        let games = edit.form().unwrap().item.options(&read, Field::Place).iter().position(|(v, _)| v == "games").unwrap() as u16;
        edit.tap(EditId::Choice(games), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Save, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        let c = written(&out);
        assert!(matches!(c.sources.first(), Some(Source::Dir(d)) if d.also == ["*.zip"] && d.place == "games"), "{:?}", c.sources.first());
        // A new one, on the last row.
        let mut out = Out::default();
        let last = edit.listed(&read) as u16 - 1;
        edit.tap(EditId::Row(last), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(edit.form().unwrap().index, None);
        edit.tap(EditId::Save, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert_eq!(written(&out).sources.len(), read.sources.len() + 1);
        let mut out = Out::default();
        edit.tap(EditId::Row(0), &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::MoveDown, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(matches!(written(&out).sources.get(1), Some(Source::Dir(d)) if d.id == "snes"));
        let mut out = Out::default();
        edit.tap(EditId::Runners, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        edit.tap(EditId::Remove, &mut ConfigShown { read: Some(&read), written: &mut None }, &mut out);
        assert!(out.effects.is_empty(), "SNES's scan names it");
    }
}
