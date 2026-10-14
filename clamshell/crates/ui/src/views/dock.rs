//! The dock: the running games first (the game, the one last in front, marked), or while none runs a
//! "Start game" tile that goes to shelf (so the apps keep their places unless several games run); then
//! the apps from launcher.json, as a grid of tiles on black, each an icon over its name. A tap starts an
//! app or goes to it (again: its next window), or shows a game; a long press closes the window it is on,
//! or quits the game; a "+" in the corner of a running app that allows several starts another. Running shows as
//! colour: app icons in full colour (drawn
//! icons green), idle ones grey and dim; "1/3" when it has several instances, the one shown first. What is
//! wrong with the file, if anything, shows at the bottom.

use std::fmt::Write;

use clamshell_core::apps::App;
use clamshell_core::{Game, Id, State};

use super::{DIM, Frame, GO, Look, TEXT, label};
use crate::draw::{Canvas, Keep, Rect, rgb};
use crate::icons::Icon;
use crate::images::Images;

/// Padding and the error line's size, of the area's height.
const PAD: f32 = 0.03;
const ERROR: f32 = 0.032;
/// The grid: at least this many rows, so a few apps keep tile-sized tiles.
const COLUMNS: usize = 4;
const MIN_ROWS: usize = 4;
/// Gap between tiles, of the area's height.
const GAP: f32 = 0.014;
/// Icon size, name size, and where they start, of a tile's height.
const ICON: f32 = 0.42;
const ICON_Y: f32 = 0.1;
const NAME: f32 = 0.15;
const NAME_Y: f32 = 0.62;
/// The window position ("1/3") size, of a tile's height.
const COUNT: f32 = 0.11;
/// The "+" button's tap area (about 6 mm on the Thor) and the drawn "+", of a tile's height.
const PLUS: f32 = 0.45;
const PLUS_ICON: f32 = 0.17;
/// An idle app's icon image: grey at this brightness.
const IDLE: f32 = 0.55;

const LOOK: Look = Look { radius: 10.0, fill: None, inset: 0.0, ring_inset: -2.0 };

fn icon(name: &str) -> Option<Icon> {
    match name {
        "shelf" => Some(Icon::Shelf),
        "gamepad" => Some(Icon::Gamepad),
        "terminal" => Some(Icon::Env),
        "browser" => Some(Icon::Browser),
        "files" => Some(Icon::Files),
        "music" => Some(Icon::Music),
        "video" => Some(Icon::Video),
        "settings" => Some(Icon::Sliders),
        _ => None,
    }
}

pub(super) fn draw(f: &mut Frame, body: Rect, images: &mut Images, buf: &mut String) {
    let s = f.s;
    let pad = body.h * PAD;
    let mut area = body.inset(pad);
    if let Some(error) = &s.apps_error {
        let size = body.h * ERROR;
        area.h -= size * 1.6;
        f.c.text_fit((area.x, area.y + area.h + size * 0.4, area.w), size, rgb(DIM), error, Keep::Start);
    }
    let games = s.games.len().max(1);
    let rows = (s.apps.len() + games).div_ceil(COLUMNS).max(MIN_ROWS);
    let gap = body.h * GAP;
    if s.games.is_empty() {
        start_tile(f, area.cell(COLUMNS, rows, gap, 0));
    }
    for (i, running) in s.games.iter().enumerate() {
        let game = s.library.games.iter().find(|g| g.id == running.id);
        let the_game = s.running.as_ref() == Some(&running.id);
        game_tile(f, area.cell(COLUMNS, rows, gap, i), i, game, the_game && s.games.len() > 1, images);
    }
    for (i, app) in s.apps.iter().enumerate() {
        app_tile(f, area.cell(COLUMNS, rows, gap, i + games), i, app, images, buf);
    }
}

/// "2/3" into `buf` when there are several windows, else nothing.
fn position(buf: &mut String, (at, count): (usize, usize)) {
    buf.clear();
    if count > 1 {
        let _ = write!(buf, "{}/{count}", at.max(1));
    }
}

/// The name under the icon, and the window position in the top right corner.
fn caption(c: &mut Canvas, r: Rect, name: &str, color: u32, position: &str) {
    let size = r.h * NAME;
    let w = c.text_width(size, name).min(r.w - 8.0);
    let y = r.y + r.h * NAME_Y + (r.h * NAME * 1.6 - size * 1.2) / 2.0;
    c.text_fit((r.x + (r.w - w) / 2.0, y, r.w - 8.0), size, rgb(color), name, Keep::Start);
    if !position.is_empty() {
        let size = r.h * COUNT;
        let w = c.text_width(size, position);
        c.text(r.x + r.w - w - r.h * 0.06, r.y + r.h * 0.05, size, rgb(TEXT), position);
    }
}

fn icon_square(r: Rect) -> Rect {
    let size = r.h * ICON;
    Rect::new(r.x + (r.w - size) / 2.0, r.y + r.h * ICON_Y, size, size)
}

/// While no game runs: to shelf, to start one.
fn start_tile(f: &mut Frame, r: Rect) {
    f.button(Id::StartGame, r, LOOK, |c| {
        c.icon(Icon::Shelf, icon_square(r), rgb(DIM));
        caption(c, r, "Start game", TEXT, "");
    });
}

/// A running game: its box art in colour; the game (the one last in front) marked with a dot when
/// several run.
fn game_tile(f: &mut Frame, r: Rect, i: usize, game: Option<&Game>, the_game: bool, images: &mut Images) {
    let scale = f.c.scale();
    f.button(Id::GameTile(i as u16), r, LOOK, |c| {
        if the_game {
            c.circle(r.x + r.w - r.h * 0.1, r.y + r.h * 0.1, r.h * 0.03, rgb(GO));
        }
        let Some(game) = game else {
            c.icon(Icon::Gamepad, icon_square(r), rgb(GO));
            caption(c, r, "Game", TEXT, "");
            return;
        };
        // Its box art, as tall as the icon area allows, in colour: it runs.
        let h = r.h * (NAME_Y - ICON_Y * 0.6 - 0.03);
        let art = Rect::new(r.x + (r.w - h * 2.0 / 3.0) / 2.0, r.y + r.h * ICON_Y * 0.6, h * 2.0 / 3.0, h);
        let (pw, ph) = ((art.w * scale).round() as u32, (art.h * scale).round() as u32);
        match game.box_front.as_deref().and_then(|p| images.get(&game.id, p, pw, ph)) {
            Some(img) => {
                let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
                c.image(img, art.x + (art.w - iw) / 2.0, art.y + (art.h - ih) / 2.0);
            }
            None => c.rounded(art, 4.0, rgb(game.color)),
        }
        caption(c, r, &game.title, TEXT, "");
    });
}

fn app_tile(f: &mut Frame, r: Rect, i: usize, app: &App, images: &mut Images, buf: &mut String) {
    let s: &State = f.s;
    let (at, count) = s.instance_position(i);
    let running = s.app_running(i);
    position(buf, (at, count));
    let position: &str = buf;
    let scale = f.c.scale();
    f.button(Id::App(i as u16), r, LOOK, |c| {
        let square = icon_square(r);
        let drawn = icon(&app.icon);
        let image = match (drawn, &app.icon_file) {
            (None, Some(file)) => {
                let px = (square.w * scale).round() as u32;
                images.get(file, std::path::Path::new(&**file), px, px)
            }
            _ => None,
        };
        match (drawn, image) {
            (Some(icon), _) => c.icon(icon, square, rgb(if running { GO } else { DIM })),
            (None, Some(img)) => {
                let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
                let (x, y) = (square.x + (square.w - iw) / 2.0, square.y + (square.h - ih) / 2.0);
                if running {
                    c.image(img, x, y);
                } else {
                    c.image_grey(img, x, y, IDLE);
                }
            }
            // No icon: the name's initial in a ring.
            (None, None) => {
                let color = if running { GO } else { DIM };
                let initial = app.name.chars().next().map_or("", |ch| app.name.get(..ch.len_utf8()).unwrap_or(""));
                let size = square.w;
                c.outline(square.inset(size * 0.06), size * 0.44, 1.8 * size / 24.0, rgb(color));
                label(c, square, size * 0.5, color, initial);
            }
        }
        caption(c, r, &app.name, TEXT, position);
    });
    // "+": another instance, a button of its own in the top left corner (drawn after the tile, so a tap
    // there is its).
    if s.offers_new(i) {
        let side = r.h * PLUS;
        let plus = Rect::new(r.x, r.y, side, side * 0.8);
        f.button(Id::NewInstance(i as u16), plus, Look { radius: 8.0, ..LOOK }, |c| {
            let (cx, cy) = plus.center();
            let size = r.h * PLUS_ICON;
            // Green: it starts something.
            c.icon(Icon::Plus, Rect::new(cx - size / 2.0, cy - size / 2.0, size, size), rgb(GO));
        });
    }
}
