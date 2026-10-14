//! The sysinfo mode: six tiles (CPU, GPU, FPS, power, memory, cores), each a label, one large value, a
//! detail line and a minute of history as bars. Values the machine doesn't have show a dash. Text is
//! formatted into two reused buffers, so drawing doesn't allocate.

use std::fmt::{Arguments, Write};

use sysprog_core::metrics::{HISTORY, Ring};

use super::{DIM, Frame, SURFACE, TEXT, TEXT_HI};
use crate::draw::{Canvas, Rect, rgb};

/// Padding, of the area's height.
const PAD: f32 = 0.03;
/// Sizes and positions, of a tile's height: label, value, detail; the graph's share at the bottom.
const LABEL: f32 = 0.11;
const VALUE_Y: f32 = 0.14;
const VALUE: f32 = 0.24;
const DETAIL_Y: f32 = 0.45;
const DETAIL: f32 = 0.105;
const GRAPH: f32 = 0.28;

/// Graph colours: muted, one per tile.
const CPU: u32 = 0x3b5479;
const GPU: u32 = 0x2f6846;
const FPS: u32 = 0x76602e;
const POWER: u32 = 0x684577;
const MEMORY: u32 = 0x3c616d;
/// Full scale for graphs whose values have no natural maximum, at least.
const POWER_SCALE_W: f32 = 10.0;
const FPS_SCALE: f32 = 60.0;
const GIB: f32 = 1024.0 * 1024.0 * 1024.0;

/// Reused text buffers for one tile's value and detail line.
#[derive(Debug, Default)]
pub struct Lines {
    value: String,
    detail: String,
}

impl Lines {
    fn set(&mut self, value: Arguments, detail: Arguments) {
        self.value.clear();
        let _ = self.value.write_fmt(value);
        self.detail.clear();
        let _ = self.detail.write_fmt(detail);
    }
}

struct Tile {
    label: &'static str,
    rect: Rect,
}

impl Tile {
    fn text(&self, c: &mut Canvas, lines: &Lines) {
        let r = self.rect;
        c.text(r.x, r.y, r.h * LABEL, rgb(DIM), self.label);
        c.text(r.x, r.y + r.h * VALUE_Y, r.h * VALUE, rgb(TEXT_HI), &lines.value);
        c.text(r.x, r.y + r.h * DETAIL_Y, r.h * DETAIL, rgb(DIM), &lines.detail);
    }

    fn graph_area(&self) -> Rect {
        let r = self.rect;
        Rect::new(r.x, r.y + r.h * (1.0 - GRAPH), r.w, r.h * GRAPH)
    }

    /// The history as bars, newest at the right, scaled so that `full` reaches the top.
    fn graph(&self, c: &mut Canvas, ring: &Ring, full: f32, color: u32) {
        let g = self.graph_area();
        c.fill(Rect::new(g.x, g.y + g.h - 1.0, g.w, 1.0), rgb(SURFACE));
        let bar = g.w / HISTORY as f32;
        let skip = HISTORY - ring.len();
        for (i, v) in ring.iter().enumerate() {
            let h = (v / full.max(f32::EPSILON)).clamp(0.0, 1.0) * g.h;
            if h > 0.5 {
                let x = g.x + (skip + i) as f32 * bar;
                c.fill(Rect::new(x, g.y + g.h - h, (bar * 0.7).max(1.0), h), rgb(color));
            }
        }
    }
}

/// A temperature for a detail line: " · 46°C", or nothing.
struct Temp(Option<f32>);

impl std::fmt::Display for Temp {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Some(t) => write!(f, " · {t:.0}°C"),
            None => Ok(()),
        }
    }
}

/// A percentage, or a dash.
struct Percent(Option<f32>);

impl std::fmt::Display for Percent {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{:.0}%", v * 100.0),
            None => write!(f, "–"),
        }
    }
}

/// A clock: "3.7 GHz", "680 MHz", or nothing.
struct Clock(Option<u32>);

impl std::fmt::Display for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Some(mhz) if mhz >= 1000 => write!(f, "{:.1} GHz", mhz as f32 / 1000.0),
            Some(mhz) => write!(f, "{mhz} MHz"),
            None => write!(f, "clock unknown"),
        }
    }
}

pub(super) fn draw(f: &mut Frame, body: Rect, lines: &mut Lines) {
    let s = f.s;
    let (m, h) = (&s.metrics, &s.history);
    let c = &mut *f.c;
    let pad = body.h * PAD;
    let grid = body.inset(pad);
    let tile = |label, i| Tile { label, rect: grid.cell(2, 3, pad, i) };

    let t = tile("CPU", 0);
    lines.set(format_args!("{}", Percent(m.cpu)), format_args!("{}{}", Clock(m.cpu_mhz), Temp(m.cpu_temp)));
    t.text(c, lines);
    t.graph(c, &h.cpu, 1.0, CPU);

    let t = tile("GPU", 1);
    lines.set(format_args!("{}", Percent(m.gpu)), format_args!("{}{}", Clock(m.gpu_mhz), Temp(m.gpu_temp)));
    t.text(c, lines);
    t.graph(c, &h.gpu, 1.0, GPU);

    let t = tile("FPS", 2);
    match (m.fps, m.frame_ms) {
        (Some(fps), Some(ms)) => lines.set(format_args!("{fps:.0}"), format_args!("{ms:.1} ms a frame")),
        (Some(fps), None) => lines.set(format_args!("{fps:.0}"), format_args!("")),
        _ => lines.set(format_args!("–"), format_args!("no game measured")),
    }
    t.text(c, lines);
    t.graph(c, &h.fps, h.fps.max().max(FPS_SCALE), FPS);

    let t = tile("Power", 3);
    let battery = s.status.battery;
    match (m.power_w, m.minutes_left, battery) {
        (Some(w), Some(min), Some(b)) if w > 0.0 => {
            lines.set(format_args!("{w:.1} W"), format_args!("{b}% · {}:{:02} left", min / 60, min % 60));
        }
        (Some(w), _, Some(b)) if w < 0.0 => lines.set(format_args!("{:.1} W", -w), format_args!("{b}% · charging")),
        (Some(w), _, _) => lines.set(format_args!("{:.1} W", w.abs()), format_args!("battery")),
        _ => lines.set(format_args!("–"), format_args!("no battery")),
    }
    t.text(c, lines);
    t.graph(c, &h.power, h.power.max().max(POWER_SCALE_W), POWER);

    let t = tile("Memory", 4);
    let (used, total, swap) = (m.mem_used as f32 / GIB, m.mem_total as f32 / GIB, m.swap_used as f32 / GIB);
    if m.mem_total > 0 {
        lines.set(format_args!("{used:.1} GB"), format_args!("of {total:.0} GB · swap {swap:.1} GB"));
    } else {
        lines.set(format_args!("–"), format_args!(""));
    }
    t.text(c, lines);
    t.graph(c, &h.mem, 1.0, MEMORY);

    // Cores: one bar per core, its current load, over a dim track.
    let t = tile("Cores", 5);
    c.text(t.rect.x, t.rect.y, t.rect.h * LABEL, rgb(DIM), t.label);
    let n = usize::from(m.core_count);
    if n > 0 {
        let area = Rect::new(t.rect.x, t.rect.y + t.rect.h * 0.2, t.rect.w, t.rect.h * 0.8);
        let slot = area.w / n as f32;
        for (i, load) in m.cores.iter().take(n).enumerate() {
            let x = area.x + i as f32 * slot;
            let w = (slot * 0.3).max(1.0);
            c.fill(Rect::new(x, area.y, w, area.h), rgb(SURFACE));
            let hgt = load.clamp(0.0, 1.0) * area.h;
            if hgt > 0.5 {
                c.fill(Rect::new(x, area.y + area.h - hgt, w, hgt), rgb(CPU));
            }
        }
    } else {
        c.text(t.rect.x, t.rect.y + t.rect.h * VALUE_Y, t.rect.h * VALUE, rgb(TEXT), "–");
    }
}
