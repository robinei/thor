//! The dock: its entries (pinned, then running; see `core`'s `dock`) as a grid of tiles on black, each its
//! picture (box art, or an app's icon) over its name, scrolled by rows when there are more than fit. A
//! running entry in colour (the program in force marked by a dot when several run), a stopped one grey.
//! A tap starts it or shows it; a long press opens its menu in the dock's place: Open another, Close
//! window, Pin or Unpin, Move, Quit, Force kill (a stopped one: Unpin, Move). While one moves, it is
//! named in the accent colour and the D-pad moves it.

use clamshell_core::dock::DOCK_COLUMNS;
use clamshell_core::{DockId, Id, ListId};

use crate::draw::{Keep, Rect};
use crate::toolkit::Frame;
use crate::toolkit::scroll::Scrolled;
use crate::toolkit::style::{self, Text, Tone};
use crate::toolkit::widgets::{self, CAPTION_Y, HeaderRight, TileFace, TileShape};

/// The grid: at least this many rows fill the area, so a few entries keep tile-sized tiles.
const MIN_ROWS: usize = 4;
/// Gap between tiles, of the area's height.
const GAP: f32 = 0.014;
/// A tile's picture area: its top, of the tile's height; the room it leaves above the caption.
const TILE: TileShape = TileShape { icon: 0.42, icon_y: 0.1, cut: Some(8.0) };
const ART_TOP: f32 = 0.06;
const ART_ROOM: f32 = 0.03;
const ART_RADIUS: f32 = 4.0;

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
    let gap = f.body().h * GAP;
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
    let Some(game) = s.game(id) else {
        return;
    };
    let running = s.session().running(id).is_some();
    let in_force = several && s.program_in_force() == Some(id);
    let moving = s.dock().moving() == Some(id);
    let caption_tone = if moving { Tone::Accent } else { Tone::Normal };
    let face = TileFace { caption: &game.title, caption_tone, dot: in_force.then_some(Tone::Go), note: "" };
    widgets::tile(f, Id::Dock(DockId::Entry(i as u16)), r, &TILE, face, |f, r| {
        // Above the caption: portrait box art, or a square icon.
        let h = r.h * (CAPTION_Y - ART_TOP - ART_ROOM);
        let w = if game.box_front.is_some() { h * 2.0 / 3.0 } else { h };
        let art = Rect::new(r.x + (r.w - w) / 2.0, r.y + r.h * ART_TOP, w, h);
        widgets::art(f.c, f.images, game, art, ART_RADIUS, crate::draw::Align::Center, !running);
    });
}

/// An entry's menu, from a long press on its tile: running, Open another, Close window (if it has one),
/// Pin or Unpin, Move, Quit, Force kill; stopped, Unpin and Move. The back arrow returns to the dock.
fn draw_menu(f: &mut Frame, entry: &std::sync::Arc<str>) {
    let s = f.s;
    let title = s.game(entry).map_or("", |g| g.title.as_str());
    let running = s.session().running(entry).is_some();
    let pinned = s.dock().items().iter().any(|i| i.id == *entry && i.pinned);
    let item = |id, label, tone| Some((Id::Dock(id), label, tone));
    let items: Vec<(Id, &str, Tone)> = [
        if running { item(DockId::OpenAnother, "Open another", Tone::Fn) } else { None },
        if running && s.entry_window(entry).is_some() { item(DockId::CloseWindow, "Close window", Tone::Fn) } else { None },
        item(DockId::Pin, if pinned { "Unpin" } else { "Pin" }, Tone::Fn),
        item(DockId::Move, "Move", Tone::Fn),
        if running { item(DockId::Quit, "Quit", Tone::Accent) } else { None },
        if running { item(DockId::ForceKill, "Force kill", Tone::Accent) } else { None },
    ]
    .into_iter()
    .flatten()
    .collect();
    let area = widgets::header(f, Some(Id::Dock(DockId::Back)), title, false, HeaderRight::None);
    widgets::menu(f, area, &items);
}
