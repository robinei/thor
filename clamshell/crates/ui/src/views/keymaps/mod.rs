//! The keymaps' screens, over the settings tab (see `core`'s `keymap_ui`): the picker (a scrolled list:
//! Default, then the keymaps, each with who uses it and an Edit button; New), a keymap's menu, deleting
//! one in use, the edit screen (one layer of the keymap, named at the top right, a button to the list of
//! layers; Close at the top left; the pad's controls drawn in two clusters at the sides, as they sit
//! beside the screen, each the layer changes with a label; between the label columns, the whole height, a
//! panel for the selected input: the ways to remap it as icons, its parameters), the layers' lists and a
//! layer's menu, the same while a control is picked on it, what holds the gyro on, a part of the mouse
//! on its drawing, and a key chord typed on the keyboard.
//!
//! On black like the rest: the pad's parts are thin dim outlines; blue is what a keymap maps to something
//! else, grey what it only tunes.

use std::fmt::Write;

use clamshell_core::keymap::{
    Actuation, Binding, Button, Change, Chord, Dir, Input, Keymap, Kind, Layer, MouseButton, Output, PadOut, Param, Side, change, params_of,
};
use clamshell_core::keymap_ui::{EditView, Screen, holds, picked, way_of, ways};
use clamshell_core::slider::{Control, Slider};
use clamshell_core::{Id, KeymapId, ListId, RowPart, Scope, Target};

use super::format::Fmt;
use super::pad::{Pad, Part};
use super::style::{self, Look, Text, Tone};
use super::widgets::{EditCell, HeaderRight, HeldSlider, ListArea, ListRow};
use super::{Frame, Track, editor, widgets};
use crate::draw::{Align, Column, Keep, LINE, Rect, Rgb, Span};
use crate::icons::Icon;

mod choose;
mod edit;
mod panel;
mod words;

use choose::*;
use edit::*;
use panel::*;
use words::*;

#[cfg(test)]
pub(super) use edit::edit_layout;

/// A list row's height and a button's, of the area's height.
const ROW_H: f32 = 0.1;
const BUTTON_H: f32 = 0.085;

pub(super) fn draw(f: &mut Frame, screen: Screen) {
    match screen {
        Screen::List => list(f),
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

/// The picker.
fn list(f: &mut Frame) {
    let s = f.s;
    let scope = match (s.bottom().scope(), s.editing()) {
        (Scope::Game, Some(game)) => game.title.as_str(),
        _ => "all games",
    };
    let title = Fmt::<128>::of(format_args!("Keymap for {scope}"));
    let area = header(f, &title, HeaderRight::Action(Id::Keymap(KeymapId::New), "New"));
    let list = ListArea::new(f, area, f.body().h * ROW_H);
    list.hint(f, "Long press a keymap to rename, duplicate or delete it");
    let chosen = s.keymap_chosen();
    let rows = list.scrolled(f, ListId::Keymaps, s.list_len(ListId::Keymaps));
    for i in rows.shown() {
        let Some(row) = s.keymap_row(i) else {
            continue;
        };
        let id = row.map_or(0, |k| k.id);
        // (A keymap that is gone counts as Default.)
        let mine = id == chosen || (id == 0 && s.keymaps().get(chosen).is_none());
        let who = if row.is_some() { users(f, id) } else { Fmt::of(format_args!("As on the pad")) };
        let at = |part| Id::Row { list: ListId::Keymaps, i: i as u16, part };
        let item = ListRow {
            label: row.map_or("Default", |k| k.name.as_str()),
            tone: Tone::high_if(mine),
            mark: mine,
            note: &who,
            edit: if row.is_some() { EditCell::Edit(at(RowPart::Edit)) } else { EditCell::Empty },
        };
        widgets::list_row(f, at(RowPart::Main), rows.row(i), list.pad, list.size, item);
    }
    list.end(f, rows);
}

fn menu(f: &mut Frame, id: u16) {
    let name = f.s.keymap_name(id);
    let area = header(f, name, HeaderRight::None);
    let k = |id| Id::Keymap(id);
    let items = [
        (k(KeymapId::Edit), "Edit", Tone::Fn),
        (k(KeymapId::Rename), "Rename", Tone::Fn),
        (k(KeymapId::Duplicate), "Duplicate", Tone::Fn),
        (k(KeymapId::Delete), "Delete", Tone::Accent),
    ];
    widgets::menu(f, area, &items);
}

fn delete(f: &mut Frame, id: u16) {
    let name = f.s.keymap_name(id);
    let title = Fmt::<128>::of(format_args!("Delete {name}?"));
    let area = header(f, &title, HeaderRight::None);
    let who = users(f, id);
    let size = f.text(Text::Body);
    let text = Fmt::<160>::of(format_args!("In use ({}): deleting it puts them back on Default.", who.as_str()));
    let used = f.c.wrap(Span { x: area.x, y: area.y, w: area.w }, size, Tone::Normal, &text, 3);
    let pad = f.pad();
    let rest = Rect::new(area.x, area.y + used + pad, area.w, area.h - used - pad);
    widgets::menu(f, rest, &[(Id::Keymap(KeymapId::Back), "Keep it", Tone::Fn), (Id::Keymap(KeymapId::ConfirmDelete), "Delete", Tone::Accent)]);
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
