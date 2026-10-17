//! shelf on the main screen: collections as tabs along the top (icon and name); the current list (the
//! collection's entries, or its open group's under a header that goes back) with box art thumbnails on
//! the left, scrolled to keep the selection on screen; the selected entry's large box art and details on
//! the right. Images come from the image cache (placeholders in the entry's colour until they arrive).

use std::fmt::{self, Display, Formatter};

use clamshell_core::{Game, Id, ListId, ShelfId, State, Tab};

use crate::draw::{Align, Column, Keep, Rect, Span, WRAP_LINE};
use crate::icons::Icon;
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
/// A row's marks (favourite, on the dock): their size, of a row's height.
const MARK: f32 = 0.3;
const BOX_ASPECT: f32 = 2.0 / 3.0;
/// A thumbnail placeholder's corner radius.
const THUMB_RADIUS: f32 = 3.0;
/// The large art's height, of the list's height.
const ART_H: f32 = 0.78;
/// Details text sizes, of the screen's height.
const TITLE: f32 = 0.05;
const DETAIL: f32 = 0.03;
const SUMMARY: f32 = 0.028;
/// A screenshot's height at most, of the list's height (it is as wide as the details, 16:9).
const SHOT_H: f32 = 0.45;
/// The box art's width at most, of the room right of the list: a portrait one is as high as it may be
/// within it; a landscape one this wide, the screenshot under it.
const COVER_W: f32 = 0.55;

/// Draws shelf over `screen`; returns how many list rows fit (the state keeps the selection within
/// them).
pub(super) fn draw(f: &mut Frame, screen: Rect) -> usize {
    let s = f.s;
    let (w, h) = (screen.w, screen.h);
    // (The second screen's padding rule, of this screen's height; at least 4 pixels.)
    let pad = style::pad(h).max(4.0);

    // Tabs: the collections' icons and names.
    let tabs_h = h * TABS_H;
    let collections = s.shelf().library().collections.iter().enumerate();
    let items = collections.map(|(i, coll)| {
        let icon = coll.icon.as_deref().and_then(Icon::named);
        (Id::Shelf(ShelfId::Collection(i as u16)), icon, coll.name.as_str(), i == s.shelf().current())
    });
    widgets::tabs(f, Rect::new(pad, 0.0, w - 2.0 * pad, tabs_h), h * TAB_TEXT, pad * 0.8, f32::INFINITY, items);

    // The list, filling the height below the tabs (and an open group's header) exactly with whole rows.
    let mut list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    if let Some(group) = s.shelf().group() {
        let header = list.take_top(h * ROW_H);
        group_header(f, group, header, pad);
    }
    let rows = ((list.h / (h * ROW_H)).floor() as usize).max(1);
    list_rows(f, list, rows, s.shelf().list().len(), pad);
    if let Some(game) = s.shelf().selected_game() {
        selected(f, game, screen, list, pad);
    }
    rows
}

/// An open group's header over the list: a back arrow and the group's title; a tap goes back out.
fn group_header(f: &mut Frame, group: &Game, r: Rect, pad: f32) {
    f.button(Id::Shelf(ShelfId::Up), r, Look::ICON, |f| {
        let arrow = Rect::new(r.x, r.y, r.h, r.h);
        f.c.icon(Icon::ArrowLeft, arrow.square_center(r.h * 0.4), Tone::Normal);
        let title = Rect::new(r.x + r.h + pad * 0.2, r.y, r.w - r.h - pad * 0.2, r.h);
        f.c.text_fit(title, r.h * ROW_TEXT * 1.15, Tone::High, &group.title, Keep::Start);
    });
}

/// The list (`len` entries, `rows` on screen), scrolled as the cursor says, the selection on a background;
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
        // An episode (an entry of a group) played already: dimmer.
        let tone = if selected {
            Tone::High
        } else if game.parent.is_some() && game.plays.count > 0 {
            Tone::Dim
        } else {
            Tone::Normal
        };
        let look = Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD };
        let thumb_h = r.h * THUMB;
        let thumb = Rect::new(r.x + pad * 0.4, r.y + (r.h - thumb_h) / 2.0, thumb_h * BOX_ASPECT, thumb_h);
        f.button(Id::Shelf(ShelfId::Game(game.id.clone())), r, look, |f| {
            widgets::art(f.c, f.images, game, thumb, THUMB_RADIUS, Align::Center, false);
            // Its marks at the row's right end, right to left: on the dock, a favourite.
            let mark = r.h * MARK;
            let mut right = r.x + r.w - pad * 0.5;
            let marks = [(s.dock().is_pinned(&game.id), Icon::for_tab(Tab::Dock), Tone::Normal), (game.favourite, Icon::Star, Tone::Accent)];
            for (on, icon, mark_tone) in marks {
                if on {
                    right -= mark;
                    f.c.icon(icon, Rect::new(right, r.y + (r.h - mark) / 2.0, mark, mark), mark_tone);
                    right -= pad * 0.4;
                }
            }
            let tx = thumb.x + thumb.w + pad * 0.6;
            let title = Rect::new(tx, r.y, right - tx - pad * 0.3, r.h);
            f.c.text_fit(title, row_h * ROW_TEXT, tone, &game.title, Keep::Start);
        });
    }
    scrolled.end(f, list.x + list.w - 3.0);
}

/// The selected entry right of the `list`, as high as it: large box art, then beside it the title, the
/// groups it is in, who made it and when, what playing it does now, whether it is a favourite and on the
/// dock, the play time, what it is about (as much as fits), and a screenshot: at the bottom of the text,
/// or under a landscape box (as wide as it).
fn selected(f: &mut Frame, game: &Game, screen: Rect, list: Rect, pad: f32) {
    let s = f.s;
    let h = screen.h;
    let x0 = screen.w * LIST_W + pad;
    let room = Rect::new(x0, list.y, (screen.w - pad - x0) * COVER_W, list.h * ART_H);
    let cover = widgets::cover(f.c, f.images, game, room, RADIUS_L, BOX_ASPECT);
    let landscape = cover.w > cover.h;
    let tx = if landscape { room.x + room.w } else { cover.x + cover.w } + pad * 1.2;
    let text_w = screen.w - pad - tx;
    let mut col = Column::new(Rect::new(tx, list.y, text_w, list.h));
    let span = |col: &Column| Span { x: tx, y: col.y(), w: text_w };
    let detail = h * DETAIL;
    col.skip(f.c.wrap(span(&col), h * TITLE, Tone::High, &game.title, 3) + pad * 0.4);

    // The groups it is in (a series, a season); who made it and when.
    if game.parent.is_some() {
        let groups = Fmt::<192>::of(format_args!("{}", Groups(s, game)));
        col.skip(f.c.wrap(span(&col), detail, Tone::Normal, &groups, 2) + pad * 0.4);
    }
    let maker = Fmt::<128>::of(format_args!("{}", Maker(game)));
    if !maker.is_empty() {
        col.skip(f.c.wrap(span(&col), detail, Tone::Dim, &maker, 2) + pad * 0.4);
    }

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
        mx += f.c.text(mx, col.y(), detail, Tone::Accent, "Favourite") + pad;
    }
    if s.dock().is_pinned(&game.id) {
        mx += f.c.text(mx, col.y(), detail, Tone::Dim, "On the dock");
    }
    if mx > tx {
        col.skip(detail * WRAP_LINE + pad * 0.4);
    }

    // The play time.
    let plays = game.plays;
    if plays.count > 0 {
        let days = s.unix_secs().saturating_sub(plays.last) / 86_400;
        let played = Fmt::<64>::of(format_args!("{}{}", PlayTime(plays.seconds), DaysAgo(days)));
        col.skip(f.c.wrap(span(&col), detail, Tone::Dim, &played, 2) + pad * 0.4);
    }

    // A screenshot: under a landscape box, as wide; else at the bottom of the text. What it is about in as
    // many lines as fit above that.
    let shot = match (game.shot.is_some(), landscape) {
        (false, _) => None,
        (true, true) => {
            let y = cover.y + cover.h + pad;
            // (4:3 room: a console's screenshots fill the width; a wider one fits in it.)
            Some(Rect::new(room.x, y, room.w, (room.w * 3.0 / 4.0).min(list.y + list.h - y)))
        }
        (true, false) => {
            let shot_h = (text_w * 9.0 / 16.0).min(list.h * SHOT_H);
            Some(Rect::new(tx, list.y + list.h - shot_h, text_w, shot_h))
        }
    };
    let bottom = match shot {
        Some(r) if !landscape => r.y - pad,
        _ => list.y + list.h,
    };
    let (top, size) = (col.y() + pad * 0.2, h * SUMMARY);
    let lines = ((bottom - top) / (size * WRAP_LINE)).floor().max(0.0) as usize;
    if !game.summary.is_empty() && lines > 0 {
        f.c.wrap(Span { x: tx, y: top, w: text_w }, size, Tone::Normal, &game.summary, lines.min(12));
    }
    if let Some(r) = shot.filter(|r| r.h > pad) {
        widgets::shot(f.c, f.images, game, r);
    }
}

/// Who made an entry and when: "Supergiant Games · 2020", either alone, or nothing.
struct Maker<'a>(&'a Game);

impl Display for Maker<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let game = self.0;
        match (game.developer.as_str(), game.year) {
            ("", Some(y)) => write!(f, "{y}"),
            ("", None) => Ok(()),
            (d, Some(y)) => write!(f, "{d} · {y}"),
            (d, None) => f.write_str(d),
        }
    }
}

/// The groups an entry is in, outermost first: "Cosmos · Season 1".
struct Groups<'a>(&'a State, &'a Game);

impl Display for Groups<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let games = &self.0.shelf().library().games;
        // (At most a few levels: walked from the entry up, written from the top down.)
        let mut chain = [None; 8];
        let mut at = self.1.parent;
        let mut n = 0;
        while let (Some(i), Some(slot)) = (at, chain.get_mut(n)) {
            *slot = Some(i);
            n += 1;
            at = games.get(i).and_then(|g| g.parent);
        }
        for (k, i) in chain.iter().take(n).rev().flatten().enumerate() {
            if k > 0 {
                f.write_str(" · ")?;
            }
            f.write_str(games.get(*i).map_or("", |g| g.title.as_str()))?;
        }
        Ok(())
    }
}
