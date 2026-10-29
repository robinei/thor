//! The editor screen, used for the environment, Wi-Fi passwords, the account password (shown as dots)
//! and keymap names. It has a line saying what is being edited, the field with the text so far (only its
//! end if it is long), Cancel and the save button, and the keyboard below. The keyboard's keys type into
//! the field; Enter saves, Esc cancels and Ctrl+V pastes the clipboard's text. Typing a key chord for a
//! keymap uses the same layout ([`field`]).

use clamshell_state::{EditorId, Id};

use super::keyboard;
use crate::gfx::{Align, Keep, LINE, Rect};
use crate::toolkit::Frame;
use crate::toolkit::style::{self, RADIUS, SURFACE_HI, Text, Tone};

/// The field's row height as a fraction of the area's height, the field's share of the row's width, and
/// the text sizes of the field and buttons as fractions of the row's height.
const ROW_H: f32 = 0.1;
const FIELD_W: f32 = 0.7;
const FIELD_TEXT: f32 = 0.42;
const BUTTON_TEXT: f32 = 0.36;

pub(super) fn draw(f: &mut Frame, title: &str, save: &str) {
    let Some(editor) = f.s.bottom().editor() else {
        return;
    };
    field_with(f, title, &editor.text, (Id::Editor(EditorId::Cancel), "Cancel"), (Id::Editor(EditorId::Save), save), editor.snippets());
}

/// Draws the editor's layout: `title`, a field with `text` and a caret, and the two buttons. `cancel` and
/// `save` give each button's target and label. The keyboard goes below.
pub(super) fn field(f: &mut Frame, title: &str, text: &str, cancel: (Id, &str), save: (Id, &str)) {
    field_with(f, title, text, cancel, save, &[]);
}

/// [`field`], with a row of `snippets` between the field and the keyboard, each a key that types it in.
fn field_with(f: &mut Frame, title: &str, text: &str, cancel: (Id, &str), save: (Id, &str), snippets: &[&str]) {
    let body = f.body();
    let pad = f.pad();
    let area = style::content(body).shifted(0.0, pad * 0.5);
    let size = f.text(Text::Small);
    f.c.text_fit(area.top_line(size), size, Tone::Dim, title, Keep::Start);

    // The field, with a caret after the text, showing only the end when the text is too long. Cancel and
    // save go beside it.
    let mut row = Rect::new(area.x, area.y + size * 1.6, area.w, f.body().h * ROW_H);
    let field = row.take_left(row.w * FIELD_W);
    f.c.rounded(field, RADIUS, SURFACE_HI);
    let text_size = field.h * FIELD_TEXT;
    let text_area = Rect::new(field.x + pad * 0.6, field.y, field.w - pad * 1.5, field.h);
    let tw = f.c.text_fit(text_area, text_size, Tone::High, text, Keep::End);
    let ty = field.y + (field.h - text_size * LINE) / 2.0;
    f.c.fill(Rect::new(text_area.x + tw + 2.0, ty, 2.0, text_size * LINE), Tone::Accent);

    let buttons = Rect::new(row.x + pad * 0.5, row.y, row.w - pad * 0.5, row.h);
    let (cancel_id, cancel_label) = cancel;
    f.back_over(cancel_id.clone());
    let cancel = buttons.cell(2, 1, pad * 0.5, 0);
    f.text_button(cancel_id, cancel, cancel.h * BUTTON_TEXT, Tone::Fn, cancel_label, Align::Center);
    let (save_id, save_label) = save;
    let ok = buttons.cell(2, 1, pad * 0.5, 1);
    f.text_button(save_id, ok, ok.h * BUTTON_TEXT, Tone::Fn, save_label, Align::Center);

    let mut keys_y = field.y + field.h + pad;
    if !snippets.is_empty() {
        let line = Rect::new(body.x + pad * 0.5, keys_y, body.w - pad, field.h * 0.8);
        for (i, snippet) in snippets.iter().enumerate() {
            let cell = line.cell(snippets.len().max(4), 1, pad * 0.5, i);
            // A space shows as a word, being a key of its own here.
            let label = if *snippet == " " { "space" } else { snippet };
            f.text_button(Id::Editor(EditorId::Insert(i as u8)), cell, cell.h * BUTTON_TEXT, Tone::Fn, label, Align::Center);
        }
        keys_y += line.h + pad * 0.5;
    }
    keyboard::draw(f, Rect::new(body.x, keys_y, body.w, body.y + f.body().h - keys_y));
}
