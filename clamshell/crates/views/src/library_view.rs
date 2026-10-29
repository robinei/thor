//! The Library on the main screen. The collections are tabs along the top, each an icon and a name. The current
//! list is on the left with box art thumbnails, scrolled to keep the selection on screen. It holds the
//! collection's entries, or an open group's entries under a header that goes back. The selected entry's
//! large box art and details are on the right. Images come from the image cache, with placeholders in
//! the entry's colour until they arrive.

use std::fmt::{self, Display, Formatter};

use clamshell_state::{Entry, Event, Id, Launch, LibraryViewEvent, LibraryViewId, ListId, PointerEvent, State};

use clamshell_ui::gfx::icons::Icon;
use clamshell_ui::gfx::{Column, Keep, Rect, Span, WRAP_LINE};
use clamshell_ui::input::nav::Dir;
use clamshell_ui::toolkit::format::{Bytes, DaysAgo, Fmt, PlayTime};
use clamshell_ui::toolkit::scroll::Scrolled;
use clamshell_ui::toolkit::style::{self, Look, RADIUS_L, SURFACE_HI, Tone};
use clamshell_ui::toolkit::{Frame, PadMove, PadUse, widgets};

/// The tab bar's height and text size, as fractions of the screen's height.
pub(super) const TABS_H: f32 = 0.08;
pub(super) const TAB_TEXT: f32 = 0.034;
/// The list's share of the width.
pub(super) const LIST_W: f32 = 0.42;
/// The target row height, as a fraction of the screen's height. Rows are stretched to fill the list
/// exactly.
pub(super) const ROW_H: f32 = 0.085;
/// Title size and thumbnail height (`THUMB`), as fractions of a row's height.
pub(super) const ROW_TEXT: f32 = 0.36;
/// The text size of the filter and an open group's header, a little larger than a row's.
pub(super) const HEADER_TEXT: f32 = ROW_TEXT * 1.15;
const THUMB: f32 = 0.82;
/// The size of a row's marks (favourite, on the dock, running), as a fraction of a row's height.
const MARK: f32 = 0.3;
const BOX_ASPECT: f32 = 2.0 / 3.0;
/// A row's picture is square, with box art and video frames cropped to fill it.
const ROW_ASPECT: f32 = 1.0;
/// A thumbnail placeholder's corner radius.
const THUMB_RADIUS: f32 = 3.0;
/// The large art's height, as a fraction of the list's height.
const ART_H: f32 = 0.78;
/// Details text sizes, as fractions of the screen's height.
pub(super) const TITLE: f32 = 0.05;
pub(super) const DETAIL: f32 = 0.03;
const SUMMARY: f32 = 0.028;
/// An action row's height, as a multiple of the details' text size.
pub(super) const ACTION: f32 = 1.45;
/// The gap after each paragraph of the details, as a fraction of the padding.
const PARA_GAP: f32 = 0.4;
/// The largest screenshot height, as a fraction of the list's height. The screenshot is as wide as the
/// details, at 16:9.
const SHOT_H: f32 = 0.45;
/// The largest box art width, as a fraction of the room right of the list. Portrait art is as tall as
/// fits within it. Landscape art is this wide, with the screenshot under it.
const COVER_W: f32 = 0.55;

/// Draws the Library over `screen` and returns how many list rows fit. The state keeps the selection within
/// them.
/// What the pad's moves do on the Library's list (see `clamshell_ui::toolkit::PadOut::moves`). Up and down
/// step through the games, scrolling it, counted from the focused game, which isn't the selected one when
/// the focus came up from the second screen; down past the last goes on by direction, to the second
/// screen. Left and right switch tabs, or picking, where the Files tab is the only one, go out of the open
/// folder and into the selected one. A shoulder moves a page, the rows on screen; a trigger to an end.
fn moves(s: &State, m: PadMove) -> Option<Option<Event>> {
    let lv = s.library_view();
    let (len, cursor) = (lv.list().len(), lv.cursor().index);
    let focused = match s.pointer().focus() {
        Some(Id::LibraryView(LibraryViewId::Entry(id))) => (0..len).find(|&i| lv.entry_at(i).is_some_and(|e| &e.id == id)),
        _ => None,
    };
    let from = focused.unwrap_or(cursor);
    let step = |n: i64| Some(Some(LibraryViewEvent::ListStep(i32::try_from(n).unwrap_or(0)).into()));
    let to = |n: usize| step(n as i64 - cursor as i64);
    let picking = lv.pick().is_some();
    match m {
        PadMove::Dir(Dir::Up) => to(from.saturating_sub(1)),
        PadMove::Dir(Dir::Down) if from + 1 < len => to(from + 1),
        PadMove::Dir(Dir::Down) => None,
        PadMove::Dir(Dir::Left) if picking => Some(lv.group().map(|_| PointerEvent::Activate(Id::LibraryView(LibraryViewId::Up)).into())),
        // Pressing the selected entry opens it.
        PadMove::Dir(Dir::Right) if picking => {
            Some(lv.entry_at(from).filter(|e| e.is_group()).map(|e| PointerEvent::Activate(Id::LibraryView(LibraryViewId::Entry(e.id.clone()))).into()))
        }
        PadMove::Dir(Dir::Left) => Some(Some(LibraryViewEvent::CollectionStep(-1).into())),
        PadMove::Dir(Dir::Right) => Some(Some(LibraryViewEvent::CollectionStep(1).into())),
        PadMove::Page(n) => step(i64::from(n) * lv.rows().max(1) as i64),
        PadMove::End(n) => step(i64::from(n) * len as i64),
        PadMove::Scroll(n) => step(i64::from(n)),
    }
}

/// The Library as an app (see `super::apps`): its edit mode, or its list, which a pick shows too.
pub(super) fn app(f: &mut Frame, screen: Rect) {
    let rows = match f.s.library_view().edit().filter(|_| f.s.library_view().pick().is_none()) {
        Some(edit) => {
            f.start(clamshell_state::library_view::edit::EditId::Row(edit.cursor() as u16).into());
            super::library_edit::draw(f, screen, edit)
        }
        None => draw(f, screen),
    };
    // The state keeps the selection within the rows on screen.
    if rows != f.s.library_view().rows() {
        f.report(LibraryViewEvent::Rows(rows).into());
    }
}

pub(super) fn draw(f: &mut Frame, screen: Rect) -> usize {
    let s = f.s;
    let (w, h) = (screen.w, screen.h);
    // The second screen's padding rule applied to this screen's height, at least 4 pixels.
    let pad = style::pad(h).max(4.0);

    // Tabs: the collections' icons and names.
    let tabs_h = h * TABS_H;
    // Picking, the Files tab alone.
    let picking = s.library_view().pick().is_some();
    // The pad starts on the selection. B goes back from the details' actions to it, clears what was typed,
    // or goes out of the open group; picking, it cancels (the header's B), and Left goes out instead.
    let selection = s.library_view().selected().map(|id| Id::LibraryView(LibraryViewId::Entry(id.clone())));
    if let Some(id) = selection.clone() {
        f.start(id);
    }
    if !picking {
        if s.library_view().in_actions(s.pointer()) {
            if let Some(id) = selection {
                f.back_with(PointerEvent::Focus(id).into());
            }
        } else if !s.library_view().filter().is_empty() {
            f.back_with(LibraryViewEvent::ClearFilter.into());
        } else if s.library_view().group().is_some() {
            f.back(Id::LibraryView(LibraryViewId::Up));
        }
    }
    let collections = s.library_view().library().collections.iter().enumerate().filter(|(i, _)| !picking || *i == s.library_view().current());
    let items = collections.map(|(i, coll)| {
        let icon = coll.icon.as_deref().and_then(Icon::named);
        (Id::LibraryView(LibraryViewId::Collection(i as u16)), icon, coll.name.as_str(), i == s.library_view().current())
    });
    let first = f.targets_drawn();
    let tabs_end = widgets::tabs(f, Rect::new(pad, 0.0, w - 2.0 * pad, tabs_h), h * TAB_TEXT, pad * 0.8, f32::INFINITY, items);
    // The tabs are the list's, whose moves are its own (see `moves`).
    f.pad_use_since(first, PadUse::List);

    // The first problem found reading the library, such as in library.json or a missing folder, shown at
    // the right end.
    let problems = &s.library_view().library().problems;
    // Picking, what is asked for and Cancel take their place; saving, also the name (Save as) and Save
    // here, dimmed while no folder it can go in is open. Each shows the pad's button for it, as the
    // details' actions do: B, Start and Y.
    if let Some(pick) = s.library_view().pick() {
        let size = h * DETAIL;
        // The actions a row high, in the middle of the tabs' height.
        let (row_h, row_y) = (size * 1.6, (tabs_h - size * 1.6) / 2.0);
        let cancel_w = widgets::key_action_width(f, row_h, "B", "Cancel");
        let cancel = Rect::new(w - pad - cancel_w, row_y, cancel_w, row_h);
        widgets::key_action(f, Id::LibraryView(LibraryViewId::CancelPick), cancel, "B", "Cancel", Tone::Fn, false);
        let mut end = cancel.x;
        if pick.saves() {
            use clamshell_state::library_view::pick::SaveHere;
            let here = s.library_view().save_here();
            let label = match here {
                SaveHere::Replaces { first, more: 0, asked: true } => Fmt::<192>::of(format_args!("Replace {first}?")),
                SaveHere::Replaces { first, more, asked: true } => Fmt::of(format_args!("Replace {first} and {more} more?")),
                _ => Fmt::of(format_args!("Save here")),
            };
            // At most half the room between the tab and Cancel: a long name is cut, not drawn over the rest.
            let room = end - tabs_end - pad * 3.0;
            let save_w = widgets::key_action_width(f, row_h, "Start", &label).min(room * 0.5);
            let save = Rect::new(end - pad * 2.0 - save_w, row_y, save_w, row_h);
            let tone = if here == SaveHere::Off { Tone::Dim } else { Tone::Accent };
            widgets::key_action(f, Id::LibraryView(LibraryViewId::SaveHere), save, "Start", &label, tone, here == SaveHere::Off);
            end = save.x;
        }
        let asks = Rect::new(tabs_end + pad * 2.0, 0.0, end - tabs_end - pad * 3.0, tabs_h);
        match &pick.takes {
            clamshell_state::library_view::pick::Takes::Save { name, .. } => {
                let text = Fmt::<192>::of(format_args!("Save as: {name} \u{270e}"));
                let save_as = Rect::new(asks.x, row_y, widgets::key_action_width(f, row_h, "Y", &text).min(asks.w), row_h);
                widgets::key_action(f, Id::LibraryView(LibraryViewId::SaveAs), save_as, "Y", &text, Tone::Fn, false);
            }
            _ => {
                f.c.text_fit(asks, size, Tone::Accent, pick.asks(), Keep::End);
            }
        }
    } else if let Some(first) = problems.first() {
        let more = Fmt::<16>::of(format_args!(" (+{})", problems.len() - 1));
        let text = Fmt::<256>::of(format_args!("{first}{}", if problems.len() > 1 { &*more } else { "" }));
        let x = tabs_end + pad * 2.0;
        let size = h * DETAIL;
        let tw = f.c.text_width(size, &text).min(w - pad - x);
        if tw > size * 4.0 {
            f.c.text_fit(Rect::new(w - pad - tw, 0.0, tw + 1.0, tabs_h), size, Tone::Accent, &text, Keep::Start);
        }
    }

    // The list fills the height below the tabs, and below an open group's header, exactly with whole rows.
    let mut list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    if let Some(group) = s.library_view().group() {
        let header = list.take_top(h * ROW_H);
        group_header(f, group, header, pad);
    }
    // The filter text typed on the bottom screen's keyboard to find an entry, over the list.
    let filter = s.library_view().filter();
    if !filter.is_empty() {
        let line = list.take_top(h * ROW_H * 0.7);
        let size = line.h * HEADER_TEXT;
        let text = Fmt::<96>::of(format_args!("{filter}…"));
        f.c.text_fit(Rect::new(line.x + pad * 0.4, line.y, line.w - pad, line.h), size, Tone::Accent, &text, Keep::End);
        if s.library_view().list().is_empty() {
            let none = Fmt::<128>::of(format_args!("No title starts with \u{201c}{filter}\u{201d}"));
            f.c.text_fit(Rect::new(list.x + pad * 0.4, list.y, list.w - pad, h * ROW_H), size, Tone::Dim, &none, Keep::Start);
        }
    }
    let rows = ((list.h / (h * ROW_H)).floor() as usize).max(1);
    let first = f.targets_drawn();
    list_rows(f, list, rows, s.library_view().list().len(), pad);
    f.pad_use_since(first, PadUse::List);
    f.list_moves(moves);
    // When there is nothing but ClamShell's own tools, say where games and apps come from, in the middle
    // of the screen. Not while the library is still being read at start, when the Library is empty for a moment.
    if s.library_view().read() && s.library_view().library().entries.iter().all(|g| matches!(g.launch, Launch::Tool(_) | Launch::Builtin(_))) {
        let line = Rect::new(pad, h * 0.5 - h * ROW_H, w - 2.0 * pad, h * ROW_H);
        widgets::centred_text(f, line, h * DETAIL, Tone::Dim, "No games or apps found: see ~/.config/clamshell/library.json");
    }
    // The selected entry. In an empty browsed folder, the folder itself, for its actions (paste, new
    // folder).
    if let Some((entry, _)) = s.library_view().file_target().filter(|(g, selected)| *selected || g.browsed.is_some()) {
        selected(f, entry, screen, list, pad);
    }
    rows
}

/// An open group's header over the list: a back arrow and the open groups, outermost first ("Handhelds ›
/// GBA"), cut at the start when it doesn't fit. A tap goes back out.
fn group_header(f: &mut Frame, group: &Entry, r: Rect, pad: f32) {
    let s = f.s;
    f.button(Id::LibraryView(LibraryViewId::Up), r, Look::ICON, |f| {
        let arrow = Rect::new(r.x, r.y, r.h, r.h);
        f.c.icon(Icon::ArrowLeft, arrow.square_center(r.h * 0.4), Tone::Normal);
        let title = Rect::new(r.x + r.h + pad * 0.2, r.y, r.w - r.h - pad * 0.2, r.h);
        let path = Fmt::<192>::of(format_args!("{}", Groups { s, entry: group, own: true, between: " › " }));
        f.c.text_fit(title, r.h * HEADER_TEXT, Tone::High, &path, Keep::End);
    });
}

/// The list of `len` entries with `rows` on screen, scrolled as the cursor says, with the selection on a
/// background. When it doesn't all fit, a thin bar along its right edge shows the position.
fn list_rows(f: &mut Frame, list: Rect, rows: usize, len: usize, pad: f32) {
    let s = f.s;
    let row_h = list.h / rows as f32;
    let cursor = s.library_view().cursor();
    let scrolled = Scrolled::begin(f, ListId::LibraryView, list, row_h, len);
    for position in scrolled.shown() {
        let Some(entry) = s.library_view().entry_at(position) else {
            break;
        };
        let at = scrolled.row(position);
        let r = Rect::new(at.x, at.y, list.w - pad * 0.6, row_h - 2.0);
        let selected = position == cursor.index;
        // A watched video, to its end or into its credits, is dimmer, as is a folder with nothing in it.
        let tone = if selected {
            Tone::High
        } else if (entry.mpv_file().is_some() && entry.watched()) || entry.is_empty_folder() {
            Tone::Dim
        } else if s.library_view().pick().is_some_and(|p| !entry.is_group() && !p.takes(entry)) {
            // Picking, a file it doesn't take.
            Tone::Dim
        } else {
            Tone::Normal
        };
        let look = Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD };
        // Every row's picture is a square, cropped to fill it, so that the titles line up.
        let thumb_h = r.h * THUMB;
        let thumb = Rect::new(r.x + pad * 0.4, r.y + (r.h - thumb_h) / 2.0, thumb_h * ROW_ASPECT, thumb_h);
        f.button(Id::LibraryView(LibraryViewId::Entry(entry.id.clone())), r, look, |f| {
            widgets::thumb(f.c, f.images, entry, thumb, THUMB_RADIUS, false);
            // The marks at the row's right end, from right to left: a folder's chevron, running (dim
            // while starting or paused), on the dock, and favourite.
            let mark = r.h * MARK;
            let mut right = r.x + r.w - pad * 0.5;
            let starting = s.session().starting(&entry.id);
            let paused = starting || s.session().paused(&entry.id) || s.session().suspended(&entry.id);
            let marks = [
                (entry.is_group(), Icon::Chevron, Tone::Normal),
                (starting || s.session().runs(&entry.id), Icon::Dot, if paused { Tone::Dim } else { Tone::Go }),
                (s.library_view().files().selected().contains(&entry.id), Icon::Check, Tone::Accent),
                (s.dock().is_pinned(&entry.id), Icon::Apps, Tone::Normal),
                (entry.favourite, Icon::Star, Tone::Accent),
            ];
            for (on, icon, mark_tone) in marks {
                if on {
                    right -= mark;
                    f.c.icon(icon, Rect::new(right, r.y + (r.h - mark) / 2.0, mark, mark), mark_tone);
                    right -= pad * 0.4;
                }
            }
            let tx = thumb.x + thumb.w + pad * 0.6;
            let title = Rect::new(tx, r.y, right - tx - pad * 0.3, r.h);
            // Outside its folder, as in Recent or Favourites, the folders it is in go before its title.
            let open = s.library_view().group().map(|g| &g.id);
            let in_view = entry.parent.and_then(|p| s.library_view().library().entries.get(p)).is_none_or(|p| Some(&p.id) == open);
            if in_view {
                f.c.text_fit(title, row_h * ROW_TEXT, tone, &entry.title, Keep::Start);
            } else {
                let path = Fmt::<256>::of(format_args!("{}", Groups { s, entry, own: true, between: " · " }));
                f.c.text_fit(title, row_h * ROW_TEXT, tone, &path, Keep::Start);
            }
            // For a video under way, how far in it is, under its title.
            if let Some(fraction) = watched_fraction(entry).filter(|_| !entry.watched()) {
                let bar = Rect::new(tx, r.y + r.h * 0.8, (title.w * 0.6).min(row_h * 4.0), (r.h * 0.05).max(2.0));
                widgets::progress(f.c, bar, fraction);
            }
        });
    }
    scrolled.end(f, list.x + list.w - 3.0);
}

/// The selected entry, right of the `list` and as tall as it. Large box art comes first. Beside it are
/// the title, the groups it is in, who made it and when, what playing it does now, whether it is a
/// favourite and on the dock, the play time, and as much of its summary as fits. A screenshot goes at the
/// bottom of the text, or under landscape box art at the same width.
fn selected(f: &mut Frame, entry: &Entry, screen: Rect, list: Rect, pad: f32) {
    let s = f.s;
    let h = screen.h;
    let x0 = screen.w * LIST_W + pad;
    let room = Rect::new(x0, list.y, (screen.w - pad - x0) * COVER_W, list.h * ART_H);
    let cover = widgets::cover(f.c, f.images, entry, room, RADIUS_L, BOX_ASPECT);
    let landscape = cover.w > cover.h;
    let tx = if landscape { room.x + room.w } else { cover.x + cover.w } + pad * 1.2;
    let text_w = screen.w - pad - tx;
    let mut col = Column::new(Rect::new(tx, list.y, text_w, list.h));
    // A paragraph of the details, wrapped in up to `lines` lines at the column's position, then a gap.
    let para = |f: &mut Frame, col: &mut Column, size: f32, tone: Tone, text: &str, lines: usize| {
        col.skip(f.c.wrap(Span { x: tx, y: col.y(), w: text_w }, size, tone, text, lines) + pad * PARA_GAP);
    };
    let detail = h * DETAIL;
    para(f, &mut col, h * TITLE, Tone::High, &entry.title, 3);

    // The groups it is in (a series, a season), and who made it and when.
    if entry.parent.is_some() {
        let groups = Fmt::<192>::of(format_args!("{}", Groups { s, entry, own: false, between: " · " }));
        para(f, &mut col, detail, Tone::Normal, &groups, 2);
    }
    let maker = Fmt::<128>::of(format_args!("{}", Maker(entry)));
    if !maker.is_empty() {
        para(f, &mut col, detail, Tone::Dim, &maker, 2);
    }

    // A browsed file: its size and when it was changed.
    if let Some(b) = entry.browsed.as_ref().filter(|b| !b.folder && b.modified > 0) {
        let days = s.clock().unix().saturating_sub(b.modified) / 86_400;
        let facts = Fmt::<64>::of(format_args!("{}{}", Bytes(b.size.unwrap_or(0)), DaysAgo(days)));
        para(f, &mut col, detail, Tone::Dim, &facts, 1);
    }

    // A folder: what is in it. Not for a browsed folder, since what is known of it may be out of date.
    if entry.is_group() && entry.browsed.is_none() {
        let inside = Fmt::<64>::of(format_args!("{}", Inside(s, entry)));
        para(f, &mut col, detail, Tone::Normal, &inside, 2);
    }

    // A video: how far in it is, and how long it is.
    if let Some(at) = entry.playback.position.filter(|_| entry.mpv_file().is_some()) {
        let minutes = Fmt::<48>::of(format_args!("{}", Minutes(at, entry.playback.duration)));
        let bar_w = (text_w * 0.45).min(detail * 9.0);
        if let Some(fraction) = watched_fraction(entry) {
            let bar = Rect::new(tx, col.y() + detail * 0.42, bar_w, (detail * 0.22).max(3.0));
            widgets::progress(f.c, bar, fraction);
        }
        let x = if watched_fraction(entry).is_some() { tx + bar_w + pad * 0.6 } else { tx };
        f.c.text(x, col.y(), detail, Tone::Normal, &minutes);
        col.skip(detail * WRAP_LINE + pad * PARA_GAP);
    } else if entry.mpv_file().is_some() && entry.watched() {
        para(f, &mut col, detail, Tone::Dim, "Watched", 1);
    }

    // Its actions (see clamshell-state's `actions`), each a row to tap, with its button.
    let action_h = detail * ACTION;
    let rows = actions(f, entry, Rect::new(tx, col.y(), text_w, list.y + list.h - col.y()), action_h, pad);
    col.skip(rows);
    col.skip(pad * 0.3);

    // The play time.
    let plays = entry.plays;
    if plays.count > 0 {
        let days = s.clock().unix().saturating_sub(plays.last) / 86_400;
        let played = Fmt::<64>::of(format_args!("{}{}", PlayTime(plays.seconds), DaysAgo(days)));
        para(f, &mut col, detail, Tone::Dim, &played, 2);
    }

    // A screenshot goes under landscape box art at the same width, or else at the bottom of the text. The
    // summary takes as many lines as fit above that. There is no screenshot if it is already the picture.
    let shot = match (entry.details.shot.is_some() && !widgets::wide_picture(entry), landscape) {
        (false, _) => None,
        (true, true) => {
            let y = cover.y + cover.h + pad;
            // 4:3 room, so a console's screenshots fill the width and a wider one fits inside it.
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
    if !entry.details.summary.is_empty() && lines > 0 {
        f.c.wrap(Span { x: tx, y: top, w: text_w }, size, Tone::Normal, &entry.details.summary, lines.min(12));
    }
    if let Some(r) = shot.filter(|r| r.h > pad) {
        widgets::shot(f.c, f.images, entry, r);
    }
}

/// Draws the details' actions as rows from the top of `r`, each with its pad button, and returns their
/// height. A opens (plays, or shows it running), Start continues a folder of videos, and Y and X toggle
/// favourite and on the dock, or Copy and Paste for a browsed file. Then come the file actions, where
/// Delete and Empty Trash ask to be pressed again, and how far a paste under way has got.
fn actions(f: &mut Frame, entry: &Entry, r: Rect, action_h: f32, pad: f32) -> f32 {
    use clamshell_state::library_view::actions::{Action, Shortcut};
    use clamshell_state::library_view::files::FileAction;
    let s = f.s;
    let mut y = r.y;
    // A row is fourteen times the details' text size wide. Continue's row, which names the next video,
    // is as wide as the details.
    let w = r.w.min(action_h / ACTION * 14.0);
    let shortcut = |key| s.library_view().button_of(key);
    // While the pad is on the actions themselves, their shortcuts are off and faded, and A and B are
    // explained below.
    let in_actions = s.library_view().in_actions(s.pointer());
    for &action in s.library_view().actions().as_slice() {
        let key = match action {
            Action::Open => "A",
            _ if shortcut(Shortcut::Start) == Some(action) => "Start",
            _ if shortcut(Shortcut::Y) == Some(action) => "Y",
            _ if shortcut(Shortcut::X) == Some(action) => "X",
            _ => "",
        };
        let running = s.session().runs(&entry.id);
        let starting = s.session().starting(&entry.id);
        let (label, tone) = match action {
            Action::Choose if entry.is_group() => (Fmt::<192>::of(format_args!("Choose this folder")), Tone::Accent),
            // Saving over a file: Replace, then asked again.
            Action::Choose if s.library_view().pick().is_some_and(|p| p.saves()) => {
                let asked = s.library_view().pick().zip(entry.file.as_deref()).is_some_and(|(p, file)| p.confirming(file));
                if asked { (Fmt::of(format_args!("Replace {}?", entry.title)), Tone::Accent) } else { (Fmt::of(format_args!("Replace")), Tone::Accent) }
            }
            Action::Choose => (Fmt::<192>::of(format_args!("Choose")), Tone::Accent),
            Action::Open if entry.is_group() => (Fmt::<192>::of(format_args!("Open")), Tone::Normal),
            // A launcher's game says in which launcher it starts, or that the launcher starts first.
            Action::Open if starting => match s.session().starts_in(entry, s.library_view().library()) {
                Some((launcher, true)) => (Fmt::of(format_args!("Starting · in {launcher}")), Tone::Dim),
                Some((launcher, false)) => (Fmt::of(format_args!("Starting · {launcher} first")), Tone::Dim),
                None => (Fmt::of(format_args!("Starting…")), Tone::Dim),
            },
            Action::Open if running => (Fmt::of(format_args!("Running · show it")), Tone::Go),
            Action::Open if matches!(entry.launch, clamshell_state::Launch::Tool(_)) => (Fmt::of(format_args!("Run")), Tone::Normal),
            Action::Open if entry.browsed.is_some() => (Fmt::of(format_args!("Open")), Tone::Normal),
            Action::Open => (Fmt::of(format_args!("Play")), Tone::Normal),
            Action::Continue => {
                // The position within it is mpv's.
                let library = s.library_view().library();
                let next = library.entries.iter().position(|g| g.id == entry.id).and_then(|g| library.to_continue(g)).and_then(|i| library.entries.get(i));
                match next {
                    Some(n) => (Fmt::of(format_args!("Play · {}", Within(s, n, entry))), Tone::Go),
                    None => (Fmt::of(format_args!("Play")), Tone::Go),
                }
            }
            Action::Enqueue if !s.library_view().files().selected().is_empty() => {
                (Fmt::of(format_args!("Enqueue · {} selected", s.library_view().files().selected().len())), Tone::Normal)
            }
            Action::Enqueue => (Fmt::of(format_args!("Enqueue")), Tone::Normal),
            Action::Favourite if entry.favourite => (Fmt::of(format_args!("Favourite")), Tone::Accent),
            Action::Favourite => (Fmt::of(format_args!("Not a favourite")), Tone::Dim),
            Action::Pin if s.dock().is_pinned(&entry.id) => (Fmt::of(format_args!("On the dock")), Tone::Normal),
            // It always runs, so it is always on the dock, pinned or not.
            Action::Pin => (Fmt::of(format_args!("Not on the dock")), Tone::Dim),
            Action::File(a) if s.library_view().file_armed(a) => (Fmt::of(format_args!("{} · press again", a.label())), Tone::Accent),
            Action::File(a) => {
                let files = s.library_view().files();
                let clip = files.clip();
                let selected = files.selected().len();
                let label = match (a, clip) {
                    (FileAction::Paste, Some(clip)) if clip.entries.len() > 1 => Fmt::of(format_args!("Paste · {} entries", clip.entries.len())),
                    (FileAction::Paste, Some(clip)) => Fmt::of(format_args!("Paste · {}", clip.entries.first().map_or("", |(_, n)| n.as_str()))),
                    (FileAction::Select, _) if files.selected().contains(&entry.id) => Fmt::of(format_args!("Selected · {selected}")),
                    (FileAction::ShowHidden, _) => Fmt::of(format_args!("{} hidden files", if files.hidden() { "Hide" } else { "Show" })),
                    (FileAction::OpenIn, _) => Fmt::of(format_args!("Open in {}", s.library_view().opened_in(entry).map_or("", |r| &*r.name))),
                    (FileAction::Copy | FileAction::Cut | FileAction::Trash | FileAction::Restore | FileAction::Delete, _) if selected > 0 => {
                        Fmt::of(format_args!("{} · {selected} selected", a.label()))
                    }
                    (FileAction::Copy | FileAction::Cut, Some(clip))
                        if clip.entries.iter().any(|(p, _)| Some(p) == entry.file.as_ref()) && (a == FileAction::Cut) == clip.cut =>
                    {
                        Fmt::of(format_args!("{} · {}", a.label(), if clip.cut { "cut" } else { "copied" }))
                    }
                    _ => Fmt::of(format_args!("{}", a.label())),
                };
                (label, if a.confirms() { Tone::Accent } else { Tone::Normal })
            }
        };
        let row_w = if action == Action::Continue { r.w } else { w };
        widgets::key_action(f, Id::LibraryView(action.id()), Rect::new(r.x, y, row_w, action_h), key, &label, tone, in_actions);
        f.pad_use(PadUse::Held);
        y += action_h + pad * 0.15;
    }
    if let Some(clamshell_state::library_view::files::Busy { what, fraction }) = s.library_view().files().busy() {
        y += pad * 0.3;
        let size = action_h * 0.62;
        f.c.text_fit(Rect::new(r.x, y, r.w, size * 1.4), size, Tone::Normal, what, Keep::Start);
        y += size * 1.6;
        widgets::progress(f.c, Rect::new(r.x, y, w, (size * 0.22).max(3.0)), *fraction);
        y += size;
    }
    // While the pad is on them, what its buttons do now is shown at the bottom, and the details' text
    // below the actions gives way to it.
    if in_actions {
        let line = Rect::new(r.x, r.y + r.h - action_h, w, action_h);
        let confirm = widgets::key_hint(f, line, "A", "Confirm");
        widgets::key_hint(f, Rect::new(line.x + confirm + pad, line.y, line.w - confirm - pad, line.h), "B", "Back");
        return r.h;
    }
    y - r.y
}

/// How far into a video, as a fraction of its length, if both are known.
fn watched_fraction(entry: &Entry) -> Option<f32> {
    let (at, length) = (entry.playback.position?, entry.playback.duration?);
    (length > 0).then(|| at as f32 / length as f32)
}

/// Minutes into a video and its length: "23 of 45 min", or "23 min in" without the length.
struct Minutes(u32, Option<u32>);

impl Display for Minutes {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.1 {
            Some(length) => write!(f, "{} of {} min", self.0 / 60, length.div_ceil(60)),
            None => write!(f, "{} min in", self.0 / 60),
        }
    }
}

/// An entry as seen from a folder it is in: the folders between, then its title ("Season 1 · 2. One
/// Voice").
struct Within<'a>(&'a State, &'a Entry, &'a Entry);

impl Display for Within<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let entries = &self.0.library_view().library().entries;
        // The folders from the entry up to the folder, collected bottom up and written top down.
        let mut chain = [None; 8];
        let mut at = self.1.parent;
        let mut n = 0;
        while let (Some(i), Some(slot)) = (at, chain.get_mut(n)) {
            if entries.get(i).is_some_and(|g| g.id == self.2.id) {
                break;
            }
            *slot = Some(i);
            n += 1;
            at = entries.get(i).and_then(|g| g.parent);
        }
        for i in chain.iter().take(n).rev().flatten() {
            write!(f, "{} · ", entries.get(*i).map_or("", |g| g.title.as_str()))?;
        }
        f.write_str(&self.1.title)
    }
}

/// What a folder holds: "2 folders · 3 entries", "24 entries".
struct Inside<'a>(&'a State, &'a Entry);

impl Display for Inside<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let all = &self.0.library_view().library().entries;
        let folders = self.1.items.iter().filter(|&&i| all.get(i).is_some_and(Entry::is_group)).count();
        let entries = self.1.items.len() - folders;
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        if folders > 0 {
            write!(f, "{folders} folder{}", plural(folders))?;
        }
        if entries > 0 || folders == 0 {
            let between = if folders > 0 { " · " } else { "" };
            write!(f, "{between}{entries} entr{}", if entries == 1 { "y" } else { "ies" })?;
        }
        Ok(())
    }
}

/// Who made an entry and when: "Supergiant Games · 2020", either one alone, or nothing.
struct Maker<'a>(&'a Entry);

impl Display for Maker<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let entry = self.0;
        match (entry.details.developer.as_str(), entry.details.year) {
            ("", Some(y)) => write!(f, "{y}"),
            ("", None) => Ok(()),
            (d, Some(y)) => write!(f, "{d} · {y}"),
            (d, None) => f.write_str(d),
        }
    }
}

/// The groups an entry is in, outermost first and separated by `between`: "Cosmos · Season 1". Its own
/// title comes last if `own` is set.
struct Groups<'a> {
    s: &'a State,
    entry: &'a Entry,
    own: bool,
    between: &'a str,
}

impl Display for Groups<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let entries = &self.s.library_view().library().entries;
        // At most a few levels, walked from the entry up and written from the top down.
        let mut chain = [None; 8];
        let mut at = self.entry.parent;
        let mut n = 0;
        while let (Some(i), Some(slot)) = (at, chain.get_mut(n)) {
            *slot = Some(i);
            n += 1;
            at = entries.get(i).and_then(|g| g.parent);
        }
        for (k, i) in chain.iter().take(n).rev().flatten().enumerate() {
            if k > 0 {
                f.write_str(self.between)?;
            }
            f.write_str(entries.get(*i).map_or("", |g| g.title.as_str()))?;
        }
        if self.own {
            if n > 0 {
                f.write_str(self.between)?;
            }
            f.write_str(&self.entry.title)?;
        }
        Ok(())
    }
}
