//! The launcher: the running game's tile first (a placeholder when none runs, so the tiles never move),
//! then the apps from launcher.json, as a grid of tiles on black, each an icon over its name. A tap
//! starts an app or goes to it (again: its next window); a long press quits the game, or starts another
//! instance of an app that allows several. Running shows as colour: app icons in full colour (drawn
//! icons green), idle ones grey and dim; "1/3" when it has several windows, the one shown first. What is
//! wrong with the file, if anything, shows at the bottom.

use std::fmt::Write;

use sysprog_core::apps::App;
use sysprog_core::{Game, Id, State};

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
/// An idle app's icon image: grey at this brightness.
const IDLE: f32 = 0.55;
/// The placeholder's icon: barely there.
const EMPTY: u32 = 0x34343c;

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
    let rows = (s.apps.len() + 1).div_ceil(COLUMNS).max(MIN_ROWS);
    let gap = body.h * GAP;
    let game = s.running.as_ref().and_then(|id| s.library.games.iter().find(|g| &g.id == id));
    game_tile(f, area.cell(COLUMNS, rows, gap, 0), game, images, buf);
    for (i, app) in s.apps.iter().enumerate() {
        app_tile(f, area.cell(COLUMNS, rows, gap, i + 1), i, app, images, buf);
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

fn game_tile(f: &mut Frame, r: Rect, game: Option<&Game>, images: &mut Images, buf: &mut String) {
    let s: &State = f.s;
    position(buf, s.window_position(|w| s.game_owns(w)));
    let position: &str = buf;
    let scale = f.c.scale();
    f.button(Id::GameTile, r, LOOK, |c| {
        let Some(game) = game else {
            c.icon(Icon::Gamepad, icon_square(r), rgb(EMPTY));
            caption(c, r, "No game", DIM, "");
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
        caption(c, r, &game.title, TEXT, position);
    });
}

fn app_tile(f: &mut Frame, r: Rect, i: usize, app: &App, images: &mut Images, buf: &mut String) {
    let s: &State = f.s;
    let (at, count) = s.window_position(|w| s.app_owns(i, w));
    let running = count > 0 || s.apps_running.get(i).is_some_and(|n| *n > 0);
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
}
