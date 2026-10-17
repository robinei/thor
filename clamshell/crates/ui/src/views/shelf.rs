//! shelf on the main screen: collections as tabs along the top; the current one's games as a list with
//! box art thumbnails on the left, scrolled to keep the selection on screen; the selected game's large
//! box art and details on the right. Images come from the image cache (placeholders in the game's
//! colour until they arrive).

use std::fmt::{self, Display, Formatter};

use clamshell_core::{Game, Id, ListId, ShelfId};

use crate::draw::{Align, Column, Keep, Rect, Span, WRAP_LINE};
use crate::toolkit::format::{DaysAgo, Fmt, PlayTime};
use crate::toolkit::scroll::Scrolled;
use crate::toolkit::style::{self, Look, RADIUS_L, SURFACE_HI, Tone};
use crate::toolkit::{Frame, widgets};

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
/// A thumbnail placeholder's corner radius.
const THUMB_RADIUS: f32 = 3.0;
/// The large art's height, of the list's height.
const ART_H: f32 = 0.78;
/// Details text sizes, of the screen's height.
const TITLE: f32 = 0.05;
const DETAIL: f32 = 0.03;
const DESCRIPTION: f32 = 0.028;

/// Draws shelf over `screen`; returns how many list rows fit (the state keeps the selection within
/// them).
pub(super) fn draw(f: &mut Frame, screen: Rect) -> usize {
    let s = f.s;
    let (w, h) = (screen.w, screen.h);
    // (The second screen's padding rule, of this screen's height; at least 4 pixels.)
    let pad = style::pad(h).max(4.0);

    // Tabs: the collections' names.
    let tabs_h = h * TABS_H;
    let collections = s.shelf().library().collections.iter().enumerate();
    let items = collections.map(|(i, coll)| (Id::Shelf(ShelfId::Collection(i as u16)), coll.name.as_str(), i == s.shelf().current()));
    widgets::tabs(f, Rect::new(pad, 0.0, w - 2.0 * pad, tabs_h), h * TAB_TEXT, pad * 0.8, f32::INFINITY, items);

    // The list, filling the height below the tabs exactly with whole rows.
    let list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    let rows = ((list.h / (h * ROW_H)).floor() as usize).max(1);
    let Some(coll) = s.shelf().collection() else {
        return rows;
    };
    list_rows(f, list, rows, coll.games.len(), pad);
    if let Some(game) = s.shelf().selected_game() {
        selected(f, game, screen, list, pad);
    }
    rows
}

/// The list (`len` games, `rows` on screen), scrolled as the cursor says, the selection on a background;
/// where in the list: a thin bar along its right edge, when it doesn't all fit.
fn list_rows(f: &mut Frame, list: Rect, rows: usize, len: usize, pad: f32) {
    let s = f.s;
    let row_h = list.h / rows as f32;
    let cursor = s.shelf().cursor();
    let scrolled = Scrolled::begin(f, ListId::Shelf, list, row_h, len);
    for position in scrolled.shown() {
        let Some(game) = s.shelf().game_at(position) else {
            break;
        };
        let at = scrolled.row(position);
        let r = Rect::new(at.x, at.y, list.w - pad * 0.6, row_h - 2.0);
        let selected = position == cursor.index;
        let look = Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD };
        let thumb_h = r.h * THUMB;
        let thumb = Rect::new(r.x + pad * 0.4, r.y + (r.h - thumb_h) / 2.0, thumb_h * BOX_ASPECT, thumb_h);
        f.button(Id::Shelf(ShelfId::Game(game.id.clone())), r, look, |f| {
            widgets::art(f.c, f.images, game, thumb, THUMB_RADIUS, Align::Center, false);
            let tx = thumb.x + thumb.w + pad * 0.6;
            let title = Rect::new(tx, r.y, r.x + r.w - tx - pad * 0.3, r.h);
            f.c.text_fit(title, row_h * ROW_TEXT, Tone::high_if(selected), &game.title, Keep::Start);
        });
    }
    scrolled.end(f, list.x + list.w - 3.0);
}

/// The selected game right of the `list`, as high as it: large art, then beside it the title, what
/// playing it does now, whether it is a favourite, the details and as much of the description as fits.
fn selected(f: &mut Frame, game: &Game, screen: Rect, list: Rect, pad: f32) {
    let s = f.s;
    let h = screen.h;
    let art_h = list.h * ART_H;
    let box_rect = Rect::new(screen.w * LIST_W + pad, list.y, art_h * BOX_ASPECT, art_h);
    widgets::art(f.c, f.images, game, box_rect, RADIUS_L, Align::Start, false);
    let tx = box_rect.x + box_rect.w + pad * 1.2;
    let text_w = screen.w - pad - tx;
    let mut col = Column::new(Rect::new(tx, list.y, text_w, list.h));
    let span = |col: &Column| Span { x: tx, y: col.y(), w: text_w };
    let detail = h * DETAIL;
    col.skip(f.c.wrap(span(&col), h * TITLE, Tone::High, &game.title, 3) + pad * 0.4);

    // Whether it runs (A shows it), then whether it is a favourite, each on a line of its own.
    let mut mx = tx;
    if s.session().running(&game.id).is_some() {
        mx += f.c.text(mx, col.y(), detail, Tone::Go, "Running · A to show it") + pad;
    }
    if mx > tx {
        col.skip(detail * WRAP_LINE + pad * 0.2);
        mx = tx;
    }
    if game.favourite {
        mx += f.c.text(mx, col.y(), detail, Tone::Accent, "Favourite");
    }
    if mx > tx {
        col.skip(detail * WRAP_LINE + pad * 0.4);
    }

    // The details: who made it and when, the genres, the players, the play time.
    let maker = Fmt::<128>::of(format_args!("{}", Maker(game)));
    let genres = Fmt::<128>::of(format_args!("{}", Genres(&game.genres)));
    let players = if game.players > 1 { Fmt::<32>::of(format_args!("Up to {} players", game.players)) } else { Fmt::new() };
    let plays = game.plays;
    let played = if plays.count > 0 {
        let days = s.unix_secs().saturating_sub(plays.last) / 86_400;
        Fmt::<64>::of(format_args!("{}{}", PlayTime(plays.seconds), DaysAgo(days)))
    } else {
        Fmt::new()
    };
    for line in [&*maker, &*genres, &*players, &*played] {
        if !line.is_empty() {
            col.skip(f.c.wrap(span(&col), detail, Tone::Dim, line, 2) + pad * 0.2);
        }
    }

    // As many lines of the description as fit above the list's bottom.
    let text = if game.description.is_empty() { &game.summary } else { &game.description };
    let (top, size) = (col.y() + pad * 0.6, h * DESCRIPTION);
    let lines = ((list.y + list.h - top) / (size * WRAP_LINE)).floor().max(0.0) as usize;
    if !text.is_empty() && lines > 0 {
        f.c.wrap(Span { x: tx, y: top, w: text_w }, size, Tone::Normal, text, lines.min(12));
    }
}

/// Who made a game and when: "Nintendo · 1991", either alone, or nothing.
struct Maker<'a>(&'a Game);

impl Display for Maker<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let game = self.0;
        match (game.developers.first().or(game.publishers.first()), game.release) {
            (Some(m), Some((y, _, _))) => write!(f, "{m} · {y}"),
            (Some(m), None) => f.write_str(m),
            (None, Some((y, _, _))) => write!(f, "{y}"),
            (None, None) => Ok(()),
        }
    }
}

/// The genres, separated by commas.
struct Genres<'a>(&'a [String]);

impl Display for Genres<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for (i, genre) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(genre)?;
        }
        Ok(())
    }
}
