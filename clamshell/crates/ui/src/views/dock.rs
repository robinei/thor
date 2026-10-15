//! The dock: the running games first (the game, the one last in front, marked), or while none runs a
//! "Start game" tile that goes to shelf (so the apps keep their places unless several games run); then
//! the apps from launcher.json, as a grid of tiles on black, each an icon over its name. A tap starts an
//! app or goes to it (again: its next instance), or shows a game; a long press closes the instance it is
//! on, or quits the game; a "+" in the corner of a running app that allows several starts another.
//! Running shows as colour: app icons in full colour (drawn icons green), idle ones grey and dim; "1/3"
//! when it has several instances, the one shown first. What is wrong with the file, if anything, shows at
//! the bottom.

use clamshell_core::apps::App;
use clamshell_core::{DockId, Game, Id};

use super::format::{Fmt, Position};
use super::style::{self, Look, Text, Tone};
use super::widgets::{self, CAPTION_Y, TileFace, TileShape};
use super::Frame;
use crate::draw::{Align, Keep, Rect};
use crate::icons::Icon;

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
/// The "+" button's tap area (about 6 mm on the Thor) and the drawn "+", of a tile's height.
const PLUS: f32 = 0.45;
const PLUS_ICON: f32 = 0.17;
/// An idle app's icon image: grey at this brightness.
const IDLE: f32 = 0.55;

fn face(caption: &str) -> TileFace<'_> {
    TileFace { caption, caption_tone: Tone::Normal, dot: None, note: "", count: "" }
}

fn running_tone(running: bool) -> Tone {
    if running { Tone::Go } else { Tone::Dim }
}

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let s = f.s;
    let pad = style::pad(body.h);
    let mut area = style::content(body);
    area.take_top(pad);
    if let Some(error) = &s.apps().error {
        let size = Text::Small.size(body.h);
        let line = area.take_bottom(size * 1.6);
        f.c.text_fit(line.shifted(0.0, size * 0.4).top_line(size), size, Tone::Dim, error, Keep::Start);
    }
    let games = s.session().games();
    let game_tiles = games.len().max(1);
    let rows = (s.apps().list.len() + game_tiles).div_ceil(COLUMNS).max(MIN_ROWS);
    let cell = |i| area.cell(COLUMNS, rows, body.h * GAP, i);
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
    let s = f.s;
    let (at, count) = s.instance_position(i);
    let running = s.app_running(i);
    let position = Fmt::<16>::of(format_args!("{}", Position(at, count)));
    let face = TileFace { count: &position, ..face(&app.name) };
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
    // "+": another instance, a button of its own in the top left corner (drawn after the tile, so a tap
    // there is its).
    if s.offers_new(i) {
        let side = r.h * PLUS;
        let plus = Rect::new(r.x, r.y, side, side * 0.8);
        f.button(Id::Dock(DockId::NewInstance(i as u16)), plus, Look { fill: None, ..Look::CARD }, |f| {
            // Green: it starts something.
            f.c.icon(Icon::Plus, plus.square_center(r.h * PLUS_ICON), Tone::Go);
        });
    }
}
