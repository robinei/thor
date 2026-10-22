//! shelf on the main screen: collections as tabs along the top (icon and name); the current list (the
//! collection's entries, or its open group's under a header that goes back) with box art thumbnails on
//! the left, scrolled to keep the selection on screen; the selected entry's large box art and details on
//! the right. Images come from the image cache (placeholders in the entry's colour until they arrive).

use std::fmt::{self, Display, Formatter};

use clamshell_core::{Entry, Id, Launch, ListId, ShelfId, State, Tab};

use crate::draw::{Column, Keep, Rect, Span, WRAP_LINE};
use crate::icons::Icon;
use crate::toolkit::format::{Bytes, DaysAgo, Fmt, PlayTime};
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
/// The filter's and an open group's header text, a little larger than a row's.
const HEADER_TEXT: f32 = ROW_TEXT * 1.15;
const THUMB: f32 = 0.82;
/// A row's marks (favourite, on the dock, running): their size, of a row's height.
const MARK: f32 = 0.3;
const BOX_ASPECT: f32 = 2.0 / 3.0;
/// A row's picture: a square (box art and a video's frame cropped to fill it).
const ROW_ASPECT: f32 = 1.0;
/// A thumbnail placeholder's corner radius.
const THUMB_RADIUS: f32 = 3.0;
/// The large art's height, of the list's height.
const ART_H: f32 = 0.78;
/// Details text sizes, of the screen's height.
const TITLE: f32 = 0.05;
const DETAIL: f32 = 0.03;
const SUMMARY: f32 = 0.028;
/// An action row's height, of the details' text size.
const ACTION: f32 = 1.45;
/// The gap after each paragraph of the details, of the padding.
const PARA_GAP: f32 = 0.4;
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
    let tabs_end = widgets::tabs(f, Rect::new(pad, 0.0, w - 2.0 * pad, tabs_h), h * TAB_TEXT, pad * 0.8, f32::INFINITY, items);

    // What was wrong reading the library (library.json, a folder missing): the first, at the right end.
    let problems = &s.shelf().library().problems;
    if let Some(first) = problems.first() {
        let more = Fmt::<16>::of(format_args!(" (+{})", problems.len() - 1));
        let text = Fmt::<256>::of(format_args!("{first}{}", if problems.len() > 1 { &*more } else { "" }));
        let x = tabs_end + pad * 2.0;
        let size = h * DETAIL;
        let tw = f.c.text_width(size, &text).min(w - pad - x);
        if tw > size * 4.0 {
            f.c.text_fit(Rect::new(w - pad - tw, 0.0, tw + 1.0, tabs_h), size, Tone::Accent, &text, Keep::Start);
        }
    }

    // The list, filling the height below the tabs (and an open group's header) exactly with whole rows.
    let mut list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    if let Some(group) = s.shelf().group() {
        let header = list.take_top(h * ROW_H);
        group_header(f, group, header, pad);
    }
    // What is typed to find an entry by (the bottom screen's keyboard), over the list.
    let filter = s.shelf().filter();
    if !filter.is_empty() {
        let line = list.take_top(h * ROW_H * 0.7);
        let size = line.h * HEADER_TEXT;
        let text = Fmt::<96>::of(format_args!("{filter}…"));
        f.c.text_fit(Rect::new(line.x + pad * 0.4, line.y, line.w - pad, line.h), size, Tone::Accent, &text, Keep::End);
        if s.shelf().list().is_empty() {
            let none = Fmt::<128>::of(format_args!("No title starts with \u{201c}{filter}\u{201d}"));
            f.c.text_fit(Rect::new(list.x + pad * 0.4, list.y, list.w - pad, h * ROW_H), size, Tone::Dim, &none, Keep::Start);
        }
    }
    let rows = ((list.h / (h * ROW_H)).floor() as usize).max(1);
    list_rows(f, list, rows, s.shelf().list().len(), pad);
    // Nothing but ClamShell's own tools: where games and apps come from, in the middle of the screen.
    if s.shelf().library().entries.iter().all(|g| matches!(g.launch, Launch::Tool(_))) {
        let line = Rect::new(pad, h * 0.5 - h * ROW_H, w - 2.0 * pad, h * ROW_H);
        widgets::centred_text(f, line, h * DETAIL, Tone::Dim, "No games or apps found: see ~/.config/clamshell/library.json");
    }
    // The selected entry; in an empty browsed folder, the folder (its actions: paste, a new folder).
    if let Some((game, _)) = s.file_target().filter(|(g, selected)| *selected || g.browsed.is_some()) {
        selected(f, game, screen, list, pad);
    }
    rows
}

/// An open group's header over the list: a back arrow and the groups open, outermost first ("Handhelds ›
/// GBA"; cut at the start where it doesn't fit); a tap goes back out.
fn group_header(f: &mut Frame, group: &Entry, r: Rect, pad: f32) {
    let s = f.s;
    f.button(Id::Shelf(ShelfId::Up), r, Look::ICON, |f| {
        let arrow = Rect::new(r.x, r.y, r.h, r.h);
        f.c.icon(Icon::ArrowLeft, arrow.square_center(r.h * 0.4), Tone::Normal);
        let title = Rect::new(r.x + r.h + pad * 0.2, r.y, r.w - r.h - pad * 0.2, r.h);
        let path = Fmt::<192>::of(format_args!("{}", Groups { s, game: group, own: true, between: " › " }));
        f.c.text_fit(title, r.h * HEADER_TEXT, Tone::High, &path, Keep::End);
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
        // A video watched (to its end, or into its credits): dimmer.
        let tone = if selected {
            Tone::High
        } else if game.mpv_file().is_some() && game.watched() {
            Tone::Dim
        } else {
            Tone::Normal
        };
        let look = Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD };
        // (Every row's picture in a square, cropped to fill it: the titles in line.)
        let thumb_h = r.h * THUMB;
        let thumb = Rect::new(r.x + pad * 0.4, r.y + (r.h - thumb_h) / 2.0, thumb_h * ROW_ASPECT, thumb_h);
        f.button(Id::Shelf(ShelfId::Game(game.id.clone())), r, look, |f| {
            widgets::thumb(f.c, f.images, game, thumb, THUMB_RADIUS, false);
            // Its marks at the row's right end, right to left: a folder's chevron, running (dim while
            // paused), on the dock, a favourite.
            let mark = r.h * MARK;
            let mut right = r.x + r.w - pad * 0.5;
            let paused = s.paused(&game.id) || s.suspended(&game.id);
            let marks = [
                (game.is_group(), Icon::Chevron, Tone::Normal),
                (s.session().running(&game.id).is_some(), Icon::Dot, if paused { Tone::Dim } else { Tone::Go }),
                (s.files().selected.contains(&game.id), Icon::Check, Tone::Accent),
                (s.dock().is_pinned(&game.id), Icon::for_tab(Tab::Dock), Tone::Normal),
                (game.favourite, Icon::Star, Tone::Accent),
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
            // Away from its folder (Recent, Favourites): the folders it is in before its title.
            let open = s.shelf().group().map(|g| &g.id);
            let in_view = game.parent.and_then(|p| s.shelf().library().entries.get(p)).is_none_or(|p| Some(&p.id) == open);
            if in_view {
                f.c.text_fit(title, row_h * ROW_TEXT, tone, &game.title, Keep::Start);
            } else {
                let path = Fmt::<256>::of(format_args!("{}", Groups { s, game, own: true, between: " · " }));
                f.c.text_fit(title, row_h * ROW_TEXT, tone, &path, Keep::Start);
            }
            // A video under way: how far, under its title.
            if let Some(fraction) = watched_fraction(game).filter(|_| !game.watched()) {
                let bar = Rect::new(tx, r.y + r.h * 0.8, (title.w * 0.6).min(row_h * 4.0), (r.h * 0.05).max(2.0));
                widgets::progress(f.c, bar, fraction);
            }
        });
    }
    scrolled.end(f, list.x + list.w - 3.0);
}

/// The selected entry right of the `list`, as high as it: large box art, then beside it the title, the
/// groups it is in, who made it and when, what playing it does now, whether it is a favourite and on the
/// dock, the play time, what it is about (as much as fits), and a screenshot: at the bottom of the text,
/// or under a landscape box (as wide as it).
fn selected(f: &mut Frame, game: &Entry, screen: Rect, list: Rect, pad: f32) {
    let s = f.s;
    let h = screen.h;
    let x0 = screen.w * LIST_W + pad;
    let room = Rect::new(x0, list.y, (screen.w - pad - x0) * COVER_W, list.h * ART_H);
    let cover = widgets::cover(f.c, f.images, game, room, RADIUS_L, BOX_ASPECT);
    let landscape = cover.w > cover.h;
    let tx = if landscape { room.x + room.w } else { cover.x + cover.w } + pad * 1.2;
    let text_w = screen.w - pad - tx;
    let mut col = Column::new(Rect::new(tx, list.y, text_w, list.h));
    // A paragraph of the details: wrapped in up to `lines` lines where the column is, then a gap.
    let para = |f: &mut Frame, col: &mut Column, size: f32, tone: Tone, text: &str, lines: usize| {
        col.skip(f.c.wrap(Span { x: tx, y: col.y(), w: text_w }, size, tone, text, lines) + pad * PARA_GAP);
    };
    let detail = h * DETAIL;
    para(f, &mut col, h * TITLE, Tone::High, &game.title, 3);

    // The groups it is in (a series, a season); who made it and when.
    if game.parent.is_some() {
        let groups = Fmt::<192>::of(format_args!("{}", Groups { s, game, own: false, between: " · " }));
        para(f, &mut col, detail, Tone::Normal, &groups, 2);
    }
    let maker = Fmt::<128>::of(format_args!("{}", Maker(game)));
    if !maker.is_empty() {
        para(f, &mut col, detail, Tone::Dim, &maker, 2);
    }

    // A browsed file: how big, when it was changed.
    if let Some(b) = game.browsed.as_ref().filter(|b| !b.folder && b.modified > 0) {
        let days = s.unix_secs().saturating_sub(b.modified) / 86_400;
        let facts = Fmt::<64>::of(format_args!("{}{}", Bytes(b.size.unwrap_or(0)), DaysAgo(days)));
        para(f, &mut col, detail, Tone::Dim, &facts, 1);
    }

    // A folder: what is in it (not a browsed one: what is known of it may be old).
    if game.is_group() && game.browsed.is_none() {
        let inside = Fmt::<64>::of(format_args!("{}", Inside(s, game)));
        para(f, &mut col, detail, Tone::Normal, &inside, 2);
    }

    // A video: how far into it, and how long it is.
    if let Some(at) = game.position.filter(|_| game.mpv_file().is_some()) {
        let minutes = Fmt::<48>::of(format_args!("{}", Minutes(at, game.duration)));
        let bar_w = (text_w * 0.45).min(detail * 9.0);
        if let Some(fraction) = watched_fraction(game) {
            let bar = Rect::new(tx, col.y() + detail * 0.42, bar_w, (detail * 0.22).max(3.0));
            widgets::progress(f.c, bar, fraction);
        }
        let x = if watched_fraction(game).is_some() { tx + bar_w + pad * 0.6 } else { tx };
        f.c.text(x, col.y(), detail, Tone::Normal, &minutes);
        col.skip(detail * WRAP_LINE + pad * PARA_GAP);
    } else if game.mpv_file().is_some() && game.watched() {
        para(f, &mut col, detail, Tone::Dim, "Watched", 1);
    }

    // Its actions (see clamshell-core's `actions`): each a row to tap, with its button.
    let action_h = detail * ACTION;
    let rows = actions(f, game, Rect::new(tx, col.y(), text_w, list.y + list.h - col.y()), action_h, pad);
    col.skip(rows);
    col.skip(pad * 0.3);

    // The play time.
    let plays = game.plays;
    if plays.count > 0 {
        let days = s.unix_secs().saturating_sub(plays.last) / 86_400;
        let played = Fmt::<64>::of(format_args!("{}{}", PlayTime(plays.seconds), DaysAgo(days)));
        para(f, &mut col, detail, Tone::Dim, &played, 2);
    }

    // A screenshot: under a landscape box, as wide; else at the bottom of the text. What it is about in as
    // many lines as fit above that.
    // (Its screenshot as its picture already: not twice.)
    let shot = match (game.shot.is_some() && !widgets::wide_picture(game), landscape) {
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

/// The details' actions, as rows from the top of `r`, each with its pad button: A opens (plays, shows
/// it running), Start continues a folder of videos, Y and X a favourite and on the dock (a browsed file's:
/// Copy and Paste), the file actions (Delete and Empty Trash asking to be pressed again); a paste under
/// way, how far. Their height.
fn actions(f: &mut Frame, game: &Entry, r: Rect, action_h: f32, pad: f32) -> f32 {
    use clamshell_core::actions::{Action, Shortcut};
    use clamshell_core::files::FileAction;
    let s = f.s;
    let mut y = r.y;
    // (A row as wide as fourteen of the details' text size; Continue's, naming the next video, as the details.)
    let w = r.w.min(action_h / ACTION * 14.0);
    let shortcut = |key| s.button_of(key);
    // (The pad on the actions themselves: their shortcuts off, faded; A and B said below.)
    let in_actions = s.in_actions();
    for &action in s.actions().as_slice() {
        let key = match action {
            Action::Open => "A",
            _ if shortcut(Shortcut::Start) == Some(action) => "Start",
            _ if shortcut(Shortcut::Y) == Some(action) => "Y",
            _ if shortcut(Shortcut::X) == Some(action) => "X",
            _ => "",
        };
        let running = s.session().running(&game.id).is_some();
        let (label, tone) = match action {
            Action::Open if game.is_group() => (Fmt::<192>::of(format_args!("Open")), Tone::Normal),
            Action::Open if running => (Fmt::of(format_args!("Running · show it")), Tone::Go),
            Action::Open if matches!(game.launch, clamshell_core::Launch::Tool(_)) => (Fmt::of(format_args!("Run")), Tone::Normal),
            Action::Open if game.browsed.is_some() => (Fmt::of(format_args!("Open")), Tone::Normal),
            Action::Open => (Fmt::of(format_args!("Play")), Tone::Normal),
            Action::Continue => {
                // (Where in it is mpv's.)
                let library = s.shelf().library();
                let next = library.entries.iter().position(|g| g.id == game.id).and_then(|g| library.to_continue(g)).and_then(|i| library.entries.get(i));
                match next {
                    Some(n) => (Fmt::of(format_args!("Play · {}", Within(s, n, game))), Tone::Go),
                    None => (Fmt::of(format_args!("Play")), Tone::Go),
                }
            }
            Action::Enqueue if !s.files().selected.is_empty() => (Fmt::of(format_args!("Enqueue · {} selected", s.files().selected.len())), Tone::Normal),
            Action::Enqueue => (Fmt::of(format_args!("Enqueue")), Tone::Normal),
            Action::Favourite if game.favourite => (Fmt::of(format_args!("Favourite")), Tone::Accent),
            Action::Favourite => (Fmt::of(format_args!("Not a favourite")), Tone::Dim),
            Action::Pin if s.dock().is_pinned(&game.id) => (Fmt::of(format_args!("On the dock")), Tone::Normal),
            Action::Pin => (Fmt::of(format_args!("Not on the dock")), Tone::Dim),
            Action::File(a) if s.file_armed(a) => (Fmt::of(format_args!("{} · press again", a.label())), Tone::Accent),
            Action::File(a) => {
                let files = s.files();
                let clip = files.clip.as_ref();
                let selected = files.selected.len();
                let label = match (a, clip) {
                    (FileAction::Paste, Some(clip)) if clip.entries.len() > 1 => Fmt::of(format_args!("Paste · {} entries", clip.entries.len())),
                    (FileAction::Paste, Some(clip)) => Fmt::of(format_args!("Paste · {}", clip.entries.first().map_or("", |(_, n)| n.as_str()))),
                    (FileAction::Select, _) if files.selected.contains(&game.id) => Fmt::of(format_args!("Selected · {selected}")),
                    (FileAction::ShowHidden, _) => Fmt::of(format_args!("{} hidden files", if files.hidden { "Hide" } else { "Show" })),
                    (FileAction::OpenIn, _) => Fmt::of(format_args!("Open in {}", s.opened_in(game).map_or("", |r| &*r.name))),
                    (FileAction::Copy | FileAction::Cut | FileAction::Trash | FileAction::Restore | FileAction::Delete, _) if selected > 0 => {
                        Fmt::of(format_args!("{} · {selected} selected", a.label()))
                    }
                    (FileAction::Copy | FileAction::Cut, Some(clip))
                        if clip.entries.iter().any(|(p, _)| Some(p) == game.file.as_ref()) && (a == FileAction::Cut) == clip.cut =>
                    {
                        Fmt::of(format_args!("{} · {}", a.label(), if clip.cut { "cut" } else { "copied" }))
                    }
                    _ => Fmt::of(format_args!("{}", a.label())),
                };
                (label, if a.confirms() { Tone::Accent } else { Tone::Normal })
            }
        };
        let row_w = if action == Action::Continue { r.w } else { w };
        widgets::key_action(f, Id::Shelf(action.id()), Rect::new(r.x, y, row_w, action_h), key, &label, tone, in_actions);
        y += action_h + pad * 0.15;
    }
    if let Some(clamshell_core::files::Busy { what, fraction }) = &s.files().busy {
        y += pad * 0.3;
        let size = action_h * 0.62;
        f.c.text_fit(Rect::new(r.x, y, r.w, size * 1.4), size, Tone::Normal, what, Keep::Start);
        y += size * 1.6;
        widgets::progress(f.c, Rect::new(r.x, y, w, (size * 0.22).max(3.0)), *fraction);
        y += size;
    }
    // The pad on them: what its buttons do now, at the bottom (the details' text gives way).
    if in_actions {
        let line = Rect::new(r.x, r.y + r.h - action_h, w, action_h);
        let confirm = widgets::key_hint(f, line, "A", "Confirm");
        widgets::key_hint(f, Rect::new(line.x + confirm + pad, line.y, line.w - confirm - pad, line.h), "B", "Back");
        return r.h;
    }
    y - r.y
}

/// How far into a video, of its length, if both are known.
fn watched_fraction(game: &Entry) -> Option<f32> {
    let (at, length) = (game.position?, game.duration?);
    (length > 0).then(|| at as f32 / length as f32)
}

/// Minutes into a video, and of how many: "23 of 45 min"; "23 min in" without its length.
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
        let games = &self.0.shelf().library().entries;
        // (The folders from the entry up to the folder, written from the top down.)
        let mut chain = [None; 8];
        let mut at = self.1.parent;
        let mut n = 0;
        while let (Some(i), Some(slot)) = (at, chain.get_mut(n)) {
            if games.get(i).is_some_and(|g| g.id == self.2.id) {
                break;
            }
            *slot = Some(i);
            n += 1;
            at = games.get(i).and_then(|g| g.parent);
        }
        for i in chain.iter().take(n).rev().flatten() {
            write!(f, "{} · ", games.get(*i).map_or("", |g| g.title.as_str()))?;
        }
        f.write_str(&self.1.title)
    }
}

/// What a folder holds: "2 folders · 3 entries", "24 entries".
struct Inside<'a>(&'a State, &'a Entry);

impl Display for Inside<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let games = &self.0.shelf().library().entries;
        let folders = self.1.items.iter().filter(|&&i| games.get(i).is_some_and(Entry::is_group)).count();
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

/// Who made an entry and when: "Supergiant Games · 2020", either alone, or nothing.
struct Maker<'a>(&'a Entry);

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

/// The groups an entry is in, outermost first, `between` between them: "Cosmos · Season 1"; with its own
/// title last if `own`.
struct Groups<'a> {
    s: &'a State,
    game: &'a Entry,
    own: bool,
    between: &'a str,
}

impl Display for Groups<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let games = &self.s.shelf().library().entries;
        // (At most a few levels: walked from the entry up, written from the top down.)
        let mut chain = [None; 8];
        let mut at = self.game.parent;
        let mut n = 0;
        while let (Some(i), Some(slot)) = (at, chain.get_mut(n)) {
            *slot = Some(i);
            n += 1;
            at = games.get(i).and_then(|g| g.parent);
        }
        for (k, i) in chain.iter().take(n).rev().flatten().enumerate() {
            if k > 0 {
                f.write_str(self.between)?;
            }
            f.write_str(games.get(*i).map_or("", |g| g.title.as_str()))?;
        }
        if self.own {
            if n > 0 {
                f.write_str(self.between)?;
            }
            f.write_str(&self.game.title)?;
        }
        Ok(())
    }
}
