//! The keymap screens, drawn over the settings tab. Their rules are on [`clamshell_state::keymap::screens::Keymapping`].
//!
//! - The picker and a keymap's menu, the profiles' picker ([`super::profiles`]).
//! - The edit screen ([`edit`](fn@edit)), showing one layer of the keymap. The layer's name is at the top
//!   right with a button to the list of layers, and Close is at the top left. The pad's controls are
//!   drawn in two clusters at the sides, as they sit beside the screen, with a label for each control the
//!   layer changes. Between the label columns, at full height, is a panel for the selected input with
//!   the ways to remap it as icons, and its parameters.
//! - The layer lists and a layer's menu, the edit screen while a control is being picked on it, what
//!   holds the gyro on, a part of the mouse on its drawing, and a key chord typed on the keyboard.
//!
//! Colours follow the rest of the UI on black. The pad's parts are thin dim outlines, blue marks what a
//! keymap maps to something else, and grey marks what it only tunes.

use clamshell_state::Id;
use clamshell_state::keymap::Input;
use clamshell_state::keymap::screens::{Screen, holds, picked};
use clamshell_state::profiles::picker::{Kind, ProfileId};

use crate::gfx::{Rect, Rgb};
use crate::toolkit::format::Fmt;
use crate::toolkit::widgets::HeaderRight;
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

/// The height of a list row and of a button, as fractions of the area's height.
const ROW_H: f32 = 0.1;
const BUTTON_H: f32 = 0.085;

pub(super) fn draw(f: &mut Frame, screen: Screen) {
    match screen {
        Screen::Picker(p) => super::profiles::draw(f, Kind::Keymap, p),
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

/// The keymap screens' header (see [`widgets::header`]): Back, the title, and `right` on the right.
fn header(f: &mut Frame, title: &str, right: HeaderRight) -> Rect {
    widgets::header(f, Some(Id::Profile(Kind::Keymap, ProfileId::Back)), title, false, right)
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

/// The header for a choice, such as what an input is remapped to or what holds the gyro on. It has the
/// title centred and Cancel on the right. There is no back arrow, since choosing something closes it and
/// Cancel gives up. Returns the area below it.
fn choice_header(f: &mut Frame, title: &str) -> Rect {
    widgets::header(f, None, title, true, HeaderRight::Action(Id::Profile(Kind::Keymap, ProfileId::Back), "Cancel"))
}

/// "Remap A to…".
fn remap_title(input: Input, labels: &clamshell_state::ControlLabels) -> Fmt<64> {
    Fmt::of(format_args!("Remap {} to…", Label(input, labels)))
}

/// Who uses a keymap, in words.
fn users(f: &Frame, id: u16) -> Fmt<48> {
    super::profiles::users(f, Kind::Keymap, id)
}

/// Edit screen sizes. The labels' text is a fraction of the small text size. A cluster's width, the gap
/// between it and its labels where the lines turn, and the labels' column width are fractions of the
/// area's width. The panel's row height is a fraction of the area's height.
const LABEL: f32 = 0.66;
const CLUSTER_W: f32 = 0.222;
const GAP: f32 = 0.016;
const LABEL_COLUMN_W: f32 = 0.115;
const PANEL_ROW_H: f32 = 0.075;
/// The text size of the panel and the bottom row, as a fraction of the small text size.
const PANEL_TEXT: f32 = 0.8;
/// The colour of the controls' outlines, and how strongly the lines in to the labels show. Those lines
/// take their label's tone.
const PART_LINE: Rgb = Rgb(0x4a4a54);
/// The colour of buttons that are drawn but can't be remapped (Home, the AYN button and its neighbours).
const FIXED: Rgb = Rgb(0x2a2a30);
/// While a control is being picked, the colour of those that can't be picked, barely visible.
const MUTED: Rgb = Rgb(0x1c1c21);
const LEADER_ALPHA: f32 = 0.45;
/// Line widths in logical pixels: the controls' outlines, the lines in to the labels, and a selected
/// control's outline.
const PART_W: f32 = 1.5;
const LEADER_W: f32 = 1.0;
const SELECTED_W: f32 = 2.5;
/// The smallest gap between labels, and the narrowest column that still shows words, as multiples of
/// the labels' text size.
const LABEL_GAP: f32 = 1.2;
const LABEL_MIN: f32 = 4.5;
/// The most controls in a cluster.
const CONTROLS: usize = 16;
