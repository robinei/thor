//! The dock: the running games first (the game, the one last in front, marked), or while none runs a
//! "Start game" tile that goes to shelf (so the apps keep their places unless several games run); then
//! the apps from launcher.json, as a grid of tiles on black, each an icon over its name. A tap starts an
//! app or goes to it, or shows a game; a long press on a running one opens its menu (Open another, Close
//! window, Quit, Force kill; a game's Quit and Force kill) in the dock's place. Running shows as colour: app icons
//! in full colour (drawn icons green), idle ones grey and dim. What is wrong with the file, if anything,
//! shows at the bottom.

use clamshell_core::apps::App;
use clamshell_core::{DockId, DockMenu, Game, Id};

use crate::draw::{Align, Keep, Rect};
use crate::icons::Icon;
use crate::toolkit::Frame;
use crate::toolkit::style::{self, Text, Tone};
use crate::toolkit::widgets::{self, CAPTION_Y, HeaderRight, TileFace, TileShape};

/// The grid: at least this many rows, so a few apps keep tile-sized tiles.
const COLUMNS: usize = 4;
const MIN_ROWS: usize = 4;
/// Gap between tiles, of the area's height.
const GAP: f32 = 0.014;
/// The icon's size and top, of a tile's height.
const TILE: TileShape = TileShape { icon: 0.42, icon_y: 0.1, cut: Some(8.0) };
/// A running game's box art: its top, of the icon's top; the room it leaves above the caption (it
/// reaches down to it), of the tile's height; its placeholder's corner radius.
const ART_Y: f32 = 0.6;
const ART_ROOM: f32 = 0.03;
const ART_RADIUS: f32 = 4.0;
/// An idle app's icon image: grey at this brightness.
const IDLE: f32 = 0.55;

fn face(caption: &str) -> TileFace<'_> {
    TileFace { caption, caption_tone: Tone::Normal, dot: None, note: "" }
}

fn running_tone(running: bool) -> Tone {
    if running { Tone::Go } else { Tone::Dim }
}

pub(super) fn draw(f: &mut Frame) {
    if let Some(menu) = f.s.dock_menu() {
        return draw_menu(f, menu);
    }
    let body = f.body();
    let s = f.s;
    let pad = f.pad();
    let mut area = style::content(body);
    area.take_top(pad);
    if let Some(error) = &s.apps().error {
        let size = f.text(Text::Small);
        let line = area.take_bottom(size * 1.6);
        f.c.text_fit(line.shifted(0.0, size * 0.4).top_line(size), size, Tone::Dim, error, Keep::Start);
    }
    let games = s.session().games();
    let game_tiles = games.len().max(1);
    let rows = (s.apps().list.len() + game_tiles).div_ceil(COLUMNS).max(MIN_ROWS);
    let gap = f.body().h * GAP;
    let cell = |i| area.cell(COLUMNS, rows, gap, i);
    if games.is_empty() {
        start_tile(f, cell(0));
    }
    for (i, running) in games.iter().enumerate() {
        // The game in front is marked only when there is another to tell it from.
        let marked = s.current_game() == Some(&running.id) && games.len() > 1;
        game_tile(f, cell(i), i, s.game(&running.id), marked);
    }
    for (i, app) in s.apps().list.iter().enumerate() {
        app_tile(f, cell(i + game_tiles), i, app);
    }
}

/// While no game runs: to shelf, to start one.
fn start_tile(f: &mut Frame, r: Rect) {
    widgets::tile(f, Id::Dock(DockId::StartGame), r, &TILE, face("Start game"), |f, r| {
        f.c.icon(Icon::Shelf, TILE.square(r), Tone::Dim);
    });
}

/// A running game: its box art in colour (it runs), as tall as the room above the caption allows;
/// `marked` (the game in front of several) with a dot.
fn game_tile(f: &mut Frame, r: Rect, i: usize, game: Option<&Game>, marked: bool) {
    let caption = game.map_or("Game", |g| g.title.as_str());
    let face = TileFace { dot: marked.then_some(Tone::Go), ..face(caption) };
    widgets::tile(f, Id::Dock(DockId::GameTile(i as u16)), r, &TILE, face, |f, r| {
        let Some(game) = game else {
            f.c.icon(Icon::Gamepad, TILE.square(r), Tone::Go);
            return;
        };
        let h = r.h * (CAPTION_Y - TILE.icon_y * ART_Y - ART_ROOM);
        let art = Rect::new(r.x + (r.w - h * 2.0 / 3.0) / 2.0, r.y + r.h * TILE.icon_y * ART_Y, h * 2.0 / 3.0, h);
        widgets::art(f.c, f.images, game, art, ART_RADIUS, Align::Center);
    });
}

fn app_tile(f: &mut Frame, r: Rect, i: usize, app: &App) {
    let running = f.s.app_running(i);
    let face = face(&app.name);
    widgets::tile(f, Id::Dock(DockId::App(i as u16)), r, &TILE, face, |f, r| {
        let square = TILE.square(r);
        let scale = f.c.scale();
        let drawn = Icon::for_app(&app.icon);
        let image = match (drawn, &app.icon_file) {
            (None, Some(file)) => {
                let px = (square.w * scale).round() as u32;
                f.images.get(file, std::path::Path::new(&**file), px, px)
            }
            _ => None,
        };
        match (drawn, image) {
            (Some(icon), _) => f.c.icon(icon, square, running_tone(running)),
            (None, Some(img)) => {
                let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
                let (x, y) = (square.x + (square.w - iw) / 2.0, square.y + (square.h - ih) / 2.0);
                if running {
                    f.c.image(img, x, y);
                } else {
                    f.c.image_grey(img, x, y, IDLE);
                }
            }
            // No icon: the name's initial in a ring.
            (None, None) => {
                let tone = running_tone(running);
                let initial = app.name.chars().next().map_or("", |ch| app.name.get(..ch.len_utf8()).unwrap_or(""));
                let size = square.w;
                f.c.outline(square.inset(size * 0.06), size * 0.44, 1.8 * size / 24.0, tone);
                f.c.text_in(square, size * 0.5, tone, initial, Align::Center);
            }
        }
    });
}

/// A running app's or game's menu, from a long press on its tile: Open another (an app that allows
/// several), Close window (the one in view, if it has one), Quit, Force kill; a game's Quit and Force
/// kill. The back arrow returns to the dock.
fn draw_menu(f: &mut Frame, menu: DockMenu) {
    let s = f.s;
    let (title, items) = match menu {
        DockMenu::App(i) => {
            let i = usize::from(i);
            let name = s.apps().list.get(i).map_or("", |a| a.name.as_str());
            let items: Vec<(Id, &str, Tone)> = [
                s.offers_new(i).then_some((Id::Dock(DockId::OpenAnother), "Open another", Tone::Fn)),
                s.app_window(i).map(|_| (Id::Dock(DockId::CloseWindow), "Close window", Tone::Fn)),
                Some((Id::Dock(DockId::Quit), "Quit", Tone::Accent)),
                Some((Id::Dock(DockId::ForceKill), "Force kill", Tone::Accent)),
            ]
            .into_iter()
            .flatten()
            .collect();
            (name, items)
        }
        DockMenu::Game(i) => {
            let title = s.session().games().get(usize::from(i)).and_then(|g| s.game(&g.id)).map_or("Game", |g| g.title.as_str());
            (title, vec![(Id::Dock(DockId::Quit), "Quit", Tone::Accent), (Id::Dock(DockId::ForceKill), "Force kill", Tone::Accent)])
        }
    };
    let area = widgets::header(f, Some(Id::Dock(DockId::Back)), title, false, HeaderRight::None);
    widgets::menu(f, area, &items);
}
