//! The dock: its entries, pinned first and then running ones (see `core`'s `dock`), as a grid of tiles on
//! black. Each tile shows the entry's picture (box art or an app's icon) over its name. The grid scrolls
//! by rows when there are more entries than fit. A running entry is in colour and a stopped one grey.
//! When several are running, the one in front has a dot.
//!
//! A tap starts an entry or brings it forward. A long press opens its menu in place of the dock (see
//! [`draw_menu`]). While an entry is being moved, its name is in the accent colour and the D-pad moves
//! it. A tile with a tray icon (see `core`'s `tray`) has a mark of three bars in its top left corner.
//! Tapping the mark opens the program's tray menu in the same place, as Menu in the long-press menu does.

use clamshell_core::session::dock::DOCK_COLUMNS;
use clamshell_core::session::tray::TrayEntry;
use clamshell_core::{DockId, Id, ListId};

use crate::gfx::{Keep, Rect};
use crate::toolkit::Frame;
use crate::toolkit::format::{Bytes, Fmt, MaybeShare, Share};
use crate::toolkit::scroll::Scrolled;
use crate::toolkit::style::{self, Look, Text, Tone};
use crate::toolkit::widgets::{self, CAPTION_Y, HeaderRight, TileFace, TileShape};

/// The grid always has room for at least this many rows, so that a few entries still get normal-sized
/// tiles.
const MIN_ROWS: usize = 4;
const TILE: TileShape = TileShape { icon: 0.42, icon_y: 0.1, cut: Some(8.0) };
/// A tile's picture: its top and the gap it leaves above the caption, as fractions of the tile's height,
/// and its corner radius.
const ART_TOP: f32 = 0.06;
const ART_ROOM: f32 = 0.03;
const ART_RADIUS: f32 = 4.0;
/// The size of the tray mark's target, a square in the tile's top left corner, and of its bars, as
/// fractions of the tile's height.
const MARK_TARGET: f32 = 0.3;
const MARK_W: f32 = 0.11;

pub(super) fn draw(f: &mut Frame) {
    if let Some(entry) = f.s.dock().menu().cloned() {
        return draw_menu(f, &entry);
    }
    let body = f.body();
    let s = f.s;
    let pad = f.pad();
    let mut area = style::content(body);
    area.take_top(pad);
    if s.dock().moving().is_some() {
        let size = f.text(Text::Small);
        let line = area.take_bottom(size * 1.6);
        f.c.text_fit(line.shifted(0.0, size * 0.4).top_line(size), size, Tone::Dim, "The D-pad moves it · A or B: done", Keep::Start);
    }
    let items = s.dock().items();
    let rows = items.len().div_ceil(DOCK_COLUMNS);
    let gap = f.body().h * style::TILE_GAP;
    let row_h = area.h / MIN_ROWS as f32;
    let several = s.session().running_entries().len() > 1;
    let list = Scrolled::begin(f, ListId::Dock, area, row_h, rows);
    for row in list.shown() {
        let r = list.row(row);
        for column in 0..DOCK_COLUMNS {
            let i = row * DOCK_COLUMNS + column;
            let Some(item) = items.get(i) else {
                break;
            };
            let cell = Rect::new(r.x, r.y, r.w, r.h - gap).cell(DOCK_COLUMNS, 1, gap, column);
            entry_tile(f, cell, i, &item.id, several);
        }
    }
    list.end(f, area.x + area.w + pad * 0.3);
}

/// An entry's tile: its picture over its name, in colour while it runs and grey otherwise.
fn entry_tile(f: &mut Frame, r: Rect, i: usize, id: &std::sync::Arc<str>, several: bool) {
    let s = f.s;
    let Some(game) = s.entry(id) else {
        return;
    };
    let running = s.session().running(id).is_some();
    let in_force = several && s.program_in_force() == Some(id);
    let moving = s.dock().moving() == Some(id);
    let caption_tone = if moving { Tone::Accent } else { Tone::Normal };
    let note = if s.suspended(id) { "Suspended" } else { "" };
    let face = TileFace { caption: &game.title, caption_tone, dot: in_force.then_some(Tone::Go), note };
    // For a file, the picture of the app showing it, if its window says which app that is.
    let picture = s.app_showing(id).unwrap_or(game);
    widgets::tile(f, Id::Dock(DockId::Entry(i as u16)), r, &TILE, face, |f, r| {
        // Above the caption: portrait box art or a square icon.
        let h = r.h * (CAPTION_Y - ART_TOP - ART_ROOM);
        let w = if picture.box_front.is_some() { h * 2.0 / 3.0 } else { h };
        let art = Rect::new(r.x + (r.w - w) / 2.0, r.y + r.h * ART_TOP, w, h);
        widgets::art(f.c, f.images, picture, art, ART_RADIUS, crate::gfx::Align::Center, !running);
    });
    if s.tray_of(id).is_some() {
        tray_mark(f, r, i);
    }
}

/// A tile's tray mark: three bars in its top left corner, with its own target on top of the tile's.
fn tray_mark(f: &mut Frame, tile: Rect, i: usize) {
    let target = Rect::new(tile.x, tile.y, tile.h * MARK_TARGET, tile.h * MARK_TARGET);
    let w = tile.h * MARK_W;
    let shown = target.centered(w * 1.6, w * 1.6);
    f.button_in(Id::Dock(DockId::Tray(i as u16)), target, shown, Look::ICON, |f| {
        let bar = (w * 0.14).max(1.5);
        for k in 0..3 {
            let y = shown.y + shown.h * 0.3 + k as f32 * w * 0.3 - bar / 2.0;
            f.c.fill(Rect::new(shown.x + (shown.w - w) / 2.0, y, w, bar), Tone::Normal);
        }
    });
}

/// An entry's menu, opened by a long press on its tile. It has the items that apply: Open another, Close
/// window, Menu (the tray icon's menu), Pin or Unpin, Start with ClamShell or Don't, Move, Suspend or
/// Resume, and Quit and Force kill while it runs. The back arrow returns to the dock. If the entry's tray
/// menu is open, that is drawn instead.
fn draw_menu(f: &mut Frame, entry: &std::sync::Arc<str>) {
    let s = f.s;
    if let Some((tray, entries)) = s.tray_page().filter(|_| s.dock().tray_open()) {
        return draw_tray_menu(f, &tray.title, entries);
    }
    let title = s.entry(entry).map_or("", |g| g.title.as_str());
    let running = s.session().running(entry).is_some();
    let pinned = s.dock().items().iter().any(|i| i.id == *entry && i.pinned);
    // A tile for a tray icon alone isn't a library entry, so it can't be pinned.
    let pinnable = s.shelf().library().has(entry);
    let item = |id, label, tone| Some((Id::Dock(id), label, tone));
    // An array with `None` for the items that don't apply, so that drawing it every frame doesn't allocate.
    let items = [
        if s.opens_another(entry) { item(DockId::OpenAnother, "Open another", Tone::Fn) } else { None },
        if running && s.entry_window(entry).is_some() { item(DockId::CloseWindow, "Close window", Tone::Fn) } else { None },
        if s.tray_of(entry).is_some() { item(DockId::TrayMenu, "Menu", Tone::Fn) } else { None },
        if pinnable { item(DockId::Pin, if pinned { "Unpin" } else { "Pin" }, Tone::Fn) } else { None },
        if pinnable {
            item(DockId::Autostart, if s.autostarts(entry) { "Don't start with ClamShell" } else { "Start with ClamShell" }, Tone::Fn)
        } else {
            None
        },
        item(DockId::Move, "Move", Tone::Fn),
        if s.suspended(entry) {
            item(DockId::Suspend, "Resume", Tone::Fn)
        } else if s.suspendable(entry) {
            item(DockId::Suspend, "Suspend", Tone::Fn)
        } else {
            None
        },
        if running { item(DockId::Quit, "Quit", Tone::Accent) } else { None },
        if running { item(DockId::ForceKill, "Force kill", Tone::Accent) } else { None },
    ];
    let mut area = widgets::header(f, Some(Id::Dock(DockId::Back)), title, false, HeaderRight::None);
    // What it currently costs, once that has been read (see the battery tab).
    if let Some(clamshell_core::status::battery::Cost { cpu, gpu, memory }) = s.cost_of(entry) {
        let size = f.text(Text::Small);
        let line = area.take_top(size * 2.0);
        let cost = Fmt::<64>::of(format_args!("{} · CPU {} · GPU {}", Bytes(memory), Share(cpu), MaybeShare(gpu)));
        f.c.text_fit(line.top_line(size), size, Tone::Dim, &cost, Keep::Start);
    }
    widgets::menu(f, area, items.into_iter().flatten());
}

/// A program's tray menu or one of its submenus, with its entries in as many columns as fit. A disabled
/// entry is dim, a checked one follows a dot, and one that opens a submenu ends in `›`. The back arrow
/// goes up from a submenu, or else back to the dock.
fn draw_tray_menu(f: &mut Frame, title: &str, entries: &[TrayEntry]) {
    let items = entries.iter().map(|e| {
        let mark = if e.checked == Some(true) { "• " } else { "" };
        let more = if e.children.is_empty() { "" } else { " ›" };
        let label = Fmt::<160>::of(format_args!("{mark}{}{more}", e.label));
        (Id::Dock(DockId::TrayEntry(e.id)), label, if e.enabled { Tone::Fn } else { Tone::Dim })
    });
    let area = widgets::header(f, Some(Id::Dock(DockId::Back)), title, false, HeaderRight::None);
    widgets::menu(f, area, items);
}
