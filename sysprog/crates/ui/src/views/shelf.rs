//! shelf on the main screen: the game list on the left, the selected game's art and details on the right.

use sysprog_core::Id;

use super::{DIM, Frame, Look, SURFACE_HI, TEXT, TEXT_HI};
use crate::draw::{Rect, rgb};

/// Outer padding, of the screen's height (at least 4 pixels).
const PAD: f32 = 0.03;
/// The list's share of the width.
const LIST_W: f32 = 0.4;
/// Rows that fit on the screen.
const ROWS: f32 = 8.0;
/// Title size, of a row's height.
const ROW_TEXT: f32 = 0.34;
/// The colour swatch (box art stand-in), of a row's height.
const SWATCH: f32 = 0.4;
/// The art's height, of the screen's.
const ART_H: f32 = 0.66;
/// The selected game's title and detail line sizes, of the screen's height, and the gap between them.
const TITLE: f32 = 0.055;
const DETAIL: f32 = 0.032;
const TITLE_STEP: f32 = 0.085;

pub(super) fn draw(f: &mut Frame, w: f32, h: f32) {
    let s = f.s;
    let pad = (h * PAD).max(4.0);
    let list_w = w * LIST_W;
    let row_h = (h - pad) / ROWS;
    let size = row_h * ROW_TEXT;
    for (i, game) in s.games.iter().enumerate() {
        let r = Rect::new(pad, pad + i as f32 * row_h, list_w - 2.0 * pad, row_h - pad * 0.5);
        if r.y + r.h > h {
            break;
        }
        let selected = s.selected.as_ref() == Some(&game.id);
        let look = Look { radius: 8.0, fill: selected.then_some(SURFACE_HI), inset: 0.0, ring_inset: -2.0 };
        f.button(Id::Game(game.id.clone()), r, look, |c| {
            let swatch = Rect::new(r.x + pad * 0.5, r.y + r.h * (1.0 - SWATCH) / 2.0, r.h * SWATCH, r.h * SWATCH);
            c.rounded(swatch, 3.0, rgb(game.color));
            let text_x = swatch.x + swatch.w + pad * 0.7;
            let color = if selected { TEXT_HI } else { TEXT };
            c.text(text_x, r.y + (r.h - size * 1.2) / 2.0, size, rgb(color), &game.title);
        });
    }
    if let Some(game) = s.selected_game() {
        let c = &mut *f.c;
        let art = Rect::new(list_w + pad, pad, w - list_w - 2.0 * pad, h * ART_H);
        c.rounded(art, 12.0, rgb(game.color));
        let mut y = art.y + art.h + pad;
        c.text(art.x, y, h * TITLE, rgb(TEXT_HI), &game.title);
        y += h * TITLE_STEP;
        let power = s.power_of(&game.id).label();
        let x = art.x + c.text(art.x, y, h * DETAIL, rgb(DIM), "Power: ");
        c.text(x, y, h * DETAIL, rgb(DIM), power);
    }
}
