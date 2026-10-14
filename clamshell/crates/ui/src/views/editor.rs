//! The editor's screen, shared by the environment and Wi-Fi passwords: a line saying what is edited, the
//! field with the text so far (its end, if long), Cancel and the save button, and the keyboard below (its
//! keys type here, Enter saves, Esc cancels).

use clamshell_core::Id;

use super::{ACCENT, DIM, Frame, Look, PRESSED, SURFACE, SURFACE_HI, TEXT, TEXT_HI, keyboard, label};
use crate::draw::{Keep, Rect, rgb};

/// Padding and the title's text size, of the area's height.
const PAD: f32 = 0.03;
const TITLE: f32 = 0.032;

pub(super) fn draw(f: &mut Frame, body: Rect, title: &str, save: &str) {
    let Some(editor) = &f.s.editor else {
        return;
    };
    let pad = body.h * PAD;
    let x = body.x + pad;
    let w = body.w - 2.0 * pad;
    let size = body.h * TITLE;
    f.c.text_fit((x, body.y + pad * 0.5, w), size, rgb(DIM), title, Keep::Start);

    // The field, with a caret after the text (its end, when it is too long); Cancel and save beside it.
    let row = Rect::new(x, body.y + pad * 0.5 + size * 1.6, w, body.h * 0.1);
    let field = Rect::new(row.x, row.y, row.w * 0.7, row.h);
    f.c.rounded(field, 8.0, rgb(SURFACE_HI));
    let text_size = field.h * 0.42;
    let ty = field.y + (field.h - text_size * 1.2) / 2.0;
    let tx = field.x + pad * 0.6;
    let tw = f.c.text_fit((tx, ty, field.w - pad * 1.5), text_size, rgb(TEXT_HI), &editor.text, Keep::End);
    f.c.fill(Rect::new(tx + tw + 2.0, ty, 2.0, text_size * 1.2), rgb(ACCENT));

    let buttons = Rect::new(field.x + field.w + pad * 0.5, row.y, row.w - field.w - pad * 0.5, row.h);
    let look = Look { radius: 8.0, fill: Some(SURFACE), inset: 0.0, ring_inset: -2.0 };
    let cancel = buttons.cell(2, 1, pad * 0.5, 0);
    f.button(Id::EditorCancel, cancel, look, |c| label(c, cancel, cancel.h * 0.36, TEXT, "Cancel"));
    let ok = buttons.cell(2, 1, pad * 0.5, 1);
    let look = Look { fill: Some(PRESSED), ..look };
    f.button(Id::EditorSave, ok, look, |c| label(c, ok, ok.h * 0.36, TEXT_HI, save));

    let keys_y = row.y + row.h + pad;
    keyboard::draw(f, Rect::new(body.x, keys_y, body.w, body.y + body.h - keys_y));
}
