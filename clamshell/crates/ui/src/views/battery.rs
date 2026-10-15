//! The battery tab: the level, what it is doing (drain, time left) and the charge limit's button on top;
//! the charge over the last day (UPower's history; charging in green) below; then what is using the
//! machine now, the busiest first, as many as fit: each a bar of its share of the work and its CPU and GPU.
//! The battery reports only its total drain, so programs are ranked by work, not watts.

use std::fmt::{self, Display, Formatter};

use clamshell_core::settings::Setting;
use clamshell_core::{Id, SettingsId, State};

use super::format::{Duration, Fmt, Share};
use super::style::{self, Look, SURFACE, SURFACE_HI, Tone};
use super::Frame;
use crate::draw::{Align, Keep, LINE, Rect};
use crate::icons::Icon;

/// The top line's height, the level's and the other text's sizes, of the area's height. (The text is
/// a little smaller than other tabs' body text, for the figures to fit.)
const TOP_H: f32 = 0.12;
const LEVEL: f32 = 0.075;
const TEXT_SIZE: f32 = 0.036;
/// The graph's height, of the area's height; the line's width.
const GRAPH_H: f32 = 0.34;
const STROKE: f32 = 2.0;
/// A spender's row height, of the area's height; its name column, of the width.
const ROW_H: f32 = 0.072;
const NAME_W: f32 = 0.3;
/// Secondary text (the list's title, the figures), of the text size.
const SMALL: f32 = 0.85;
/// The history's span, seconds.
const SPAN: f32 = 24.0 * 3600.0;

pub(super) fn draw(f: &mut Frame, body: Rect) {
    let s = f.s;
    let pad = style::pad(body.h);
    let mut rest = style::content(body);
    let size = body.h * TEXT_SIZE;
    let top = rest.take_top(body.h * TOP_H);
    top_line(f, top, body.h);

    let graph = rest.take_top(body.h * GRAPH_H);
    let graph = Rect::new(graph.x, graph.y + pad * 0.5, graph.w, graph.h - pad * 1.5);
    history(f, graph, size);

    // What is using the machine, as many as fit.
    let title_y = rest.y + pad * 0.3;
    f.c.text(rest.x, title_y, size * SMALL, Tone::Dim, "Using the machine now");
    let list = Rect::new(rest.x, title_y + size * 1.6, rest.w, rest.y + rest.h - title_y - size * 1.6);
    let row_h = body.h * ROW_H;
    let rows = (list.h / row_h).floor().max(0.0) as usize;
    let spenders = &s.sys().battery.spenders;
    if spenders.is_empty() {
        f.c.text(list.x, list.y, size, Tone::Dim, "Measuring");
    }
    let total: f32 = spenders.iter().map(|p| p.load()).sum::<f32>().max(f32::EPSILON);
    // The figures' column, as wide as the widest they can be, so the bars line up.
    let figures_x = list.x + list.w - f.c.text_width(size * SMALL, "CPU 100% · GPU 100%");
    for (i, p) in spenders.iter().enumerate().take(rows) {
        let r = Rect::new(list.x, list.y + i as f32 * row_h, list.w, row_h);
        let name_w = r.w * NAME_W;
        f.c.text_fit(Rect::new(r.x, r.y, name_w - pad, r.h), size, Tone::Normal, s.spender_name(p), Keep::Start);
        let figures = Fmt::<64>::of(format_args!("CPU {} · GPU {}", Share(p.cpu), Share(p.gpu)));
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
    }
}

/// The level, the state with drain and time left, and the charge limit's button on the right.
fn top_line(f: &mut Frame, top: Rect, body_h: f32) {
    let s = f.s;
    let level_size = body_h * LEVEL;
    let size = body_h * TEXT_SIZE;

    // The charge limit: a tap steps it (full, 90, 80), as the settings tile does.
    let limit = s.settings().global.get(Setting::ChargeLimit);
    let text = f.scratch.describe(Setting::ChargeLimit, limit);
    let text_w = f.c.text_width(size, &text);
    let icon = size * 1.4;
    let button_w = text_w + icon + size * 2.0;
    let b = Rect::new(top.x + top.w - button_w, top.y + top.h * 0.15, button_w, top.h * 0.7);
    let tone = if limit < 100 { Tone::Go } else { Tone::Dim };
    f.button(Id::Settings(SettingsId::Tile(Setting::ChargeLimit)), b, Look::CARD, |f| {
        let battery = Icon::Battery { tenths: (limit / 10) as u8, charging: true };
        f.c.icon(battery, Rect::new(b.x + size * 0.6, b.y + (b.h - icon) / 2.0, icon, icon), tone);
        f.c.text_in(Rect::new(b.x + icon + size * 0.8, b.y, text_w + size * 0.6, b.h), size, Tone::Normal, &text, Align::Center);
    });

    let Some(level) = s.sys().status.battery else {
        f.c.text_in(top, size, Tone::Dim, "No battery", Align::Start);
        return;
    };
    let level = Fmt::<8>::of(format_args!("{level}%"));
    f.c.text_in(top, level_size, Tone::High, &level, Align::Start);
    let level_w = f.c.text_width(level_size, &level);
    let dx = top.x + level_w + size;
    let status = Fmt::<128>::of(format_args!("{}", Drain(s)));
    f.c.text_fit(Rect::new(dx, top.y, b.x - dx - size, top.h), size, Tone::Normal, &status, Keep::Start);
}

/// What the battery is doing: "Discharging · 6.2 W · 3 h 10 min left", "Charging · 12.0 W", "Charging
/// or full".
struct Drain<'a>(&'a State);

impl Display for Drain<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let sys = self.0.sys();
        match (sys.status.charging, sys.metrics.power_w) {
            (true, Some(w)) if w.abs() > 0.05 => write!(f, "Charging · {:.1} W", w.abs()),
            (true, _) => f.write_str("Charging or full"),
            (false, Some(w)) => {
                write!(f, "Discharging · {w:.1} W")?;
                match sys.metrics.minutes_left {
                    Some(m) => write!(f, " · {} left", Duration(m)),
                    None => Ok(()),
                }
            }
            (false, None) => f.write_str("Discharging"),
        }
    }
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
