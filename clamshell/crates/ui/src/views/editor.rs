//! The editor's screen, shared by the environment, Wi-Fi passwords and keymaps' names: a line saying what
//! is edited, the field with the text so far (its end, if long), Paste (the clipboard's text; dim
//! without any), Cancel and the save button, and the keyboard below (its keys type here, Enter saves, Esc cancels). A key chord being typed for a keymap
//! looks the same ([`field`]).

use clamshell_core::{EditorId, Id};

use super::keyboard;
use crate::draw::{Align, Keep, LINE, Rect};
use crate::toolkit::Frame;
use crate::toolkit::style::{self, RADIUS, SURFACE_HI, Text, Tone};

/// The field's row, of the area's height; each button's share of the row's width (the field has the
/// rest); the field's and the buttons' text, of the row's height.
const ROW_H: f32 = 0.1;
const BUTTON_W: f32 = 0.15;
const FIELD_TEXT: f32 = 0.42;
const BUTTON_TEXT: f32 = 0.36;

pub(super) fn draw(f: &mut Frame, title: &str, save: &str) {
    let Some(editor) = f.s.bottom().editor() else {
        return;
    };
    let paste = if f.s.can_paste() { Tone::Fn } else { Tone::Dim };
    let buttons =
        [(Id::Editor(EditorId::Paste), "Paste", paste), (Id::Editor(EditorId::Cancel), "Cancel", Tone::Fn), (Id::Editor(EditorId::Save), save, Tone::Fn)];
    field(f, title, &editor.text, &buttons);
}

/// The editor's layout: `title`, a field with `text` and a caret, the buttons beside it (their targets,
/// labels and tones), the keyboard below.
pub(super) fn field(f: &mut Frame, title: &str, text: &str, buttons: &[(Id, &str, Tone)]) {
    let body = f.body();
    let pad = f.pad();
    let area = style::content(body).shifted(0.0, pad * 0.5);
    let size = f.text(Text::Small);
    f.c.text_fit(area.top_line(size), size, Tone::Dim, title, Keep::Start);

    // The field, with a caret after the text (its end, when it is too long); Cancel and save beside it.
    let mut row = Rect::new(area.x, area.y + size * 1.6, area.w, f.body().h * ROW_H);
    let field = row.take_left(row.w * (1.0 - BUTTON_W * buttons.len() as f32));
    f.c.rounded(field, RADIUS, SURFACE_HI);
    let text_size = field.h * FIELD_TEXT;
    let text_area = Rect::new(field.x + pad * 0.6, field.y, field.w - pad * 1.5, field.h);
    let tw = f.c.text_fit(text_area, text_size, Tone::High, text, Keep::End);
    let ty = field.y + (field.h - text_size * LINE) / 2.0;
    f.c.fill(Rect::new(text_area.x + tw + 2.0, ty, 2.0, text_size * LINE), Tone::Accent);

    let area = Rect::new(row.x + pad * 0.5, row.y, row.w - pad * 0.5, row.h);
    for (i, (id, label, tone)) in buttons.iter().enumerate() {
        let cell = area.cell(buttons.len(), 1, pad * 0.5, i);
        f.text_button(id.clone(), cell, cell.h * BUTTON_TEXT, *tone, label, Align::Center);
    }

    let keys_y = field.y + field.h + pad;
    keyboard::draw(f, Rect::new(body.x, keys_y, body.w, body.y + f.body().h - keys_y));
}
