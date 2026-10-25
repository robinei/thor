//! A kind's picker, drawn over the settings tab. Its rules are in [`clamshell_state::profiles::picker`].
//!
//! - The picker, a scrolled list. For a game it starts with its own profile or the row that makes one.
//!   Then come Default, the named profiles (each with who uses it and an Edit button), and New.
//! - A profile's menu, and the confirmation for deleting one in use.
//! - The lists for loading a profile into a game's own and for saving the game's own as a named one. They
//!   show the picker's rows without the game's own profile, the Edit buttons and the mark. Saving lists a
//!   new profile first, and saving over a named one asks first.

use clamshell_state::profiles::picker::{Kind, PickerRow, PickerScreen, ProfileId};
use clamshell_state::{Id, RowPart, Scope};

use crate::gfx::{Rect, Span};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Text, Tone};
use crate::toolkit::widgets::{EditCell, HeaderRight, ListArea, ListRow};
use crate::toolkit::{Frame, widgets};

/// The height of a list row, as a fraction of the area's height.
pub(super) const ROW_H: f32 = 0.1;

/// What a kind's picker says.
struct Words {
    /// "Keymap for …".
    title: &'static str,
    make_own: &'static str,
    new: &'static str,
    /// Default's note.
    default_note: &'static str,
    load_title: &'static str,
    load_hint: &'static str,
    save_title: &'static str,
    save_hint: &'static str,
    pick_hint: &'static str,
    /// What saving over a named profile does, without and with the games using it.
    overwrite: &'static str,
    overwrite_used: &'static str,
}

fn words(kind: Kind) -> Words {
    match kind {
        Kind::Keymap => Words {
            title: "Keymap",
            make_own: "Make own keymap",
            new: "New keymap…",
            default_note: "As on the pad",
            load_title: "Load into own keymap",
            load_hint: "Its bindings replace the own keymap's (Undo goes back)",
            save_title: "Save own keymap as",
            save_hint: "A new keymap, or one whose bindings it replaces",
            pick_hint: "Long press a keymap to rename, duplicate or delete it",
            overwrite: "Its bindings become the own keymap's.",
            overwrite_used: "Its bindings become the own keymap's, for every game using it",
        },
        Kind::Curve => Words {
            title: "Tone curve",
            make_own: "Make own curve",
            new: "New curve…",
            default_note: "Unchanged",
            load_title: "Load into own curve",
            load_hint: "Its values replace the own curve's",
            save_title: "Save own curve as",
            save_hint: "A new curve, or one whose values it replaces",
            pick_hint: "Maps the content's SDR or HDR to the screen's · Long press a curve for its menu",
            overwrite: "Its values become the own curve's.",
            overwrite_used: "Its values become the own curve's, for every game using it",
        },
    }
}

pub(super) fn draw(f: &mut Frame, kind: Kind, screen: PickerScreen) {
    match screen {
        PickerScreen::List => list(f, kind, Listing::Pick),
        PickerScreen::Load(_) => list(f, kind, Listing::Load),
        PickerScreen::SaveAs(_) => list(f, kind, Listing::Save),
        PickerScreen::Overwrite { to, .. } => overwrite(f, kind, to),
        PickerScreen::Menu(id) => menu(f, kind, id),
        PickerScreen::Delete(id) => delete(f, kind, id),
    }
}

/// The picker's header (see [`widgets::header`]): Back, the title, and `right` on the right.
fn header(f: &mut Frame, kind: Kind, title: &str, right: HeaderRight) -> Rect {
    widgets::header(f, Some(Id::Profile(kind, ProfileId::Back)), title, false, right)
}

/// Who uses a profile, in words.
pub(super) fn users(f: &Frame, kind: Kind, id: u16) -> Fmt<48> {
    let u = f.s.prefs().settings().users(kind.setting(), id);
    match (u.all_entries, u.entries) {
        (true, 0) => Fmt::of(format_args!("All games")),
        (true, n) => Fmt::of(format_args!("All games, {n} {}", if n == 1 { "game" } else { "games" })),
        (false, 0) => Fmt::of(format_args!("Not used")),
        (false, n) => Fmt::of(format_args!("{n} {}", if n == 1 { "game" } else { "games" })),
    }
}

/// What the list is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// The picker.
    Pick,
    /// A profile to copy into the game's own.
    Load,
    /// Where to save the game's own: a new one, or over a named one.
    Save,
}

/// The picker, or the list of profiles to load into a game's own or to save it as.
fn list(f: &mut Frame, kind: Kind, listing: Listing) {
    let s = f.s;
    let w = words(kind);
    let panel = s.panel();
    let scope = match (panel.scope(), panel.editing()) {
        (Scope::Runner | Scope::Content, Some(entry)) => entry.title,
        _ => "all",
    };
    let (area, hint) = match listing {
        Listing::Load => (header(f, kind, w.load_title, HeaderRight::None), w.load_hint),
        Listing::Save => (header(f, kind, w.save_title, HeaderRight::None), w.save_hint),
        Listing::Pick => {
            let title = Fmt::<128>::of(format_args!("{} for {scope}", w.title));
            (header(f, kind, &title, HeaderRight::Action(Id::Profile(kind, ProfileId::New), "New")), w.pick_hint)
        }
    };
    let loading = (listing != Listing::Pick).then_some(());
    let list = ListArea::new(f, area, f.body().h * ROW_H);
    list.hint(f, hint);
    let chosen = panel.profile_chosen(kind);
    let rows = list.scrolled(f, kind.list(), s.lists().len(kind.list()));
    for i in rows.shown() {
        let Some(row) = panel.profile_row(kind, i) else {
            continue;
        };
        let at = |part| Id::Row { list: kind.list(), i: i as u16, part };
        let (id, label, note) = match row {
            PickerRow::Own(Some(id)) => (Some(id), kind.own_name(), Fmt::<48>::of(format_args!("This game only"))),
            PickerRow::Own(None) => (None, w.make_own, Fmt::of(format_args!("From {}", s.profile_name(kind, chosen)))),
            PickerRow::New => (None, w.new, Fmt::new()),
            PickerRow::Default => (Some(0), kind.default_name(), Fmt::of(format_args!("{}", w.default_note))),
            PickerRow::Named(id, name) => (Some(id), name, users(f, kind, id)),
            PickerRow::Reset => {
                let below = panel.below_name().unwrap_or("the default");
                (None, "Reset", Fmt::of(format_args!("to {below}'s: {}", s.profile_name(kind, panel.inherited_value(kind.setting())))))
            }
        };
        // A profile that no longer exists counts as Default.
        let mine = loading.is_none() && (id == Some(chosen) || (id == Some(0) && !s.profile_exists(kind, chosen)));
        let tone = if id.is_none() { Tone::Fn } else { Tone::high_if(mine) };
        let edit = match id {
            _ if loading.is_some() => EditCell::None,
            Some(id) if id != 0 => EditCell::Edit(at(RowPart::Edit)),
            _ => EditCell::Empty,
        };
        let item = ListRow { label, tone, mark: mine, note: &note, edit };
        widgets::list_row(f, at(RowPart::Main), rows.row(i), list.pad, list.size, item);
    }
    list.end(f, rows);
}

fn menu(f: &mut Frame, kind: Kind, id: u16) {
    let name = f.s.profile_name(kind, id);
    let area = header(f, kind, name, HeaderRight::None);
    let p = |id| Id::Profile(kind, id);
    // A game's own profile becomes a shared one when it is named.
    if f.s.profile_is_own(kind, id) {
        let items = [
            (p(ProfileId::Edit), "Edit", Tone::Fn),
            (p(ProfileId::LoadFrom), "Load from…", Tone::Fn),
            (p(ProfileId::SaveAs), "Save as…", Tone::Fn),
            (p(ProfileId::Reset), "Reset", Tone::Accent),
        ];
        return widgets::menu(f, area, items);
    }
    let items = [
        (p(ProfileId::Edit), "Edit", Tone::Fn),
        (p(ProfileId::Rename), "Rename", Tone::Fn),
        (p(ProfileId::Duplicate), "Duplicate", Tone::Fn),
        (p(ProfileId::Delete), "Delete", Tone::Accent),
    ];
    widgets::menu(f, area, items);
}

fn delete(f: &mut Frame, kind: Kind, id: u16) {
    let name = f.s.profile_name(kind, id);
    let title = Fmt::<128>::of(format_args!("Delete {name}?"));
    let text = Fmt::<160>::of(format_args!("In use ({}): deleting it puts them back on {}.", users(f, kind, id).as_str(), kind.default_name()));
    confirm(f, kind, &title, &text, (ProfileId::ConfirmDelete, "Delete"));
}

/// Asks to confirm saving a game's own profile over the named profile `to`, since the change reaches the
/// games using it.
fn overwrite(f: &mut Frame, kind: Kind, to: u16) {
    let w = words(kind);
    let name = f.s.profile_name(kind, to);
    let title = Fmt::<128>::of(format_args!("Replace {name}?"));
    let text = if f.s.prefs().settings().users(kind.setting(), to).any() {
        Fmt::<160>::of(format_args!("{} ({}).", w.overwrite_used, users(f, kind, to).as_str()))
    } else {
        Fmt::of(format_args!("{}", w.overwrite))
    };
    confirm(f, kind, &title, &text, (ProfileId::ConfirmSave, "Replace"));
}

/// A screen asking to confirm something that can't be undone: its title, what it does, then Keep it,
/// which goes back, and the action.
fn confirm(f: &mut Frame, kind: Kind, title: &str, text: &str, (act, label): (ProfileId, &str)) {
    let area = header(f, kind, title, HeaderRight::None);
    let size = f.text(Text::Body);
    let used = f.c.wrap(Span { x: area.x, y: area.y, w: area.w }, size, Tone::Normal, text, 3);
    let pad = f.pad();
    let rest = Rect::new(area.x, area.y + used + pad, area.w, area.h - used - pad);
    widgets::menu(f, rest, [(Id::Profile(kind, ProfileId::Back), "Keep it", Tone::Fn), (Id::Profile(kind, act), label, Tone::Accent)]);
}
