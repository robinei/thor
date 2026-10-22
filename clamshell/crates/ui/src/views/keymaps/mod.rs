//! The keymaps' screens, drawn over the settings tab (their rules are `core`'s `keymap_screens`):
//!
//! - The picker, a scrolled list: for a game, its own keymap or Make own keymap; then Default, the named
//!   keymaps (each with who uses it and an Edit button), and New.
//! - A keymap's menu, and the confirmation for deleting one in use.
//! - The keymaps to load into a game's own and to save it as: the picker's rows without the game's own,
//!   its Edit buttons and its mark; saving lists a new keymap first. Saving over a named one asks first.
//! - The edit screen ([`edit`](fn@edit)): one layer of the keymap, named at the top right with a button to the
//!   list of layers, and Close at the top left. The pad's controls are drawn in two clusters at the sides,
//!   as they sit beside the screen, each control the layer changes with a label. Between the label
//!   columns, the full height, is a panel for the selected input: the ways to remap it as icons, and its
//!   parameters.
//! - The layers' lists and a layer's menu; the edit screen while a control is picked on it; what holds
//!   the gyro on; a part of the mouse on its drawing; a key chord typed on the keyboard.
//!
//! Colours follow the rest of the UI on black: the pad's parts are thin dim outlines, blue marks what a
//! keymap maps to something else, and grey what it only tunes.

use clamshell_core::keymap::Input;
use clamshell_core::keymap::screens::{PickerRow, Screen, holds, picked};
use clamshell_core::{Id, KeymapId, ListId, RowPart, Scope};

use crate::draw::{Rect, Rgb, Span};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Text, Tone};
use crate::toolkit::widgets::{EditCell, HeaderRight, ListArea, ListRow};
use crate::toolkit::{Frame, widgets};

mod choose;
mod edit;
mod pad;
mod panel;
mod words;

use choose::{capture, layer_menu, layer_picker, layers, mouse};
use edit::edit;
use words::Label;

#[cfg(test)]
pub(super) use edit::edit_layout;

/// A list row's height and a button's, of the area's height.
const ROW_H: f32 = 0.1;
const BUTTON_H: f32 = 0.085;

pub(super) fn draw(f: &mut Frame, screen: Screen) {
    match screen {
        Screen::List => list(f, Listing::Pick),
        Screen::Load(_) => list(f, Listing::Load),
        Screen::SaveAs(_) => list(f, Listing::Save),
        Screen::Overwrite { to, .. } => overwrite(f, to),
        Screen::Menu(id) => menu(f, id),
        Screen::Delete(id) => delete(f, id),
        Screen::Edit(id) => edit(f, id, None),
        Screen::Pick { id, input } => edit(f, id, Some(Picking::Target(input))),
        Screen::Holds { id, input } => edit(f, id, Some(Picking::Hold(input))),
        Screen::Mouse { id, input } => mouse(f, id, input),
        Screen::Layers { id, input } => layers(f, id, input),
        Screen::LayerMenu { id, layer } => layer_menu(f, id, layer),
        Screen::LayerPicker(id) => layer_picker(f, id),
        Screen::Capture { input, keys, .. } => capture(f, input, keys),
    }
}

/// The keymap screens' header (see [`widgets::header`]): Back, the title, and on the right `right`.
fn header(f: &mut Frame, title: &str, right: HeaderRight) -> Rect {
    widgets::header(f, Some(Id::Keymap(KeymapId::Back)), title, false, right)
}

/// What is picked on the edit screen's drawing: what an input maps to, or what holds the gyro on.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Picking {
    Target(Input),
    Hold(Input),
}

impl Picking {
    /// Whether `input` can be picked.
    fn takes(self, input: Input) -> bool {
        match self {
            Picking::Target(from) => from == input || picked(from, input).is_some(),
            Picking::Hold(_) => holds(input),
        }
    }
}

/// A choice's header (what an input is remapped to, what holds the gyro on): the title centred and
/// Cancel on the right; no back arrow, since choosing something closes it and Cancel gives up. Returns
/// the area below it.
fn choice_header(f: &mut Frame, title: &str) -> Rect {
    widgets::header(f, None, title, true, HeaderRight::Action(Id::Keymap(KeymapId::Back), "Cancel"))
}

/// "Remap A to…".
fn remap_title(input: Input) -> Fmt<64> {
    Fmt::of(format_args!("Remap {} to…", Label(input)))
}

/// Who uses a keymap, in words.
fn users(f: &Frame, id: u16) -> Fmt<48> {
    let u = f.s.keymap_users(id);
    match (u.all_games, u.games) {
        (true, 0) => Fmt::of(format_args!("All games")),
        (true, n) => Fmt::of(format_args!("All games, {n} {}", if n == 1 { "game" } else { "games" })),
        (false, 0) => Fmt::of(format_args!("Not used")),
        (false, n) => Fmt::of(format_args!("{n} {}", if n == 1 { "game" } else { "games" })),
    }
}

/// What the keymaps' list is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// The picker.
    Pick,
    /// A keymap to copy into the game's own.
    Load,
    /// Where to save the game's own: a new one, or over a named one.
    Save,
}

/// The picker; or the keymaps to load into a game's own, or to save it as.
fn list(f: &mut Frame, listing: Listing) {
    let s = f.s;
    let scope = match (s.bottom().scope(), s.editing()) {
        (Scope::Runner | Scope::Content, Some(game)) => game.title,
        _ => "all",
    };
    let (area, hint) = match listing {
        Listing::Load => (header(f, "Load into own keymap", HeaderRight::None), "Its bindings replace the own keymap's (Undo goes back)"),
        Listing::Save => (header(f, "Save own keymap as", HeaderRight::None), "A new keymap, or one whose bindings it replaces"),
        Listing::Pick => {
            let title = Fmt::<128>::of(format_args!("Keymap for {scope}"));
            (header(f, &title, HeaderRight::Action(Id::Keymap(KeymapId::New), "New")), "Long press a keymap to rename, duplicate or delete it")
        }
    };
    let loading = (listing != Listing::Pick).then_some(());
    let list = ListArea::new(f, area, f.body().h * ROW_H);
    list.hint(f, hint);
    let chosen = s.keymap_chosen();
    let rows = list.scrolled(f, ListId::Keymaps, s.list_len(ListId::Keymaps));
    for i in rows.shown() {
        let Some(row) = s.keymap_row(i) else {
            continue;
        };
        let at = |part| Id::Row { list: ListId::Keymaps, i: i as u16, part };
        let (id, label, note) = match row {
            PickerRow::Own(Some(k)) => (Some(k.id), "Own keymap", Fmt::<48>::of(format_args!("This game only"))),
            PickerRow::Own(None) => (None, "Make own keymap", Fmt::of(format_args!("From {}", s.keymap_name(chosen)))),
            PickerRow::New => (None, "New keymap…", Fmt::new()),
            PickerRow::Default => (Some(0), "Default", Fmt::of(format_args!("As on the pad"))),
            PickerRow::Named(k) => (Some(k.id), k.name.as_str(), users(f, k.id)),
        };
        // (A keymap that is gone counts as Default.)
        let mine = loading.is_none() && (id == Some(chosen) || (id == Some(0) && s.keymaps().get(chosen).is_none()));
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

fn menu(f: &mut Frame, id: u16) {
    let name = f.s.keymap_name(id);
    let area = header(f, name, HeaderRight::None);
    let k = |id| Id::Keymap(id);
    // A game's own: named, it becomes a shared one.
    if f.s.keymap_is_own(id) {
        let items = [
            (k(KeymapId::Edit), "Edit", Tone::Fn),
            (k(KeymapId::LoadFrom), "Load from…", Tone::Fn),
            (k(KeymapId::SaveAs), "Save as…", Tone::Fn),
            (k(KeymapId::Delete), "Delete", Tone::Accent),
        ];
        return widgets::menu(f, area, items);
    }
    let items = [
        (k(KeymapId::Edit), "Edit", Tone::Fn),
        (k(KeymapId::Rename), "Rename", Tone::Fn),
        (k(KeymapId::Duplicate), "Duplicate", Tone::Fn),
        (k(KeymapId::Delete), "Delete", Tone::Accent),
    ];
    widgets::menu(f, area, items);
}

fn delete(f: &mut Frame, id: u16) {
    let name = f.s.keymap_name(id);
    let title = Fmt::<128>::of(format_args!("Delete {name}?"));
    let text = Fmt::<160>::of(format_args!("In use ({}): deleting it puts them back on Default.", users(f, id).as_str()));
    confirm(f, &title, &text, (KeymapId::ConfirmDelete, "Delete"));
}

/// Saving a game's own keymap over named keymap `to`: are you sure (it reaches the games using it).
fn overwrite(f: &mut Frame, to: u16) {
    let name = f.s.keymap_name(to);
    let title = Fmt::<128>::of(format_args!("Replace {name}?"));
    let text = if f.s.keymap_users(to).any() {
        Fmt::<160>::of(format_args!("Its bindings become the own keymap's, for every game using it ({}).", users(f, to).as_str()))
    } else {
        Fmt::of(format_args!("Its bindings become the own keymap's."))
    };
    confirm(f, &title, &text, (KeymapId::ConfirmSave, "Replace"));
}

/// A screen asking to confirm something that can't be undone: its title, what it does, then Keep it (back)
/// and the action.
fn confirm(f: &mut Frame, title: &str, text: &str, (act, label): (KeymapId, &str)) {
    let area = header(f, title, HeaderRight::None);
    let size = f.text(Text::Body);
    let used = f.c.wrap(Span { x: area.x, y: area.y, w: area.w }, size, Tone::Normal, text, 3);
    let pad = f.pad();
    let rest = Rect::new(area.x, area.y + used + pad, area.w, area.h - used - pad);
    widgets::menu(f, rest, [(Id::Keymap(KeymapId::Back), "Keep it", Tone::Fn), (Id::Keymap(act), label, Tone::Accent)]);
}

/// The edit screen: the labels' text, of the small text size; of the area's width, a cluster's width,
/// the room between it and its labels (where the lines turn), and the labels' column's width; the
/// bottom row's height and the panel's rows', of the area's height.
const LABEL: f32 = 0.66;
const CLUSTER_W: f32 = 0.222;
const GAP: f32 = 0.016;
const LABEL_COLUMN_W: f32 = 0.115;
const PANEL_ROW_H: f32 = 0.075;
/// The panel's and the bottom row's text, of the small text size.
const PANEL_TEXT: f32 = 0.8;
/// The controls' outlines, and the lines in to the labels (blue, dimmed).
const PART_LINE: Rgb = Rgb(0x4a4a54);
/// The buttons drawn that can't be remapped (Home, the AYN button and its neighbours).
const FIXED: Rgb = Rgb(0x2a2a30);
/// While a control is picked, those that can't be: barely there.
const MUTED: Rgb = Rgb(0x1c1c21);
const LEADER_ALPHA: f32 = 0.45;
/// Line widths in logical pixels: the controls', the lines in, a selected control's.
const PART_W: f32 = 1.5;
const LEADER_W: f32 = 1.0;
const SELECTED_W: f32 = 2.5;
/// The least room between labels, and the narrowest column (of the labels' text size) that has words.
const LABEL_GAP: f32 = 1.2;
const LABEL_MIN: f32 = 4.5;
/// The most controls in a cluster.
const CONTROLS: usize = 16;
