//! The picker, shown over the settings tab, the same for every kind of profile ([`Kind`]):
//!
//! - It lists, for a game, its own profile or a row that makes one from the profile in force. Then come
//!   Default (no profile: the pad as it is, or for curves Off), the named profiles with how many games use each,
//!   and New.
//! - A profile's menu has Edit, Rename, Duplicate and Delete; deleting one in use asks first. A game's own
//!   profile stays while another is chosen, and is never deleted. Its menu has Edit, Load from, Save as
//!   and Reset. Load from copies Default or a named profile into it and opens its edit screen, where Undo
//!   goes back; Reset loads Default. Save as copies it to a new named profile, or over an existing one
//!   after asking.
//!
//! The kind's own screens, such as its edit screen, are its own; the picker opens them through the kind's
//! [`Owner`].

use std::sync::Arc;

use super::Profiles;
use crate::bottom::editor::{Editor, Editors, Purpose};
use crate::effect::Effect;
use crate::nav::lists::RowPart;
use crate::out::Out;
use crate::settings::Setting;
use crate::{Event, ListId};

/// A kind of profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Keymap,
    /// tone curves (see [`crate::curve`]).
    Curve,
}

impl Kind {
    /// The setting that holds the profile chosen for a game or for all games: a profile's id, 0 for
    /// Default.
    pub fn setting(self) -> Setting {
        match self {
            Kind::Keymap => Setting::Keymap,
            Kind::Curve => Setting::Curve,
        }
    }

    /// The kind whose setting `setting` is (the keymap's or the curve's).
    pub(crate) fn of(setting: Setting) -> Kind {
        if setting == Setting::Curve { Kind::Curve } else { Kind::Keymap }
    }

    /// The kind whose picker's list `list` is.
    pub(crate) fn of_list(list: ListId) -> Kind {
        if list == ListId::Curves { Kind::Curve } else { Kind::Keymap }
    }

    /// The picker's list.
    pub fn list(self) -> ListId {
        match self {
            Kind::Keymap => ListId::Keymaps,
            Kind::Curve => ListId::Curves,
        }
    }

    /// What a game's own profile is called.
    pub fn own_name(self) -> &'static str {
        match self {
            Kind::Keymap => "Own keymap",
            Kind::Curve => "Own curve",
        }
    }

    /// The word for one, as in "the new keymap".
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Keymap => "keymap",
            Kind::Curve => "curve",
        }
    }

    /// What no profile, id 0, is called.
    pub fn default_name(self) -> &'static str {
        match self {
            Kind::Keymap => "Default",
            Kind::Curve => "Default colours",
        }
    }
}

/// Which of the picker's screens is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerScreen {
    /// Default, the profiles, and New.
    List,
    /// A profile's long-press menu.
    Menu(u16),
    /// Asks whether to delete a profile that games use.
    Delete(u16),
    /// Default and the named profiles, to pick one to copy into this game's own profile.
    Load(u16),
    /// A new named profile and the named profiles, to pick what to save this game's own profile as.
    SaveAs(u16),
    /// Asks whether to save `own` over the named profile `to`.
    Overwrite { own: u16, to: u16 },
}

impl PickerScreen {
    /// The profile it is about, if any.
    pub fn profile(self) -> Option<u16> {
        match self {
            PickerScreen::List => None,
            PickerScreen::Menu(id) | PickerScreen::Delete(id) | PickerScreen::Load(id) | PickerScreen::SaveAs(id) => Some(id),
            PickerScreen::Overwrite { own, .. } => Some(own),
        }
    }
}

/// A row of the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerRow<'a> {
    /// The game's own profile by its id, or none for the row that makes it.
    Own(Option<u16>),
    /// When saving a game's own profile: save it as a new named profile.
    New,
    Default,
    /// A named profile: its id and name.
    Named(u16, &'a str),
    /// Back to the level below's choice, last, while the level edited chooses one itself (see
    /// [`crate::Panel::resettable`]).
    Reset,
}

/// Who uses a profile: whether all games do, as their default, and how many games have chosen it as their
/// own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Users {
    pub all_entries: bool,
    pub entries: usize,
}

impl Users {
    pub fn any(self) -> bool {
        self.all_entries || self.entries > 0
    }
}

/// What the picker reads from elsewhere: the game the settings panel edits, if any, and the profile chosen
/// for its level.
#[derive(Debug, Clone, Default)]
pub(crate) struct PickerCx {
    pub(crate) entry: Option<Arc<str>>,
    pub(crate) chosen: u16,
}

/// The picker's buttons. Its rows are [`crate::Id::Row`]s of the kind's list ([`Kind::list`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileId {
    /// Back one screen.
    Back,
    /// The picker's New.
    New,
    /// A profile's menu.
    Edit,
    Rename,
    Duplicate,
    Delete,
    /// Confirms deleting a profile that games use.
    ConfirmDelete,
    /// In the menu of a game's own profile: save it over a named profile, or as a new one.
    SaveAs,
    /// Confirms saving over a named profile.
    ConfirmSave,
    /// In the menu of a game's own profile: the profiles to load into it.
    LoadFrom,
    /// In the menu of a game's own profile: Default loaded into it.
    Reset,
}

/// What a profile's name, typed in the editor, is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileName {
    New,
    Rename(u16),
    /// A copy of this one.
    Duplicate(u16),
    /// A copy of a game's own profile, as a new named one.
    SaveAs(u16),
}

/// What the picker asks of the settings; State handles it for the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileEvent {
    /// Profile `id` is chosen for what the settings panel edits.
    Choose(u16),
    /// Profile `id` was made as the own profile of the game the settings panel edits. It is chosen for the
    /// game, and then the profiles are saved.
    MadeOwn(u16),
    /// Profile `id` was deleted. The games that used it, and all games if it was theirs, go back to
    /// Default.
    Deleted(u16),
}

/// What a kind of profile gives the picker: its profiles, where the picker stands among its screens, and
/// its edit screen and saving.
pub(crate) trait Owner {
    type Body: Clone + Default + PartialEq;
    const KIND: Kind;
    fn profiles(&self) -> &Profiles<Self::Body>;
    fn profiles_mut(&mut self) -> &mut Profiles<Self::Body>;
    /// The picker's screen open, if one of them is.
    fn picker(&self) -> Option<PickerScreen>;
    /// Opens one of the picker's screens, or with none closes the kind's screens.
    fn set_picker(&mut self, screen: Option<PickerScreen>);
    /// Opens profile `id`'s edit screen.
    fn open_edit(&mut self, id: u16);
    /// The effect that saves the profiles, the change counted for the edit screen's Undo.
    fn save(&mut self) -> Effect;
}

impl<B: Clone + Default + PartialEq> Profiles<B> {
    /// The picker's rows. Usually they are, for `entry`, its own profile or the row that makes one, then
    /// Default and the named profiles. When loading into a game's own profile they are Default and the
    /// named profiles; when saving it, a new profile and the named profiles.
    fn rows<'a>(&'a self, screen: Option<PickerScreen>, entry: Option<&str>) -> impl Iterator<Item = PickerRow<'a>> + use<'a, B> {
        let (loading, saving) = match screen {
            Some(PickerScreen::Load(_)) => (true, false),
            Some(PickerScreen::SaveAs(_)) => (false, true),
            _ => (false, false),
        };
        let own = entry.filter(|_| !loading && !saving).map(|g| PickerRow::Own(self.own_of(g).map(|p| p.id)));
        let new = saving.then_some(PickerRow::New);
        let default = (!saving).then_some(PickerRow::Default);
        own.into_iter().chain(new).chain(default).chain(self.named().map(|p| PickerRow::Named(p.id, p.name.as_str())))
    }

    pub(crate) fn rows_len(&self, screen: Option<PickerScreen>, entry: Option<&str>) -> usize {
        self.rows(screen, entry).count()
    }

    /// Row `i` of the picker on `screen`, with the settings panel editing `entry`.
    pub(crate) fn row(&self, screen: Option<PickerScreen>, i: usize, entry: Option<&str>) -> Option<PickerRow<'_>> {
        self.rows(screen, entry).nth(i)
    }

    /// Whether profile `id` is a game's own.
    pub fn is_own(&self, id: u16) -> bool {
        self.get(id).is_some_and(|p| p.owner.is_some())
    }

    /// Whether `entry`, the one the settings panel edits, has no profile of its own and so can make one.
    pub(crate) fn can_make_own(&self, entry: Option<&str>) -> bool {
        entry.is_some_and(|g| self.own_of(g).is_none())
    }

    /// A profile's name as shown: `kind`'s own name for a game's own, and its default name for none or
    /// for one that is gone.
    pub(crate) fn shown_name(&self, id: u16, kind: Kind) -> &str {
        match self.get(id) {
            Some(p) if p.owner.is_some() => kind.own_name(),
            Some(p) => p.name.as_str(),
            None => kind.default_name(),
        }
    }
}

/// Opens the editor for a profile's name.
fn name_profile(editors: &mut Editors, kind: Kind, action: ProfileName, text: String) {
    editors.open(Editor { purpose: Purpose::ProfileName(kind, action), text });
}

/// A row of the picker. A tap chooses it for the level being edited and closes the picker, a long press
/// opens the profile's menu, and its Edit button opens its edit screen. The game's row, when the game has
/// no profile of its own, makes one.
pub(crate) fn on_row<O: Owner>(o: &mut O, i: usize, part: RowPart, long: bool, cx: &PickerCx, editors: &mut Editors, out: &mut Out) {
    let entry = cx.entry.as_deref();
    let screen = o.picker();
    let row = o.profiles().row(screen, i, entry).map(|r| match r {
        PickerRow::Own(id) => Row::Own(id),
        PickerRow::New => Row::New,
        PickerRow::Default => Row::Default,
        PickerRow::Named(id, _) => Row::Named(id),
        // The settings' (see `State::on_row`).
        PickerRow::Reset => Row::New,
    });
    match screen {
        Some(PickerScreen::Load(own)) => {
            match row {
                Some(Row::Default) if !long => load_into(o, own, 0, out),
                Some(Row::Named(id)) if !long => load_into(o, own, id, out),
                _ => {}
            }
            return;
        }
        Some(PickerScreen::SaveAs(own)) => {
            match row {
                Some(Row::New) if !long => name_profile(editors, O::KIND, ProfileName::SaveAs(own), String::new()),
                Some(Row::Named(to)) if !long => o.set_picker(Some(PickerScreen::Overwrite { own, to })),
                _ => {}
            }
            return;
        }
        _ => {}
    }
    let (id, profile) = match row {
        None | Some(Row::New) => return,
        Some(Row::Own(None)) => return make_own(o, cx.chosen, cx, out),
        Some(Row::Default) => (0, false),
        Some(Row::Own(Some(id)) | Row::Named(id)) => (id, true),
    };
    match (part, long) {
        (RowPart::Edit, _) if profile => o.open_edit(id),
        (_, true) if profile => o.set_picker(Some(PickerScreen::Menu(id))),
        (_, false) => {
            o.set_picker(None);
            out.events.push(Event::Profile(O::KIND, ProfileEvent::Choose(id)));
        }
        _ => {}
    }
}

/// A row of the picker without the borrow of the profiles it was read from.
#[derive(Clone, Copy)]
enum Row {
    Own(Option<u16>),
    New,
    Default,
    Named(u16),
}

/// Copies profile `from` into `own`, a game's own profile, and opens its edit screen, where Undo puts it
/// back as it was.
fn load_into<O: Owner>(o: &mut O, own: u16, from: u16, out: &mut Out) {
    o.open_edit(own);
    if o.profiles_mut().copy_into(own, from) {
        out.effects.push(o.save());
    }
}

/// Gives the game the panel edits, if it has none, its own profile as a copy of profile `from`. Its edit
/// screen opens, and State chooses it for the game and saves the profiles.
pub(crate) fn make_own<O: Owner>(o: &mut O, from: u16, cx: &PickerCx, out: &mut Out) {
    let Some(entry) = cx.entry.as_ref().filter(|g| o.profiles().own_of(g).is_none()) else {
        return;
    };
    let Some(own) = o.profiles_mut().make_own(from, entry) else {
        return;
    };
    o.open_edit(own);
    out.events.push(Event::Profile(O::KIND, ProfileEvent::MadeOwn(own)));
}

/// The picker's buttons, but Back (see [`back`]), with `users` telling who uses a profile.
pub(crate) fn on<O: Owner>(o: &mut O, id: ProfileId, users: impl Fn(u16) -> Users, editors: &mut Editors, out: &mut Out) {
    let Some(screen) = o.picker() else {
        return;
    };
    debug_assert!(screen.profile().is_none_or(|p| o.profiles().get(p).is_some()), "the profile of the screen open is there: {screen:?}");
    match screen {
        PickerScreen::List if id == ProfileId::New => name_profile(editors, O::KIND, ProfileName::New, String::new()),
        PickerScreen::Menu(p) => on_menu(o, id, p, users, editors, out),
        PickerScreen::Overwrite { own, to } if id == ProfileId::ConfirmSave => {
            o.set_picker(Some(PickerScreen::List));
            if o.profiles_mut().copy_into(to, own) {
                out.effects.push(o.save());
            }
        }
        PickerScreen::Delete(p) if id == ProfileId::ConfirmDelete => delete(o, p, out),
        PickerScreen::List | PickerScreen::Load(_) | PickerScreen::SaveAs(_) | PickerScreen::Overwrite { .. } | PickerScreen::Delete(_) => {}
    }
}

/// A profile's menu (a long press on its row).
fn on_menu<O: Owner>(o: &mut O, id: ProfileId, p: u16, users: impl Fn(u16) -> Users, editors: &mut Editors, out: &mut Out) {
    let own = o.profiles().is_own(p);
    match id {
        ProfileId::Edit => o.open_edit(p),
        ProfileId::Rename => {
            let name = o.profiles().shown_name(p, O::KIND).to_owned();
            name_profile(editors, O::KIND, ProfileName::Rename(p), name);
        }
        ProfileId::Duplicate => {
            let name = o.profiles().unique_name(o.profiles().shown_name(p, O::KIND));
            name_profile(editors, O::KIND, ProfileName::Duplicate(p), name);
        }
        ProfileId::LoadFrom if own => o.set_picker(Some(PickerScreen::Load(p))),
        ProfileId::SaveAs if own => o.set_picker(Some(PickerScreen::SaveAs(p))),
        ProfileId::Reset if own => load_into(o, p, 0, out),
        ProfileId::Delete if own => {}
        ProfileId::Delete if users(p).any() => o.set_picker(Some(PickerScreen::Delete(p))),
        ProfileId::Delete => delete(o, p, out),
        _ => {}
    }
}

/// The editor saved a name: for a new profile, which then opens on its edit screen, a renamed profile, or
/// a copy.
pub(crate) fn named<O: Owner>(o: &mut O, action: ProfileName, name: &str, out: &mut Out) {
    if name.trim().is_empty() {
        return;
    }
    match action {
        ProfileName::New => {
            if let Some(id) = o.profiles_mut().add(name) {
                o.open_edit(id);
            }
        }
        ProfileName::Rename(id) => {
            o.profiles_mut().rename(id, name);
            o.set_picker(Some(PickerScreen::List));
        }
        ProfileName::Duplicate(id) | ProfileName::SaveAs(id) => {
            o.profiles_mut().duplicate(id, name);
            o.set_picker(Some(PickerScreen::List));
        }
    }
    out.effects.push(o.save());
}

/// Deletes profile `id`. State then puts the games that used it, and all games if it was theirs, back on
/// Default.
fn delete<O: Owner>(o: &mut O, id: u16, out: &mut Out) {
    o.profiles_mut().remove(id);
    o.set_picker(Some(PickerScreen::List));
    out.effects.push(o.save());
    out.events.push(Event::Profile(O::KIND, ProfileEvent::Deleted(id)));
}

/// Where Back goes from one of the picker's screens: from a profile's screens to the list, and from the
/// list out of the picker (none). A kind's edit screen goes back to the list too.
pub fn back(screen: PickerScreen) -> Option<PickerScreen> {
    match screen {
        PickerScreen::List => None,
        PickerScreen::Menu(_) | PickerScreen::Delete(_) => Some(PickerScreen::List),
        PickerScreen::Load(id) | PickerScreen::SaveAs(id) => Some(PickerScreen::Menu(id)),
        PickerScreen::Overwrite { own, .. } => Some(PickerScreen::SaveAs(own)),
    }
}
