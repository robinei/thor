//! The editor's screen, shared by the environment and Wi-Fi passwords: a line saying what is edited, the
//! field with the text so far (its end, if long), Cancel and the save button, and the keyboard below (its
//! keys type here, Enter saves, Esc cancels).

use clamshell_core::{EditorId, Id};

use super::style::{self, Look, PRESSED, SURFACE, SURFACE_HI, Text, Tone, RADIUS};
use super::{Frame, keyboard};
use crate::draw::{Align, Keep, LINE, Rect};

/// The field's row, of the area's height; the field's share of the row's width; the field's and the
/// buttons' text, of the row's height.
const ROW_H: f32 = 0.1;
const FIELD_W: f32 = 0.7;
const FIELD_TEXT: f32 = 0.42;
const BUTTON_TEXT: f32 = 0.36;

pub(super) fn draw(f: &mut Frame, body: Rect, title: &str, save: &str) {
    let Some(editor) = f.s.bottom().editor() else {
        return;
    };
    let pad = style::pad(body.h);
    let area = style::content(body).shifted(0.0, pad * 0.5);
    let size = Text::Small.size(body.h);
    f.c.text_fit(area.top_line(size), size, Tone::Dim, title, Keep::Start);

    // The field, with a caret after the text (its end, when it is too long); Cancel and save beside it.
    let mut row = Rect::new(area.x, area.y + size * 1.6, area.w, body.h * ROW_H);
    let field = row.take_left(row.w * FIELD_W);
    f.c.rounded(field, RADIUS, SURFACE_HI);
    let text_size = field.h * FIELD_TEXT;
    let text = Rect::new(field.x + pad * 0.6, field.y, field.w - pad * 1.5, field.h);
    let tw = f.c.text_fit(text, text_size, Tone::High, &editor.text, Keep::End);
    let ty = field.y + (field.h - text_size * LINE) / 2.0;
    f.c.fill(Rect::new(text.x + tw + 2.0, ty, 2.0, text_size * LINE), Tone::Accent);

    let buttons = Rect::new(row.x + pad * 0.5, row.y, row.w - pad * 0.5, row.h);
    let cancel = buttons.cell(2, 1, pad * 0.5, 0);
    f.button(Id::Editor(EditorId::Cancel), cancel, Look { fill: Some(SURFACE), ..Look::CARD }, |f| {
        f.c.text_in(cancel, cancel.h * BUTTON_TEXT, Tone::Normal, "Cancel", Align::Center);
    });
    let ok = buttons.cell(2, 1, pad * 0.5, 1);
    f.button(Id::Editor(EditorId::Save), ok, Look { fill: Some(PRESSED), ..Look::CARD }, |f| {
        f.c.text_in(ok, ok.h * BUTTON_TEXT, Tone::High, save, Align::Center);
    });

    let keys_y = field.y + field.h + pad;
    keyboard::draw(f, Rect::new(body.x, keys_y, body.w, body.y + body.h - keys_y));
}
