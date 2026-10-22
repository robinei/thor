//! The battery tab: the level and what it is doing (drain, time left) on top, beside the tiles of the
//! battery's settings (the charge limit, when the screens go off and the device sleeps without input), drawn
//! and stepped as in the settings grid; the charge over the last day (UPower's history; charging in green);
//! then what is using the machine now, the busiest first, then those idle that hold much memory, as many as
//! fit: each a bar of its share of the work, its CPU, GPU and memory. The battery reports only its total
//! drain, so programs are ranked by work, not watts. A row (not the system's) opens its menu in the tab's
//! place: Quit, Force kill.

use clamshell_core::battery::{BatteryId, Group};
use clamshell_core::settings::Setting;
use clamshell_core::{Id, State};

use crate::draw::{Align, Keep, LINE, Rect};
use crate::toolkit::Frame;
use crate::toolkit::format::{Bytes, Duration, Fmt, MaybeShare, Share};
use crate::toolkit::style::{self, Look, SURFACE, SURFACE_HI, Tone};
use crate::toolkit::widgets::{self, HeaderRight};

/// The top row's height, the level's and the other text's sizes, of the area's height. (The text is
/// a little smaller than other tabs' body text, for the figures to fit.)
const TOP_H: f32 = 0.22;
/// The settings tiles beside the level, and a tile's width, of the row's (near the settings grid's).
const TILES: [Setting; 3] = [Setting::ChargeLimit, Setting::ScreenOff, Setting::Sleep];
const TILE_W: f32 = 0.23;
/// A tile's height, of the area's height (the settings grid's).
const TILE_H: f32 = 0.19;
const LEVEL: f32 = 0.075;
const TEXT_SIZE: f32 = 0.036;
/// The graph's height, of the area's height; the line's width.
const GRAPH_H: f32 = 0.28;
const STROKE: f32 = 2.0;
/// A spender's row height, of the area's height; its name column, of the width.
const ROW_H: f32 = 0.072;
const NAME_W: f32 = 0.3;
/// Secondary text (the list's title, the figures), of the text size.
const SMALL: f32 = 0.85;
/// The history's span, seconds.
const SPAN: f32 = 24.0 * 3600.0;

pub(super) fn draw(f: &mut Frame) {
    if let Some(group) = f.s.spender_menu().cloned() {
        return spender_menu(f, &group);
    }
    let body = f.body();
    let s = f.s;
    let pad = f.pad();
    let mut rest = style::content(body);
    let size = f.body().h * TEXT_SIZE;
    let top = rest.take_top(f.body().h * TOP_H);
    top_line(f, top, f.body().h);

    let graph = rest.take_top(f.body().h * GRAPH_H);
    let graph = Rect::new(graph.x, graph.y + pad * 0.5, graph.w, graph.h - pad * 1.5);
    history(f, graph, size);

    // What is using the machine, as many as fit.
    let title_y = rest.y + pad * 0.3;
    f.c.text(rest.x, title_y, size * SMALL, Tone::Dim, "Using the machine now");
    let list = Rect::new(rest.x, title_y + size * 1.6, rest.w, rest.y + rest.h - title_y - size * 1.6);
    let row_h = f.body().h * ROW_H;
    let rows = (list.h / row_h).floor().max(0.0) as usize;
    let spenders = &s.sys().battery.spenders;
    if spenders.is_empty() {
        f.c.text(list.x, list.y, size, Tone::Dim, "Measuring");
    }
    let total: f32 = spenders.iter().map(|p| p.load()).sum::<f32>().max(f32::EPSILON);
    // The figures' column, as wide as the widest they can be, so the bars line up.
    let figures_x = list.x + list.w - f.c.text_width(size * SMALL, "CPU 100% · GPU 100% · 999 MB");
    for (i, p) in spenders.iter().enumerate().take(rows) {
        let r = Rect::new(list.x, list.y + i as f32 * row_h, list.w, row_h);
        let row = |f: &mut Frame| {
            let name_w = r.w * NAME_W;
            f.c.text_fit(Rect::new(r.x, r.y, name_w - pad, r.h), size, Tone::Normal, s.spender_name(p), Keep::Start);
            let figures = Fmt::<64>::of(format_args!("CPU {} · GPU {} · {}", Share(p.cpu), MaybeShare(p.group.gpu_known().then_some(p.gpu)), Bytes(p.memory)));
            let figures_w = f.c.text_width(size * SMALL, &figures);
            // Smaller, on the name's line.
            let ty = r.y + (r.h - size * LINE) / 2.0;
            f.c.text(r.x + r.w - figures_w, ty + size * 0.1, size * SMALL, Tone::Dim, &figures);
            let track = Rect::new(r.x + name_w, r.y + r.h * 0.42, figures_x - pad - r.x - name_w, r.h * 0.16);
            f.c.rounded(track, track.h / 2.0, SURFACE);
            let share = (p.load() / total).clamp(0.0, 1.0);
            if share > 0.0 {
                f.c.rounded(Rect::new(track.x, track.y, (track.w * share).max(track.h), track.h), track.h / 2.0, Tone::Normal);
            }
        };
        // (The system's and the session's own aren't to be ended from here.)
        if p.endable {
            f.button(Id::Battery(BatteryId::Spender(i as u16)), r, Look::ROW, row);
        } else {
            row(f);
        }
    }
}

/// A row's menu, from a tap on it: Quit and Force kill what it is (an entry as from the dock; another
/// program, its processes). The back arrow returns to the tab.
fn spender_menu(f: &mut Frame, group: &Group) {
    let s = f.s;
    let name = s.sys().battery.spenders.iter().find(|p| p.group == *group).map_or("", |p| s.spender_name(p));
    let area = widgets::header(f, Some(Id::Battery(BatteryId::Back)), name, false, HeaderRight::None);
    let items = [(Id::Battery(BatteryId::Quit), "Quit", Tone::Accent), (Id::Battery(BatteryId::ForceKill), "Force kill", Tone::Accent)];
    widgets::menu(f, area, &items);
}

/// The level with the state (and drain) and the time left under it, and the settings tiles on the
/// right.
fn top_line(f: &mut Frame, top: Rect, body_h: f32) {
    let s = f.s;
    let level_size = body_h * LEVEL;
    let size = body_h * TEXT_SIZE;
    let mut row = top;
    let tile_w = top.w * TILE_W;
    // As tall as the settings grid's tiles (their captions the same size), in the row's middle.
    let tile_h = body_h * TILE_H;
    for setting in TILES.into_iter().rev() {
        let column = row.take_right(tile_w);
        let tile = Rect::new(column.x, column.y + (column.h - tile_h) / 2.0, column.w, tile_h);
        super::settings::setting_tile(f, tile, setting);
    }
    // The level and two lines under it, in the middle of the row.
    let mut lines = Rect::new(row.x, row.y + (row.h - level_size * LINE - 2.0 * size * LINE) / 2.0, row.w - size, row.h);
    let Some(level) = s.sys().status.battery else {
        f.c.text_in(lines.take_top(level_size * LINE), size, Tone::Dim, "No battery", Align::Start);
        return;
    };
    let level = Fmt::<8>::of(format_args!("{level}%"));
    f.c.text_in(lines.take_top(level_size * LINE), level_size, Tone::High, &level, Align::Start);
    let (state, left) = drain(s);
    f.c.text_fit(lines.take_top(size * LINE), size, Tone::Normal, &state, Keep::Start);
    f.c.text_fit(lines.take_top(size * LINE), size, Tone::Dim, &left, Keep::Start);
}

/// What the battery is doing ("Discharging · 6.2 W", "Charging · 12.0 W", "Charging or full"), and
/// the time left ("3 h 10 min left") when discharging.
fn drain(s: &State) -> (Fmt<64>, Fmt<64>) {
    let sys = s.sys();
    let state = match (sys.status.charging, sys.metrics.power_w) {
        (true, Some(w)) if w.abs() > 0.05 => Fmt::of(format_args!("Charging · {:.1} W", w.abs())),
        (true, _) => Fmt::of(format_args!("Charging or full")),
        (false, Some(w)) => Fmt::of(format_args!("Discharging · {w:.1} W")),
        (false, None) => Fmt::of(format_args!("Discharging")),
    };
    let left = match (sys.status.charging, sys.metrics.minutes_left) {
        (false, Some(m)) => Fmt::of(format_args!("{} left", Duration(m))),
        _ => Fmt::of(format_args!("")),
    };
    (state, left)
}

/// The last day's charge: 0, 50 and 100% as faint lines, the level as a line (green while charging),
/// carried on to now at the current level; the times below.
fn history(f: &mut Frame, g: Rect, size: f32) {
    let s = f.s;
    let mut plot = g;
    let labels = plot.take_bottom(size * 1.4);
    for level in [0.0, 0.5, 1.0] {
        f.c.fill(Rect::new(plot.x, plot.y + plot.h * (1.0 - level), plot.w, 1.0), SURFACE_HI);
    }
    let small = size * 0.8;
    f.c.text_in(labels, small, Tone::Dim, "24 h ago", Align::Start);
    f.c.text_in(labels, small, Tone::Dim, "12 h", Align::Center);
    f.c.text_in(labels, small, Tone::Dim, "now", Align::End);
    if !s.sys().battery.has_history {
        f.c.text_in(plot, size, Tone::Dim, "No charge history from UPower", Align::Center);
        return;
    }
    let now = s.unix_secs() as f32;
    let x_at = |at: u64| plot.x + ((at as f32 - (now - SPAN)) / SPAN).clamp(0.0, 1.0) * plot.w;
    let y_at = |level: u8| plot.y + plot.h * (1.0 - f32::from(level) / 100.0);
    // A step line of rectangles (no paths: drawing doesn't allocate): along at each level, then up or
    // down to the next.
    let step = |f: &mut Frame, from: (u64, u8), to: (u64, u8), charging: bool| {
        let tone = if charging { Tone::Go } else { Tone::Normal };
        let (x0, x1, y0, y1) = (x_at(from.0), x_at(to.0), y_at(from.1), y_at(to.1));
        f.c.fill(Rect::new(x0, y0 - STROKE / 2.0, (x1 - x0).max(STROKE), STROKE), tone);
        f.c.fill(Rect::new(x1 - STROKE / 2.0, y0.min(y1) - STROKE / 2.0, STROKE, (y1 - y0).abs() + STROKE), tone);
    };
    let history = &s.sys().battery.history;
    for pair in history.windows(2) {
        if let [a, b] = pair {
            step(f, (a.at, a.level), (b.at, b.level), a.charging);
        }
    }
    // From the last point to now, at today's level.
    if let (Some(last), Some(level)) = (history.last(), s.sys().status.battery) {
        step(f, (last.at, last.level), (s.unix_secs(), level), s.sys().status.charging);
    }
    if history.is_empty() {
        f.c.text_in(plot, size, Tone::Dim, "No changes recorded yet", Align::Center);
    }
}
