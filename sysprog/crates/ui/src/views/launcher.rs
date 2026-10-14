//! The launcher: the apps from launcher.json as a grid of tiles, each an icon over its name, on black. A
//! tap starts the app (shelf is built in: it comes to the front). An app's icon is green while it runs,
//! with a count when more than one does. What is wrong with the file, if anything, shows at the bottom.

use std::fmt::Write;

use sysprog_core::Id;
use sysprog_core::apps::App;

use super::{DIM, Frame, GO, Look, TEXT, label};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

/// Padding and the error line's size, of the area's height.
const PAD: f32 = 0.03;
const ERROR: f32 = 0.032;
/// The grid: at least this many rows, so a few apps keep tile-sized tiles.
const COLUMNS: usize = 4;
const MIN_ROWS: usize = 4;
/// Gap between tiles, of the area's height.
const GAP: f32 = 0.014;
/// Icon size and name size, of a tile's height.
const ICON: f32 = 0.36;
const NAME: f32 = 0.15;

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

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    let s = f.s;
    let pad = body.h * PAD;
    let mut area = body.inset(pad);
    if let Some(error) = &s.apps_error {
        let size = body.h * ERROR;
        area.h -= size * 1.6;
        f.c.text_fit((area.x, area.y + area.h + size * 0.4, area.w), size, rgb(DIM), error, Keep::Start);
    }
    let rows = s.apps.len().div_ceil(COLUMNS).max(MIN_ROWS);
    for (i, app) in s.apps.iter().enumerate() {
        let running = s.apps_running.get(i).copied().unwrap_or(0);
        tile(f, area.cell(COLUMNS, rows, body.h * GAP, i), i, app, running, buf);
    }
}

fn tile(f: &mut Frame, r: Rect, i: usize, app: &App, running: u16, buf: &mut String) {
    let look = Look { radius: 10.0, fill: None, inset: 0.0, ring_inset: -2.0 };
    let color = if running > 0 { GO } else { DIM };
    buf.clear();
    if running > 1 {
        let _ = write!(buf, "{running}");
    }
    let count: &str = buf;
    f.button(Id::App(i as u16), r, look, |c| {
        let size = r.h * ICON;
        let square = Rect::new(r.x + (r.w - size) / 2.0, r.y + r.h * 0.17, size, size);
        match icon(&app.icon) {
            Some(icon) => c.icon(icon, square, rgb(color)),
            // No icon: the name's initial in a ring.
            None => {
                let initial = app.name.chars().next().map_or("", |ch| app.name.get(..ch.len_utf8()).unwrap_or(""));
                c.outline(square.inset(size * 0.06), size * 0.44, 1.8 * size / 24.0, rgb(color));
                label(c, square, size * 0.5, color, initial);
            }
        }
        let name_size = r.h * NAME;
        let ny = r.y + r.h * 0.62 + (r.h * NAME * 1.6 - name_size * 1.2) / 2.0;
        let w = c.text_width(name_size, &app.name).min(r.w - 8.0);
        c.text_fit((r.x + (r.w - w) / 2.0, ny, r.w - 8.0), name_size, rgb(TEXT), &app.name, Keep::Start);
        if !count.is_empty() {
            c.text(r.x + r.w * 0.5 + size * 0.55, r.y + r.h * 0.14, r.h * 0.1, rgb(GO), count);
        }
    });
}
