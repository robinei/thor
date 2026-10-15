//! The battery mode: the level, what it is doing (drain, time left) and the charge limit's button on top;
//! the charge over the last day (UPower's history; charging in green) below; then what is using the
//! machine now, the busiest first, as many as fit: each a bar of its share of the work and its CPU and GPU.
//! The battery reports only its total drain, so programs are ranked by work, not watts.

use std::fmt::Write;

use clamshell_core::settings::Setting;
use clamshell_core::{Id, State};

use super::{DIM, Frame, GO, Look, SURFACE, SURFACE_HI, TEXT, TEXT_HI, label};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

/// Padding, of the area's height.
const PAD: f32 = 0.03;
/// The top line's height, the level's and the detail's text sizes, of the area's height.
const TOP_H: f32 = 0.12;
const LEVEL: f32 = 0.075;
const TEXT_SIZE: f32 = 0.036;
/// The graph's height, of the area's height; the line's width.
const GRAPH_H: f32 = 0.34;
const LINE: f32 = 2.0;
/// A spender's row height, of the area's height; its name column, of the width.
const ROW_H: f32 = 0.072;
const NAME_W: f32 = 0.3;
/// The history's span, seconds.
const SPAN: f32 = 24.0 * 3600.0;

pub(super) fn draw(f: &mut Frame, body: Rect, buf: &mut String) {
    let s = f.s;
    let pad = body.h * PAD;
    let area = Rect::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - pad);
    let size = body.h * TEXT_SIZE;
    let (top, rest) = area.split_top(body.h * TOP_H);
    top_line(f, top, body.h, buf);

    let (graph, rest) = rest.split_top(body.h * GRAPH_H);
    let graph = Rect::new(graph.x, graph.y + pad * 0.5, graph.w, graph.h - pad * 1.5);
    history(f, graph, size);

    // What is using the machine, as many as fit.
    let title_y = rest.y + pad * 0.3;
    f.c.text(rest.x, title_y, size * 0.85, rgb(DIM), "Using the machine now");
    let list = Rect::new(rest.x, title_y + size * 1.6, rest.w, rest.y + rest.h - title_y - size * 1.6);
    let row_h = body.h * ROW_H;
    let rows = (list.h / row_h).floor().max(0.0) as usize;
    if s.battery.spenders.is_empty() {
        f.c.text(list.x, list.y, size, rgb(DIM), "Measuring");
    }
    let total: f32 = s.battery.spenders.iter().map(|p| p.load()).sum::<f32>().max(f32::EPSILON);
    // The figures' column, as wide as the widest they can be, so the bars line up.
    let figures_x = list.x + list.w - f.c.text_width(size * 0.85, "CPU 100% · GPU 100%");
    for (i, p) in s.battery.spenders.iter().enumerate().take(rows) {
        let r = Rect::new(list.x, list.y + i as f32 * row_h, list.w, row_h);
        let ty = r.y + (r.h - size * 1.2) / 2.0;
        let name_w = r.w * NAME_W;
        f.c.text_fit((r.x, ty, name_w - pad), size, rgb(TEXT), s.spender_name(p), Keep::Start);
        buf.clear();
        let _ = write!(buf, "CPU {} · GPU {}", Share(p.cpu), Share(p.gpu));
        let figures_w = f.c.text_width(size * 0.85, buf);
        f.c.text(r.x + r.w - figures_w, ty + size * 0.1, size * 0.85, rgb(DIM), buf);
        let track = Rect::new(r.x + name_w, r.y + r.h * 0.42, figures_x - pad - r.x - name_w, r.h * 0.16);
        f.c.rounded(track, track.h / 2.0, rgb(SURFACE));
        let share = (p.load() / total).clamp(0.0, 1.0);
        if share > 0.0 {
            f.c.rounded(Rect::new(track.x, track.y, (track.w * share).max(track.h), track.h), track.h / 2.0, rgb(TEXT));
        }
    }
}

/// A share as a percentage: one decimal below 10 %, so small ones don't all read 0 %.
struct Share(f32);

impl std::fmt::Display for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let percent = self.0 * 100.0;
        if percent < 9.95 { write!(f, "{percent:.1}%") } else { write!(f, "{percent:.0}%") }
    }
}

/// The level, the state with drain and time left, and the charge limit's button on the right.
fn top_line(f: &mut Frame, top: Rect, body_h: f32, buf: &mut String) {
    let s = f.s;
    let level_size = body_h * LEVEL;
    let size = body_h * TEXT_SIZE;

    // The charge limit: a tap steps it (full, 90, 80), as the settings tile does.
    let limit = s.settings.global.get(Setting::ChargeLimit);
    Setting::ChargeLimit.describe(limit, buf);
    let text_w = f.c.text_width(size, buf);
    let icon = size * 1.4;
    let button_w = text_w + icon + size * 2.0;
    let b = Rect::new(top.x + top.w - button_w, top.y + top.h * 0.15, button_w, top.h * 0.7);
    let look = Look { radius: 8.0, fill: Some(SURFACE_HI), inset: 0.0, ring_inset: -2.0 };
    let color = if limit < 100 { GO } else { DIM };
    let text: &str = buf;
    f.button(Id::Tile(Setting::ChargeLimit), b, look, |c| {
        c.icon(Icon::Battery { tenths: (limit / 10) as u8, charging: true }, Rect::new(b.x + size * 0.6, b.y + (b.h - icon) / 2.0, icon, icon), rgb(color));
        label(c, Rect::new(b.x + icon + size * 0.8, b.y, text_w + size * 0.6, b.h), size, TEXT, text);
    });

    let Some(level) = s.status.battery else {
        f.c.text(top.x, top.y + (top.h - size * 1.2) / 2.0, size, rgb(DIM), "No battery");
        return;
    };
    buf.clear();
    let _ = write!(buf, "{level}%");
    let ly = top.y + (top.h - level_size * 1.2) / 2.0;
    f.c.text(top.x, ly, level_size, rgb(TEXT_HI), buf);
    let level_w = f.c.text_width(level_size, buf);
    status_text(s, buf);
    let dx = top.x + level_w + size;
    f.c.text_fit((dx, top.y + (top.h - size * 1.2) / 2.0, b.x - dx - size), size, rgb(TEXT), buf, Keep::Start);
}

/// "Discharging · 6.2 W · 3 h 10 min left", "Charging · 12.0 W", "Full".
fn status_text(s: &State, buf: &mut String) {
    buf.clear();
    let watts = s.metrics.power_w;
    match (s.status.charging, watts) {
        (true, Some(w)) if w.abs() > 0.05 => {
            let _ = write!(buf, "Charging · {:.1} W", w.abs());
        }
        (true, _) => buf.push_str("Charging or full"),
        (false, Some(w)) => {
            let _ = write!(buf, "Discharging · {w:.1} W");
            if let Some(m) = s.metrics.minutes_left {
                let _ = write!(buf, " · {} h {} min left", m / 60, m % 60);
            }
        }
        (false, None) => buf.push_str("Discharging"),
    }
}

/// The last day's charge: 0, 50 and 100% as faint lines, the level as a line (green while charging),
/// carried on to now at the current level; the times below.
fn history(f: &mut Frame, g: Rect, size: f32) {
    let s = f.s;
    let labels_h = size * 1.4;
    let plot = Rect::new(g.x, g.y, g.w, g.h - labels_h);
    for level in [0.0, 0.5, 1.0] {
        f.c.fill(Rect::new(plot.x, plot.y + plot.h * (1.0 - level), plot.w, 1.0), rgb(SURFACE_HI));
    }
    let small = size * 0.8;
    let ly = plot.y + plot.h + (labels_h - small * 1.2) / 2.0;
    f.c.text(plot.x, ly, small, rgb(DIM), "24 h ago");
    let w = f.c.text_width(small, "12 h");
    f.c.text(plot.x + (plot.w - w) / 2.0, ly, small, rgb(DIM), "12 h");
    let w = f.c.text_width(small, "now");
    f.c.text(plot.x + plot.w - w, ly, small, rgb(DIM), "now");
    if !s.battery.has_history {
        label(f.c, plot, size, DIM, "No charge history from UPower");
        return;
    }
    let now = s.unix_secs as f32;
    let x_at = |at: u64| plot.x + ((at as f32 - (now - SPAN)) / SPAN).clamp(0.0, 1.0) * plot.w;
    let y_at = |level: u8| plot.y + plot.h * (1.0 - f32::from(level) / 100.0);
    // A step line of rectangles (no paths: drawing doesn't allocate): along at each level, then up or
    // down to the next.
    let step = |f: &mut Frame, from: (u64, u8), to: (u64, u8), charging: bool| {
        let color = rgb(if charging { GO } else { TEXT });
        let (x0, x1, y0, y1) = (x_at(from.0), x_at(to.0), y_at(from.1), y_at(to.1));
        f.c.fill(Rect::new(x0, y0 - LINE / 2.0, (x1 - x0).max(LINE), LINE), color);
        f.c.fill(Rect::new(x1 - LINE / 2.0, y0.min(y1) - LINE / 2.0, LINE, (y1 - y0).abs() + LINE), color);
    };
    let history = &s.battery.history;
    for pair in history.windows(2) {
        if let [a, b] = pair {
            step(f, (a.at, a.level), (b.at, b.level), a.charging);
        }
    }
    // From the last point to now, at today's level.
    if let (Some(last), Some(level)) = (history.last(), s.status.battery) {
        step(f, (last.at, last.level), (s.unix_secs, level), s.status.charging);
    }
    if history.is_empty() {
        label(f.c, plot, size, DIM, "No changes recorded yet");
    }
}
