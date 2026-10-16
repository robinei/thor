//! The sysinfo tab: six tiles (CPU, GPU, FPS, power, memory, cores), each a label, one large value, a
//! detail line and a minute of history as bars. Values the machine doesn't have show a dash.

use clamshell_core::metrics::{HISTORY, Ring};

use super::format::{Fmt, Frequency, HoursMinutes, Percent, Temp};
use super::style::{self, SURFACE, Tone};
use super::Frame;
use crate::draw::{Canvas, Rect, Rgb};

/// Sizes and positions, of a tile's height: label, value, detail; the graph's share at the bottom.
const LABEL: f32 = 0.11;
const VALUE_Y: f32 = 0.14;
const VALUE: f32 = 0.24;
const DETAIL_Y: f32 = 0.45;
const DETAIL: f32 = 0.105;
const GRAPH: f32 = 0.28;
/// The cores' bars: where they start, of the tile's height; a bar's width, of its slot.
const CORES_Y: f32 = 0.2;
const CORE_W: f32 = 0.3;

/// Graph colours: muted, one per tile.
const CPU: Rgb = Rgb(0x3b5479);
const GPU: Rgb = Rgb(0x2f6846);
const FPS: Rgb = Rgb(0x76602e);
const POWER: Rgb = Rgb(0x684577);
const MEMORY: Rgb = Rgb(0x3c616d);
/// Full scale for graphs whose values have no natural maximum, at least.
const POWER_SCALE_W: f32 = 10.0;
const FPS_SCALE: f32 = 60.0;
const GIB: f32 = 1024.0 * 1024.0 * 1024.0;

type Line = Fmt<64>;

struct Tile {
    label: &'static str,
    rect: Rect,
}

impl Tile {
    fn text(&self, c: &mut Canvas, value: &str, detail: &str) {
        let r = self.rect;
        c.text(r.x, r.y, r.h * LABEL, Tone::Dim, self.label);
        c.text(r.x, r.y + r.h * VALUE_Y, r.h * VALUE, Tone::High, value);
        c.text(r.x, r.y + r.h * DETAIL_Y, r.h * DETAIL, Tone::Dim, detail);
    }

    /// The history as bars, newest at the right, scaled so that `full` reaches the top.
    fn graph(&self, c: &mut Canvas, ring: &Ring, full: f32, color: Rgb) {
        let r = self.rect;
        let g = Rect::new(r.x, r.y + r.h * (1.0 - GRAPH), r.w, r.h * GRAPH);
        c.fill(Rect::new(g.x, g.y + g.h - 1.0, g.w, 1.0), SURFACE);
        let bar = g.w / HISTORY as f32;
        let skip = HISTORY - ring.len();
        for (i, v) in ring.iter().enumerate() {
            let h = (v / full.max(f32::EPSILON)).clamp(0.0, 1.0) * g.h;
            if h > 0.5 {
                let x = g.x + (skip + i) as f32 * bar;
                c.fill(Rect::new(x, g.y + g.h - h, (bar * 0.7).max(1.0), h), color);
            }
        }
    }
}

pub(super) fn draw(f: &mut Frame) {
    let body = f.body();
    let s = f.s;
    let (m, h) = (&s.sys().metrics, &s.sys().history);
    let pad = f.pad();
    let c = &mut *f.c;
    let mut grid = style::content(body);
    grid.take_top(pad);
    let tile = |label, i| Tile { label, rect: grid.cell(2, 3, pad, i) };

    let t = tile("CPU", 0);
    t.text(c, &Line::of(format_args!("{}", Percent(m.cpu))), &Line::of(format_args!("{}{}", Frequency(m.cpu_mhz), Temp(m.cpu_temp))));
    t.graph(c, &h.cpu, 1.0, CPU);

    let t = tile("GPU", 1);
    t.text(c, &Line::of(format_args!("{}", Percent(m.gpu))), &Line::of(format_args!("{}{}", Frequency(m.gpu_mhz), Temp(m.gpu_temp))));
    t.graph(c, &h.gpu, 1.0, GPU);

    let t = tile("FPS", 2);
    let (value, detail) = match (m.fps, m.frame_ms) {
        (Some(fps), Some(ms)) => (Line::of(format_args!("{fps:.0}")), Line::of(format_args!("{ms:.1} ms a frame"))),
        (Some(fps), None) => (Line::of(format_args!("{fps:.0}")), Line::new()),
        _ => (Line::of(format_args!("–")), Line::of(format_args!("no game measured"))),
    };
    t.text(c, &value, &detail);
    t.graph(c, &h.fps, h.fps.max().max(FPS_SCALE), FPS);

    let t = tile("Power", 3);
    let (value, detail) = match (m.power_w, m.minutes_left, s.sys().status.battery) {
        (Some(w), Some(min), Some(b)) if w > 0.0 => (Line::of(format_args!("{w:.1} W")), Line::of(format_args!("{b}% · {} left", HoursMinutes(min)))),
        (Some(w), _, Some(b)) if w < 0.0 => (Line::of(format_args!("{:.1} W", -w)), Line::of(format_args!("{b}% · charging"))),
        (Some(w), _, _) => (Line::of(format_args!("{:.1} W", w.abs())), Line::of(format_args!("battery"))),
        _ => (Line::of(format_args!("–")), Line::of(format_args!("no battery"))),
    };
    t.text(c, &value, &detail);
    t.graph(c, &h.power, h.power.max().max(POWER_SCALE_W), POWER);

    let t = tile("Memory", 4);
    let (used, total, swap) = (m.mem_used as f32 / GIB, m.mem_total as f32 / GIB, m.swap_used as f32 / GIB);
    if m.mem_total > 0 {
        t.text(c, &Line::of(format_args!("{used:.1} GB")), &Line::of(format_args!("of {total:.0} GB · swap {swap:.1} GB")));
    } else {
        t.text(c, "–", "");
    }
    t.graph(c, &h.mem, 1.0, MEMORY);

    // Cores: one bar per core, its current load, over a dim track.
    let t = tile("Cores", 5);
    let r = t.rect;
    c.text(r.x, r.y, r.h * LABEL, Tone::Dim, t.label);
    let n = usize::from(m.core_count);
    if n > 0 {
        let area = Rect::new(r.x, r.y + r.h * CORES_Y, r.w, r.h * (1.0 - CORES_Y));
        let slot = area.w / n as f32;
        for (i, load) in m.cores.iter().take(n).enumerate() {
            let x = area.x + i as f32 * slot;
            let w = (slot * CORE_W).max(1.0);
            c.fill(Rect::new(x, area.y, w, area.h), SURFACE);
            let bar_h = load.clamp(0.0, 1.0) * area.h;
            if bar_h > 0.5 {
                c.fill(Rect::new(x, area.y + area.h - bar_h, w, bar_h), CPU);
            }
        }
    } else {
        c.text(r.x, r.y + r.h * VALUE_Y, r.h * VALUE, Tone::Normal, "–");
    }
}
