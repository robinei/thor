//! The Library's edit mode (see `clamshell_state::library_view::edit`), laid out as the Library is: the
//! tabs along the top with + for a new tab and Done at the right end, the list of the tab's or open
//! folder's places on the left with how many entries each has, the folders sources make there dimmed and a
//! row for a new folder, and the selected place's details on the right: what it is in, what fills it, and
//! its actions.

use clamshell_state::config::{Config, Place, Source};
use clamshell_state::library_view::edit::{self, Edit, EditId, Row};

use super::library_view::{ACTION, DETAIL, HEADER_TEXT, LIST_W, ROW_H, ROW_TEXT, TAB_TEXT, TABS_H, TITLE};
use crate::gfx::icons::Icon;
use crate::gfx::{Align, Keep, Rect};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{self, Look, SURFACE_HI, Tone};
use crate::toolkit::{Frame, widgets};

/// Draws the edit mode over `screen`. Returns how many list rows fit, as the Library's list does.
pub(super) fn draw(f: &mut Frame, screen: Rect, edit: &Edit) -> usize {
    let s = f.s;
    let (w, h) = (screen.w, screen.h);
    let pad = style::pad(h).max(4.0);
    let tabs_h = h * TABS_H;
    let rows_fit = ((h - tabs_h - pad * 1.5) / (h * ROW_H)).floor().max(1.0) as usize;
    let Some(config) = edit.config(s.library_view().library().config.as_ref()) else {
        let line = Rect::new(pad, h * 0.4, w - 2.0 * pad, h * ROW_H);
        widgets::centred_text(f, line, h * DETAIL, Tone::Dim, "library.json doesn't read, so it isn't edited here: see the Rescan entry");
        done(f, Rect::new(w - pad - h * 0.2, 0.0, h * 0.2, tabs_h), h * TAB_TEXT);
        return rows_fit;
    };

    // The tabs, + for a new one, and Done.
    let items = edit::tabs(config).enumerate().map(|(i, p)| {
        let icon = p.icon.as_deref().and_then(Icon::named);
        (EditId::Tab(i as u16).into(), icon, p.name.as_str(), i == edit.tab())
    });
    let end = widgets::tabs(f, Rect::new(pad, 0.0, w * 0.75, tabs_h), h * TAB_TEXT, pad * 0.8, f32::INFINITY, items);
    f.text_button(EditId::NewTab.into(), Rect::new(end, 0.0, tabs_h, tabs_h), h * TAB_TEXT, Tone::Fn, "+", Align::Center);
    done(f, Rect::new(w - pad - h * 0.2, 0.0, h * 0.2, tabs_h), h * TAB_TEXT);
    let label = Rect::new(w * 0.6, tabs_h * 0.62, w * 0.4 - pad - h * 0.22, tabs_h * 0.38);
    f.c.text_in(label, h * DETAIL * 0.7, Tone::Dim, "editing the library", Align::End);

    let Some(here) = edit.here(config) else {
        return rows_fit;
    };
    let mut list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    // The open folders, outermost first, over the list, going back out.
    if edit.open(config).next().is_some() {
        let header = list.take_top(h * ROW_H);
        f.button(EditId::Up.into(), header, Look::ICON, |f| {
            f.c.icon(Icon::ArrowLeft, Rect::new(header.x, header.y, header.h, header.h).square_center(header.h * 0.4), Tone::Normal);
            let names = Fmt::<192>::of(format_args!("{}", Path(edit, config)));
            let title = Rect::new(header.x + header.h + pad * 0.2, header.y, header.w - header.h - pad * 0.2, header.h);
            f.c.text_fit(title, header.h * HEADER_TEXT, Tone::High, &names, Keep::End);
        });
    }
    let row_h = h * ROW_H;
    let shown = ((list.h / row_h).floor() as usize).max(1);
    let len = edit.rows(config, here);
    // The window keeps the selected row off its edges, so that the pad can step on past them.
    let top = (edit.cursor() + 2).saturating_sub(shown).min(len.saturating_sub(shown));
    for i in top..len.min(top + shown) {
        let Some(row) = edit.row(config, here, i) else { break };
        let r = Rect::new(list.x, list.y + (i - top) as f32 * row_h, list.w, row_h - 2.0);
        let selected = i == edit.cursor();
        let look = Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD };
        let size = r.h * ROW_TEXT;
        let text = Rect::new(r.x + pad * 0.6, r.y, r.w - pad * 1.2, r.h);
        f.button(EditId::Row(i as u16).into(), r, look, |f| match row {
            Row::Place(p) => {
                let cut = edit.cut() == Some(p.id.as_str());
                let tone = if cut {
                    Tone::Accent
                } else if selected {
                    Tone::High
                } else {
                    Tone::Normal
                };
                f.c.text_fit(text, size, tone, &p.name, Keep::Start);
                let count = Fmt::<16>::of(format_args!("{}", entries_in(f, p)));
                f.c.text_in(text, size * 0.8, Tone::Dim, &count, Align::End);
            }
            Row::Made(name) => {
                let label = Fmt::<96>::of(format_args!("· {name}"));
                f.c.text_fit(text, size, Tone::Dim, &label, Keep::Start);
            }
            Row::New => {
                f.c.text_fit(text, size, Tone::Fn, "+ New folder", Keep::Start);
            }
        });
    }
    if let Some(place) = edit.selected(config) {
        details(f, h, edit, config, place, Rect::new(w * LIST_W + pad, tabs_h + pad * 0.5, w - w * LIST_W - 2.0 * pad, h - tabs_h - pad * 1.5));
    }
    rows_fit
}

/// The Done button at the tabs' right end.
fn done(f: &mut Frame, r: Rect, size: f32) {
    f.text_button(EditId::Done.into(), r, size, Tone::Fn, "Done", Align::End);
}

/// How many entries the Library shows in place `p`: in its folder, or for a tab, in the tab.
fn entries_in(f: &Frame, p: &Place) -> usize {
    let library = f.s.library_view().library();
    match &p.within {
        Some(_) => library.entries.iter().find(|g| g.id.strip_prefix(clamshell_state::ids::PLACE) == Some(p.id.as_str())).map_or(0, |g| g.items.len()),
        None => library.collections.iter().find(|c| c.name == p.name).map_or(0, |c| c.entries.len()),
    }
}

/// The selected place's details: its name, what it is in, what fills it, and its actions.
/// `h` is the screen's height, which the sizes are fractions of.
fn details(f: &mut Frame, h: f32, edit: &Edit, config: &Config, place: &Place, r: Rect) {
    let (title, detail) = (h * TITLE, h * DETAIL);
    let mut y = r.y;
    f.c.text_fit(Rect::new(r.x, y, r.w, title * 1.4), title, Tone::High, &place.name, Keep::Start);
    y += title * 1.6;
    let within = place.within.as_deref().and_then(|id| config.places.iter().find(|p| p.id == id));
    let line = match (within, place.kind) {
        (Some(p), _) => Fmt::<128>::of(format_args!("in {}", p.name)),
        (None, Some(_)) => Fmt::of(format_args!("one of ClamShell's own tabs")),
        (None, None) => Fmt::of(format_args!("a tab")),
    };
    f.c.text_fit(Rect::new(r.x, y, r.w, detail * 1.4), detail, Tone::Dim, &line, Keep::Start);
    y += detail * 2.4;
    let filling = config.sources.iter().filter(|s| fills(s, &place.id));
    let mut any = false;
    for source in filling {
        if !any {
            f.c.text_fit(Rect::new(r.x, y, r.w, detail * 1.4), detail, Tone::Normal, "Filled by", Keep::Start);
            y += detail * 1.6;
            any = true;
        }
        let text = Fmt::<192>::of(format_args!("{}", Filled(source)));
        f.c.text_fit(Rect::new(r.x + detail, y, r.w - detail, detail * 1.4), detail, Tone::Dim, &text, Keep::Start);
        y += detail * 1.5;
    }
    if !any {
        f.c.text_fit(Rect::new(r.x, y, r.w, detail * 1.4), detail, Tone::Dim, "Nothing fills it yet.", Keep::Start);
        y += detail * 1.6;
    }
    y += detail;
    // The actions, a row each; one that can't be done now is dimmed and says why.
    let row = |y: f32| Rect::new(r.x, y, r.w * 0.6, detail * ACTION);
    f.text_button(EditId::Rename.into(), row(y), detail, Tone::Fn, "Rename", Align::Start);
    y += detail * ACTION;
    match edit.cut().and_then(|id| config.places.iter().find(|p| p.id == id)) {
        Some(cut) => {
            let label = Fmt::<128>::of(format_args!("Paste {} here", cut.name));
            f.text_button(EditId::Paste.into(), row(y), detail, Tone::Accent, &label, Align::Start);
        }
        None => f.text_button(EditId::Cut.into(), row(y), detail, Tone::Fn, "Cut, to move it", Align::Start),
    }
    y += detail * ACTION;
    if place.kind.is_some() {
        f.c.text_fit(row(y), detail, Tone::Dim, "Remove: ClamShell's own tabs stay", Keep::Start);
    } else if edit::used(config, &place.id) {
        f.c.text_fit(row(y), detail, Tone::Dim, "Remove: something is in it", Keep::Start);
    } else {
        f.text_button(EditId::Remove.into(), row(y), detail, Tone::Accent, "Remove", Align::Start);
    }
}

/// Whether `source` puts entries in place `id` itself (rather than in a folder of its own there).
fn fills(source: &Source, id: &str) -> bool {
    match source {
        Source::Dir(scan) => scan.place == id,
        Source::Desktop(r) | Source::Steam(r) => r.place.as_deref() == Some(id) && !r.hide,
        Source::Builtin(b) => b.place == id,
        Source::Browse(_) | Source::Removable(_) => false,
    }
}

/// What a source is, in a line: a scan by its id and dir, desktop and Steam rules by what they match.
struct Filled<'a>(&'a Source);

impl std::fmt::Display for Filled<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Source::Dir(scan) => write!(f, "{} scan · {}", scan.id, scan.dir),
            Source::Desktop(r) | Source::Steam(r) => {
                let kind = if matches!(self.0, Source::Steam(_)) { "Steam games" } else { "Desktop" };
                match (r.categories.as_deref(), r.id.as_deref(), r.name.as_deref()) {
                    (Some(c), _, _) => write!(f, "{kind}: {c}"),
                    (None, Some(id), _) => write!(f, "{kind}: {id}"),
                    (None, None, Some(name)) => write!(f, "{kind}: {name}"),
                    (None, None, None) => write!(f, "{kind}: all others"),
                }
            }
            Source::Builtin(b) => write!(f, "ClamShell's {}", b.id),
            Source::Browse(b) => write!(f, "browsed: {}", b.dir),
            Source::Removable(_) => f.write_str("removable storage"),
        }
    }
}

/// The open folders' names, outermost first, as "Games › SNES".
struct Path<'a>(&'a Edit, &'a Config);

impl std::fmt::Display for Path<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        for (i, p) in self.0.open(self.1).enumerate() {
            if i > 0 {
                f.write_str(" › ")?;
            }
            f.write_str(&p.name)?;
        }
        Ok(())
    }
}
