//! shelf on the main screen: collections as tabs along the top; the current one's games as a list with
//! box art thumbnails on the left, scrolled to keep the selection on screen; the selected game's large
//! box art and details on the right. Images come from the image cache (placeholders in the game's
//! colour until they arrive).

use std::fmt::Write;

use sysprog_core::{Game, Id};

use super::{ACCENT, DIM, Frame, Look, SURFACE, SURFACE_HI, TEXT, TEXT_HI, wrap};
use crate::draw::{Canvas, Rect, rgb};
use crate::images::Images;

/// Outer padding, of the screen's height (at least 4 pixels).
const PAD: f32 = 0.03;
/// The tab bar's height and text size, of the screen's height.
const TABS_H: f32 = 0.08;
const TAB_TEXT: f32 = 0.034;
/// The list's share of the width.
const LIST_W: f32 = 0.42;
/// A row's height aimed for, of the screen's height (rows are stretched to fill the list exactly).
const ROW_H: f32 = 0.085;
/// Title size and thumbnail height, of a row's height; thumbnails are portrait boxes.
const ROW_TEXT: f32 = 0.36;
const THUMB: f32 = 0.82;
const BOX_ASPECT: f32 = 2.0 / 3.0;
/// The large art's height, of the list's height.
const ART_H: f32 = 0.78;
/// Details text sizes, of the screen's height.
const TITLE: f32 = 0.05;
const DETAIL: f32 = 0.03;
const DESCRIPTION: f32 = 0.028;

/// Draws shelf; returns how many list rows fit (the state keeps the selection within them).
pub(super) fn draw(f: &mut Frame, images: &mut Images, buf: &mut String, w: f32, h: f32) -> usize {
    let s = f.s;
    let pad = (h * PAD).max(4.0);
    let scale = f.c.scale();

    // Tabs: the collections' names; the current one bright with an accent line under it.
    let tabs_h = h * TABS_H;
    let size = h * TAB_TEXT;
    let mut x = pad;
    for (i, coll) in s.library.collections.iter().enumerate() {
        let width = f.c.text_width(size, &coll.name) + pad * 1.6;
        if x + width > w {
            break;
        }
        let r = Rect::new(x, 0.0, width, tabs_h);
        let current = i == s.tab;
        let look = Look { radius: 6.0, fill: None, inset: 4.0, ring_inset: 2.0 };
        f.button(Id::Tab(i as u16), r, look, |c| {
            let color = if current { TEXT_HI } else { DIM };
            c.text(r.x + pad * 0.8, r.y + (r.h - size * 1.2) / 2.0, size, rgb(color), &coll.name);
            if current {
                c.fill(Rect::new(r.x + pad * 0.8, r.y + r.h - 4.0, r.w - pad * 1.6, 2.0), rgb(ACCENT));
            }
        });
        x += width;
    }

    // The list, filling the height below the tabs exactly with whole rows.
    let list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    let rows = ((list.h / (h * ROW_H)).floor() as usize).max(1);
    let row_h = list.h / rows as f32;
    let cursor = s.cursor();
    let Some(coll) = s.collection() else {
        return rows;
    };
    for row in 0..rows {
        let position = cursor.top + row;
        let Some(game) = s.game_at(position) else {
            break;
        };
        let r = Rect::new(list.x, list.y + row as f32 * row_h, list.w - pad * 0.6, row_h - 2.0);
        let selected = position == cursor.index;
        let look = Look { radius: 8.0, fill: selected.then_some(SURFACE_HI), inset: 0.0, ring_inset: -2.0 };
        let thumb_h = r.h * THUMB;
        let thumb = Rect::new(r.x + pad * 0.4, r.y + (r.h - thumb_h) / 2.0, thumb_h * BOX_ASPECT, thumb_h);
        let size = row_h * ROW_TEXT;
        f.button(Id::Game(game.id.clone()), r, look, |c| {
            art(c, images, game, thumb, scale, 3.0, false);
            let color = if selected { TEXT_HI } else { TEXT };
            let tx = thumb.x + thumb.w + pad * 0.6;
            let room = r.x + r.w - tx - pad * 0.3;
            wrap(c, (tx, r.y + (r.h - size * 1.2) / 2.0, room), size, color, &game.title, 1);
        });
    }
    // Where in the list: a thin bar along its right edge, when it doesn't all fit.
    let len = coll.games.len();
    if len > rows {
        let track = Rect::new(list.x + list.w - 3.0, list.y, 2.0, list.h);
        f.c.fill(track, rgb(SURFACE));
        let thumb_h = (track.h * rows as f32 / len as f32).max(8.0);
        let y = track.y + (track.h - thumb_h) * cursor.top as f32 / (len - rows) as f32;
        f.c.fill(Rect::new(track.x, y, track.w, thumb_h), rgb(DIM));
    }

    // The selected game: large art, then title and details beside it.
    if let Some(game) = s.selected_game() {
        let c = &mut *f.c;
        let x0 = w * LIST_W + pad;
        let art_h = list.h * ART_H;
        let box_rect = Rect::new(x0, list.y, art_h * BOX_ASPECT, art_h);
        art(c, images, game, box_rect, scale, 10.0, true);
        let tx = box_rect.x + box_rect.w + pad * 1.2;
        let room = w - pad - tx;
        let mut y = list.y;
        y += wrap(c, (tx, y, room), h * TITLE, TEXT_HI, &game.title, 3) + pad * 0.6;
        for line in details(game, buf) {
            if !line.is_empty() {
                y += wrap(c, (tx, y, room), h * DETAIL, DIM, line, 2) + pad * 0.2;
            }
        }
        let text = if game.description.is_empty() { &game.summary } else { &game.description };
        if !text.is_empty() {
            wrap(c, (tx, y + pad * 0.6, room), h * DESCRIPTION, TEXT, text, 12);
        }
    }
    rows
}

/// The detail lines under the title: who made it and when, the genre, the players. Formatted into
/// `buf` (reused), split by newlines.
fn details<'b>(game: &Game, buf: &'b mut String) -> std::str::Split<'b, char> {
    buf.clear();
    let maker = game.developers.first().or(game.publishers.first());
    match (maker, game.release) {
        (Some(m), Some((y, _, _))) => {
            let _ = write!(buf, "{m} · {y}");
        }
        (Some(m), None) => buf.push_str(m),
        (None, Some((y, _, _))) => {
            let _ = write!(buf, "{y}");
        }
        (None, None) => {}
    }
    buf.push('\n');
    for (i, genre) in game.genres.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        buf.push_str(genre);
    }
    buf.push('\n');
    if game.players > 1 {
        let _ = write!(buf, "Up to {} players", game.players);
    }
    buf.split('\n')
}

/// The game's box art fitted into `r` (centred; at the top if `top`), or a placeholder in its colour
/// while there is none.
fn art(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect, scale: f32, radius: f32, top: bool) {
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    let image = game.box_front.as_deref().and_then(|path| images.get(&game.id, path, pw, ph));
    match image {
        Some(img) => {
            let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
            let y = if top { r.y } else { r.y + (r.h - ih) / 2.0 };
            c.image(img, r.x + (r.w - iw) / 2.0, y);
        }
        None => c.rounded(r, radius, rgb(game.color)),
    }
}
