//! The Library's edit mode (see `clamshell_state::library_view::edit`), laid out as the Library is: the
//! tabs along the top with + for a new tab, then Sources and Runners, and Done at the right end. For a
//! tab, the list of its or the open folder's places on the left with how many entries each has, the
//! folders sources make there dimmed and a row for a new folder, and the selected place's details on the
//! right: what it is in, what fills it, and its actions. For Sources and Runners, the list of them in
//! order and the selected one's fields and actions; open, its form, with a chosen field's options on the
//! right.

use clamshell_state::ListId;
use clamshell_state::config::{Config, Place, Source};
use clamshell_state::library_view::edit::{self, Edit, EditId, Form, Panel, Row};
use clamshell_state::library_view::form::{self, ItemRef, Set};

use super::library_view::{ACTION, DETAIL, HEADER_TEXT, LIST_W, ROW_H, ROW_TEXT, TAB_TEXT, TABS_H, TITLE};
use crate::gfx::icons::Icon;
use crate::gfx::{Align, Keep, Rect};
use crate::toolkit::format::Fmt;
use crate::toolkit::scroll::Scrolled;
use crate::toolkit::style::{self, Look, SURFACE_HI, Tone};
use crate::toolkit::{Frame, widgets};

/// Draws the edit mode over `screen`. Returns how many list rows fit, as the Library's list does.
pub(super) fn draw(f: &mut Frame, screen: Rect, edit: &Edit) -> usize {
    let s = f.s;
    let (w, h) = (screen.w, screen.h);
    let pad = style::pad(h).max(4.0);
    let tabs_h = h * TABS_H;
    let rows_fit = ((h - tabs_h - pad * 1.5) / (h * ROW_H)).floor().max(1.0) as usize;
    let Some(config) = s.library_view().config_shown().map(|c| &**c) else {
        let line = Rect::new(pad, h * 0.4, w - 2.0 * pad, h * ROW_H);
        widgets::centred_text(f, line, h * DETAIL, Tone::Dim, "library.json doesn't read, so it isn't edited here: see the Rescan entry");
        done(f, Rect::new(w - pad - h * 0.2, 0.0, h * 0.2, tabs_h), h * TAB_TEXT);
        return rows_fit;
    };

    // The tabs, + for a new one, and Done.
    let places = edit.panel() == Panel::Places;
    let items = edit::tabs(config).enumerate().map(|(i, p)| {
        let icon = p.icon.as_deref().and_then(Icon::named);
        (EditId::Tab(i as u16).into(), icon, p.name.as_str(), places && i == edit.tab())
    });
    // Room is kept for +, Sources, Runners and Done; the places' tabs have the rest, scrolling if they
    // need more, more tightly spaced than the Library's.
    let (size, inner) = (h * TAB_TEXT, pad * 0.5);
    let lists_w = f.c.text_width(size, "Sources") + f.c.text_width(size, "Runners") + inner * 4.0;
    let right = lists_w + tabs_h + h * 0.2 + pad * 2.0;
    let end = widgets::tabs(f, Rect::new(pad, 0.0, w - pad - right, tabs_h), size, inner, f32::INFINITY, items);
    f.text_button(EditId::NewTab.into(), Rect::new(end, 0.0, tabs_h, tabs_h), size, Tone::Fn, "+", Align::Center);
    let lists =
        [(EditId::Sources.into(), None, "Sources", edit.panel() == Panel::Sources), (EditId::Runners.into(), None, "Runners", edit.panel() == Panel::Runners)];
    widgets::tabs(f, Rect::new(w - pad - h * 0.2 - pad - lists_w, 0.0, lists_w + 1.0, tabs_h), size, inner, f32::INFINITY, lists);
    done(f, Rect::new(w - pad - h * 0.2, 0.0, h * 0.2, tabs_h), size);

    let body = Rect::new(pad, tabs_h + pad * 0.5, w - 2.0 * pad, h - tabs_h - pad * 1.5);
    match (edit.panel(), edit.form()) {
        (Panel::Places, _) => {}
        (_, Some(open)) => {
            form_view(f, h, config, open, body);
            return rows_fit;
        }
        (panel, None) => {
            list_view(f, h, edit, config, panel, body);
            return rows_fit;
        }
    }
    let Some(here) = edit.here(config) else {
        return rows_fit;
    };
    let mut list = Rect::new(pad, tabs_h + pad * 0.5, w * LIST_W - pad, h - tabs_h - pad * 1.5);
    // The open folders, outermost first, over the list, going back out.
    if edit.open(config).next().is_some() {
        let header = list.take_top(h * ROW_H);
        f.back(EditId::Up.into());
        f.button(EditId::Up.into(), header, Look::ICON, |f| {
            f.c.icon(Icon::ArrowLeft, Rect::new(header.x, header.y, header.h, header.h).square_center(header.h * 0.4), Tone::Normal);
            let names = Fmt::<192>::of(format_args!("{}", Path(edit, config)));
            let title = Rect::new(header.x + header.h + pad * 0.2, header.y, header.w - header.h - pad * 0.2, header.h);
            f.c.text_fit(title, header.h * HEADER_TEXT, Tone::High, &names, Keep::End);
        });
    }
    let row_h = h * ROW_H;
    let len = edit.rows(config, here);
    // The shared scrolled list: dragged, and scrolled by the state to the pad's focus.
    let scrolled = Scrolled::begin(f, ListId::LibraryEdit, list, row_h, len);
    for i in scrolled.shown() {
        let Some(row) = edit.row(config, here, i) else { break };
        let at = scrolled.row(i);
        let r = Rect::new(at.x, at.y, at.w, row_h - 2.0);
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
    scrolled.end(f, list.x + list.w + pad * 0.2);
    if let Some(place) = edit.selected(config) {
        details(f, h, edit, config, place, Rect::new(w * LIST_W + pad, tabs_h + pad * 0.5, w - w * LIST_W - 2.0 * pad, h - tabs_h - pad * 1.5));
    }
    rows_fit
}

/// The Sources' or Runners' list on the left, a row each and one for a new one, and the selected one's
/// fields and actions on the right.
fn list_view(f: &mut Frame, h: f32, edit: &Edit, config: &Config, panel: Panel, body: Rect) {
    let list = Rect::new(body.x, body.y, body.w * LIST_W, body.h);
    let row_h = h * ROW_H;
    let len = edit.listed(config);
    let pad = style::pad(h).max(4.0);
    let scrolled = Scrolled::begin(f, ListId::LibraryEdit, list, row_h, len);
    for i in scrolled.shown() {
        let at = scrolled.row(i);
        let r = Rect::new(at.x, at.y, at.w - pad, row_h - 2.0);
        let selected = i == edit.cursor();
        let look = Look { fill: selected.then_some(SURFACE_HI), ..Look::CARD };
        let size = r.h * ROW_TEXT;
        let text = Rect::new(r.x + pad * 0.6, r.y, r.w - pad * 1.2, r.h);
        let tone = if selected { Tone::High } else { Tone::Normal };
        f.button(EditId::Row(i as u16).into(), r, look, |f| {
            if i + 1 == len {
                let new = if panel == Panel::Sources { "+ New source" } else { "+ New runner" };
                f.c.text_fit(text, size, Tone::Fn, new, Keep::Start);
                return;
            }
            match panel {
                Panel::Sources => {
                    let Some(source) = config.sources.get(i) else { return };
                    let line = Fmt::<192>::of(format_args!("{}", Filled(source)));
                    f.c.text_fit(Rect::new(text.x, text.y, text.w * 0.62, text.h), size, tone, &line, Keep::Start);
                    let to = form::source_place(source).and_then(|id| config.places.iter().find(|p| p.id == id)).map_or("", |p| p.name.as_str());
                    let right = Fmt::<96>::of(format_args!("→ {to}  {}", Taken(f, source)));
                    f.c.text_fit(Rect::new(text.x + text.w * 0.62, text.y, text.w * 0.38, text.h), size * 0.8, Tone::Dim, &right, Keep::End);
                }
                _ => {
                    let Some(runner) = config.runners.get(i) else { return };
                    let name = if runner.name.is_empty() { runner.id.as_str() } else { runner.name.as_str() };
                    f.c.text_fit(Rect::new(text.x, text.y, text.w * 0.45, text.h), size, tone, name, Keep::Start);
                    let opens = Fmt::<192>::of(format_args!("{}", form::Words(&runner.patterns)));
                    f.c.text_fit(Rect::new(text.x + text.w * 0.45, text.y, text.w * 0.55, text.h), size * 0.8, Tone::Dim, &opens, Keep::End);
                }
            }
        });
    }
    scrolled.end(f, list.x + list.w - pad * 0.6);
    // The selected one's fields, as its form would show them, and its actions.
    let r = Rect::new(body.x + body.w * LIST_W + pad, body.y, body.w * (1.0 - LIST_W) - pad, body.h);
    let item = match panel {
        Panel::Sources => config.sources.get(edit.cursor()).map(ItemRef::Source),
        _ => config.runners.get(edit.cursor()).map(ItemRef::Runner),
    };
    let detail = h * DETAIL;
    let Some(item) = item else {
        let says = if panel == Panel::Sources {
            "A new source: a folder scanned, apps, Steam games, a browsable folder, removable storage, or the Library or Device."
        } else {
            "A new runner: how files of a kind open, by a command."
        };
        f.c.text_fit(Rect::new(r.x, r.y, r.w, detail * 1.5), detail, Tone::Dim, says, Keep::Start);
        return;
    };
    let mut y = fields(f, h, config, item, None, r);
    y += detail;
    let row = |y: f32| Rect::new(r.x, y, r.w * 0.6, detail * ACTION);
    for (id, label) in [(EditId::Open, "Edit"), (EditId::MoveUp, "Move up"), (EditId::MoveDown, "Move down")] {
        f.text_button(id.into(), row(y), detail, Tone::Fn, label, Align::Start);
        y += detail * ACTION;
    }
    match item {
        ItemRef::Runner(runner) if edit::runner_in_use(config, runner) => {
            f.c.text_fit(row(y), detail, Tone::Dim, "Remove: a source names it", Keep::Start);
        }
        _ => f.text_button(EditId::Remove.into(), row(y), detail, Tone::Accent, "Remove", Align::Start),
    }
}

/// The form open: its fields on the left, a tap to type, choose or flip each, with Save and Cancel under
/// them, and the chosen field's options on the right.
fn form_view(f: &mut Frame, h: f32, config: &Config, open: &Form, body: Rect) {
    let pad = style::pad(h).max(4.0);
    let left = Rect::new(body.x, body.y, body.w * 0.55, body.h);
    let mut y = fields(f, h, config, open.item.as_ref(), Some(open.field), left);
    let detail = h * DETAIL;
    y += detail;
    let half = left.w * 0.3;
    f.text_button(EditId::Save.into(), Rect::new(left.x, y, half, detail * ACTION), detail, Tone::Fn, "Save", Align::Start);
    f.back(EditId::Cancel.into());
    f.text_button(EditId::Cancel.into(), Rect::new(left.x + half, y, half, detail * ACTION), detail, Tone::Fn, "Cancel", Align::Start);
    // Why Save didn't save.
    if let Some(problem) = edit_problem(f) {
        f.c.text_fit(Rect::new(left.x, y + detail * ACTION, left.w, detail * ACTION), detail, Tone::Accent, problem, Keep::Start);
    }

    let right = Rect::new(body.x + body.w * 0.55 + pad, body.y, body.w * 0.45 - pad, body.h);
    let Some(&field) = open.item.fields().get(open.field) else { return };
    if open.choosing {
        let mut y = right.y;
        let current = Fmt::<192>::of(format_args!("{}", open.item.as_ref().value(config, field)));
        let mut j = 0u16;
        open.item.as_ref().each_option(config, field, |value, label| {
            // Past the room, the rest isn't drawn.
            if y > right.y + right.h - detail * ACTION {
                return;
            }
            let r = Rect::new(right.x, y, right.w, detail * ACTION);
            // A field shows its value as the option's label (a type) or as the option itself (a runner's id).
            let tone = if label == &*current || value == &*current { Tone::Accent } else { Tone::Fn };
            f.text_button(EditId::Choice(j).into(), r, detail, tone, label, Align::Start);
            y += detail * ACTION;
            j += 1;
        });
    }
}

/// An item's fields, a row each with its label and value, the `selected` one lit; tappable in a form.
/// Returns where they end.
fn fields(f: &mut Frame, h: f32, config: &Config, item: ItemRef<'_>, selected: Option<usize>, r: Rect) -> f32 {
    let detail = h * DETAIL;
    let row_h = detail * ACTION;
    let mut y = r.y;
    for (i, &field) in item.fields().iter().enumerate() {
        let row = Rect::new(r.x, y, r.w, row_h);
        let value = Fmt::<256>::of(format_args!("{}", item.value(config, field)));
        let label_w = r.w * 0.36;
        let lit = selected == Some(i);
        // In a form, a field that names a folder or file is chosen in the Library too, by its Choose… at
        // the row's end.
        let choose_w = if selected.is_some() && field.picks().is_some() { r.w * 0.2 } else { 0.0 };
        let draw = |f: &mut Frame| {
            f.c.text_fit(Rect::new(row.x, row.y, label_w, row.h), detail, Tone::Dim, field.label(), Keep::Start);
            let tone = match (field.set(), selected.is_some()) {
                (_, false) => Tone::Normal,
                _ if lit => Tone::High,
                (Set::Typed, true) | (Set::Chosen, true) | (Set::Toggled, true) => Tone::Fn,
            };
            let shown = if value.is_empty() { "—" } else { value.as_str() };
            f.c.text_fit(Rect::new(row.x + label_w, row.y, row.w - label_w - choose_w, row.h), detail, tone, shown, Keep::Start);
        };
        match (selected, field.picks()) {
            (Some(_), Some(_)) => {
                let typed = Rect::new(row.x, row.y, row.w - choose_w, row.h);
                f.button(EditId::Field(i as u16).into(), typed, Look { fill: lit.then_some(SURFACE_HI), ..Look::CARD }, draw);
                let choose = Rect::new(row.x + row.w - choose_w, row.y, choose_w, row.h);
                // Without a Files tab, there is nowhere to pick in.
                if f.s.library_view().can_pick() {
                    f.text_button(EditId::Pick(i as u16).into(), choose, detail, Tone::Fn, "Choose…", Align::End);
                } else {
                    f.c.text_fit(choose, detail, Tone::Dim, "No Files tab", Keep::End);
                }
            }
            (Some(_), None) => f.button(EditId::Field(i as u16).into(), row, Look { fill: lit.then_some(SURFACE_HI), ..Look::CARD }, draw),
            (None, _) => draw(f),
        }
        y += row_h;
    }
    y
}

/// How many entries a scan takes now, in the library as read: those its ids name, outside its groups.
struct Taken<'a, 'f, 'c>(&'a Frame<'f, 'c>, &'a Source);

impl std::fmt::Display for Taken<'_, '_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let Source::Dir(scan) = self.1 else { return Ok(()) };
        let library = self.0.s.library_view().library();
        let ours = |id: &str| id.strip_prefix(scan.id.as_str()).is_some_and(|rest| rest.starts_with(':'));
        let top = |g: &&clamshell_state::Entry| ours(&g.id) && g.parent.and_then(|p| library.entries.get(p)).is_none_or(|p| !ours(&p.id));
        write!(f, "{}", library.entries.iter().filter(top).count())
    }
}

/// The Done button at the tabs' right end.
fn done(f: &mut Frame, r: Rect, size: f32) {
    f.back(EditId::Done.into());
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
    match (edit.cut().and_then(|id| config.places.iter().find(|p| p.id == id)), edit.paste_problem(config), edit.cut_problem(config)) {
        (Some(cut), Some(why), _) => {
            let label = Fmt::<192>::of(format_args!("Paste {} here: {why}", cut.name));
            f.c.text_fit(row(y), detail, Tone::Dim, &label, Keep::Start);
        }
        (Some(cut), None, _) => {
            let label = Fmt::<128>::of(format_args!("Paste {} here", cut.name));
            f.text_button(EditId::Paste.into(), row(y), detail, Tone::Accent, &label, Align::Start);
        }
        (None, _, Some(why)) => {
            let label = Fmt::<128>::of(format_args!("Cut: {why}"));
            f.c.text_fit(row(y), detail, Tone::Dim, &label, Keep::Start);
        }
        (None, _, None) => f.text_button(EditId::Cut.into(), row(y), detail, Tone::Fn, "Cut, to move it", Align::Start),
    }
    y += detail * ACTION;
    let icon = Fmt::<64>::of(format_args!("Icon: {}", place.icon.as_deref().unwrap_or("none")));
    f.text_button(EditId::Icon.into(), row(y), detail, Tone::Fn, &icon, Align::Start);
    y += detail * ACTION;
    // A tab has no picture: something in it is always selected.
    if place.within.is_some() {
        let file = place.picture.as_deref().map_or("none", |p| p.rsplit('/').next().unwrap_or(p));
        if f.s.library_view().can_pick() {
            let picture = Fmt::<128>::of(format_args!("Picture: {file}"));
            f.text_button(EditId::Picture.into(), row(y), detail, Tone::Fn, &picture, Align::Start);
        } else {
            let picture = Fmt::<128>::of(format_args!("Picture: {file} (no Files tab to pick in)"));
            f.c.text_fit(row(y), detail, Tone::Dim, &picture, Keep::Start);
        }
        y += detail * ACTION;
    }
    if edit.choosing_icon() {
        // The icons, after none, in three columns.
        for (j, name) in std::iter::once("none").chain(edit::ICONS).enumerate() {
            let (col, line) = ((j % 3) as f32, (j / 3) as f32);
            let r = Rect::new(r.x + detail + col * r.w / 3.0, y + line * detail * ACTION, r.w / 3.0 - detail, detail * ACTION);
            let current = place.icon.as_deref().unwrap_or("none") == name;
            f.button(EditId::IconChoice(j as u16).into(), r, Look::ROW, |f| {
                if let Some(glyph) = Icon::named(name) {
                    f.c.icon(glyph, Rect::new(r.x, r.y + r.h * 0.2, r.h * 0.6, r.h * 0.6), Tone::Normal);
                }
                f.c.text_fit(Rect::new(r.x + r.h, r.y, r.w - r.h, r.h), detail, if current { Tone::Accent } else { Tone::Fn }, name, Keep::Start);
            });
        }
        return;
    }
    if place.kind.is_some() {
        f.c.text_fit(row(y), detail, Tone::Dim, "Remove: ClamShell's own tabs stay", Keep::Start);
    } else if edit::used(config, &place.id) {
        f.c.text_fit(row(y), detail, Tone::Dim, "Remove: something is in it", Keep::Start);
    } else {
        f.text_button(EditId::Remove.into(), row(y), detail, Tone::Accent, "Remove", Align::Start);
    }
    // Why the last thing asked wasn't done (a name taken, say).
    if let Some(problem) = edit.problem() {
        y += detail * ACTION;
        f.c.text_fit(Rect::new(r.x, y, r.w, detail * ACTION), detail, Tone::Accent, problem, Keep::Start);
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

/// Why the edit mode's last ask wasn't done, if it wasn't.
fn edit_problem(f: &Frame) -> Option<&'static str> {
    f.s.library_view().edit().and_then(|e| e.problem())
}
