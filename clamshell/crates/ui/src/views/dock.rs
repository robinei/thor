//! The dock: its entries (pinned, then running; see `core`'s `dock`) as a grid of tiles on black. Each
//! tile shows the entry's picture (box art, or an app's icon) over its name; the grid scrolls by rows when
//! there are more than fit. A running entry is in colour, a stopped one grey, and when several run the
//! program in force has a dot.
//!
//! A tap starts an entry or shows it; a long press opens its menu in the dock's place (see [`draw_menu`]).
//! While an entry is being moved, its name is in the accent colour and the D-pad moves it. A tile with a
//! tray icon (see `core`'s `tray`) has a mark of three bars in its top left corner, which opens the
//! program's menu in the same place, as Menu in its long-press menu does.

use clamshell_core::dock::DOCK_COLUMNS;
use clamshell_core::tray::TrayEntry;
use clamshell_core::{DockId, Id, ListId};

use crate::draw::{Keep, Rect};
use crate::toolkit::Frame;
use crate::toolkit::format::{Bytes, Fmt, MaybeShare, Share};
use crate::toolkit::scroll::Scrolled;
use crate::toolkit::style::{self, Look, Text, Tone};
use crate::toolkit::widgets::{self, CAPTION_Y, HeaderRight, TileFace, TileShape};

/// The grid: at least this many rows fill the area, so a few entries keep tile-sized tiles.
const MIN_ROWS: usize = 4;
const TILE: TileShape = TileShape { icon: 0.42, icon_y: 0.1, cut: Some(8.0) };
/// A tile's picture: its top, and the room it leaves above the caption, of the tile's height; its corners'
/// radius.
const ART_TOP: f32 = 0.06;
const ART_ROOM: f32 = 0.03;
const ART_RADIUS: f32 = 4.0;
/// The tray mark's target (a square in the tile's top left corner) and its bars, of the tile's height.
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

/// An entry's tile: its picture (in colour while it runs, else grey) over its name.
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
    // A file's: the picture of the app showing it, if its window says which.
    let picture = s.app_showing(id).unwrap_or(game);
    widgets::tile(f, Id::Dock(DockId::Entry(i as u16)), r, &TILE, face, |f, r| {
        // Above the caption: portrait box art, or a square icon.
        let h = r.h * (CAPTION_Y - ART_TOP - ART_ROOM);
        let w = if picture.box_front.is_some() { h * 2.0 / 3.0 } else { h };
        let art = Rect::new(r.x + (r.w - w) / 2.0, r.y + r.h * ART_TOP, w, h);
        widgets::art(f.c, f.images, picture, art, ART_RADIUS, crate::draw::Align::Center, !running);
    });
    if s.tray_of(id).is_some() {
        tray_mark(f, r, i);
    }
}

/// A tile's tray mark: three bars in its top left corner, a target of its own over the tile's.
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

/// An entry's menu, from a long press on its tile, with the items that apply: Open another, Close window,
/// Menu (its tray icon's), Pin or Unpin, Start with ClamShell (or Don't), Move, Suspend or Resume, and while
/// it runs Quit and Force kill. The back arrow returns to the dock. If its tray menu is open, that instead.
fn draw_menu(f: &mut Frame, entry: &std::sync::Arc<str>) {
    let s = f.s;
    if let Some((tray, entries)) = s.tray_page().filter(|_| s.dock().tray_open()) {
        return draw_tray_menu(f, &tray.title, entries);
    }
    let title = s.entry(entry).map_or("", |g| g.title.as_str());
    let running = s.session().running(entry).is_some();
    let pinned = s.dock().items().iter().any(|i| i.id == *entry && i.pinned);
    // (A tray icon's own tile isn't an entry of the library: it can't be pinned.)
    let pinnable = s.shelf().library().has(entry);
    let item = |id, label, tone| Some((Id::Dock(id), label, tone));
    // (An array of the items that apply, none left out by allocating: drawn every frame.)
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
    // What it costs now, once read (see the battery tab).
    if let Some(clamshell_core::battery::Cost { cpu, gpu, memory }) = s.cost_of(entry) {
        let size = f.text(Text::Small);
        let line = area.take_top(size * 2.0);
        let cost = Fmt::<64>::of(format_args!("{} · CPU {} · GPU {}", Bytes(memory), Share(cpu), MaybeShare(gpu)));
        f.c.text_fit(line.top_line(size), size, Tone::Dim, &cost, Keep::Start);
    }
    widgets::menu(f, area, items.into_iter().flatten());
}

/// A program's tray menu (or a submenu of it): its entries (a disabled one dim, a checked one after a
/// dot, a submenu's ending in `›`), in columns as they fit. The back arrow: up a submenu, else the dock.
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
