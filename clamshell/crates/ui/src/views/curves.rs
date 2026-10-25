//! The tone curves' screens, drawn over the settings tab. Their rules are on
//! [`clamshell_state::curve::screens::Curving`].
//!
//! - The picker and a curve's menu, the profiles' picker ([`super::profiles`]).
//! - A curve's edit screen: the curve drawn as the light shown for the game's grey levels on its screen,
//!   against the game as it is, then a row for each parameter. A finger held or slid on a row brings up
//!   its slider, and the game on the top screen follows as it moves.

use clamshell_state::curve::screens::CurveScreen;
use clamshell_state::curve::{Curve, CurveParam, REFERENCE_NITS, map_hdr};
use clamshell_state::profiles::picker::{Kind, ProfileId};
use clamshell_state::session::presentation::{CurvePlot, HISTOGRAM_BINS, Histogram};
use clamshell_state::settings::Setting;

use clamshell_state::{Control, Id, Source, State};

use crate::gfx::{Align, Column, Keep, Rect, Rgb};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Text, Tone};
use crate::toolkit::widgets::{HeaderRight, HeldSlider};
use crate::toolkit::{Frame, Track, widgets};

/// A parameter row's height, as a fraction of the area's height, and its text, as a fraction of the small
/// size.
const ROW_H: f32 = 0.075;
const ROW_TEXT: f32 = 0.8;
/// The light the plot reaches at its top, in nits, the same for every curve so that curves compare; more
/// for a screen brighter than it.
const PLOT_NITS: f64 = 1000.0;
/// Points along the plotted curve.
const PLOT_STEPS: u16 = 96;
/// The lines at the screen's peak and paper white.
const AS_IS: Rgb = Rgb(0x4a4a54);
/// The brightest grey a histogram bar is filled with, the light at the top of the plot; paper white is half
/// of it.
const HISTOGRAM_WHITE: u8 = 230;

pub(super) fn draw(f: &mut Frame, screen: CurveScreen) {
    match screen {
        CurveScreen::Picker(p) => super::profiles::draw(f, Kind::Curve, p),
        CurveScreen::Edit(id) => edit(f, id),
    }
}

fn edit(f: &mut Frame, id: u16) {
    let s = f.s;
    let Some(curve) = s.curving().in_force(id).copied() else {
        return;
    };
    let on = s.curve_plot();
    // What the plot shows, beside the title: the game's signal, and its peak as set, declared or assumed.
    let caption = match on.hdr {
        Some(hdr) => {
            let signal = if hdr.scrgb { "scRGB" } else { "PQ" };
            if hdr.set {
                Fmt::<64>::of(format_args!("HDR ({signal}) set to {:.0} nits", hdr.nits))
            } else if hdr.declared {
                Fmt::of(format_args!("HDR ({signal}) to {:.0} nits", hdr.nits))
            } else {
                Fmt::of(format_args!("HDR ({signal}), {:.0} nits assumed", hdr.nits))
            }
        }
        None => Fmt::of(format_args!("SDR content")),
    };
    let area = widgets::header(f, Some(Id::Profile(Kind::Curve, ProfileId::Back)), s.curving().name(id), false, HeaderRight::Note(caption.as_str()));
    let size = f.text(Text::Small) * ROW_TEXT;
    let rh = f.body().h * ROW_H;
    // The curve's parameters, then, under a caption, the content's HDR peak: the program's setting.
    let caption_h = rh * 0.8;
    let rows_h = rh * (CurveParam::ALL.len() + 1) as f32 + caption_h;
    let pad = f.pad();
    plot(f, Rect::new(area.x, area.y, area.w, (area.h - rows_h - pad).max(0.0)), &curve, on, size);
    let mut col = Column::new(Rect::new(area.x, area.bottom() - rows_h, area.w, rows_h));
    // The track spans the screen's width, as the keymap parameters' sliders do.
    let body = f.body();
    let track = Track { x0: body.x, w: body.w };
    let slider = s.curving().slider();
    let mut held = None;
    for p in CurveParam::ALL {
        let Some(row) = col.next(rh) else {
            break;
        };
        let value = p.get(&curve);
        let sl = slider.filter(|sl| sl.control == Control::Curve(p));
        if let Some(sl) = sl {
            held = Some((row, sl, p));
        }
        let changed = value != p.get(&Curve::default());
        f.button(Id::Control(Control::Curve(p)), row, Look::ROW, |f| {
            if sl.is_none() {
                widgets::value_row(f, row, size, p.label(), shown(p, value).as_str(), changed);
            }
        });
        f.track(track);
    }
    // Set apart from the curve, which may be shared: the content's HDR peak, the setting of the level the
    // picker was opened from. Its slider starts at Default, which writes nothing there; the caption says
    // where the value is read from, or that it is written to the level, and follows the slider. It does
    // nothing for SDR content.
    let setting = Setting::HdrPeak;
    let source = s.panel().source(setting);
    if let Some(caption) = col.next(caption_h) {
        let note = if on.hdr.is_none() { " · no effect on SDR content" } else { "" };
        let text = match source {
            Source::Written(level) => Fmt::<96>::of(format_args!("Written to {level}{note}")),
            Source::Read(level) => Fmt::of(format_args!("Read from {level}{note}")),
            Source::Defaults => Fmt::of(format_args!("Read from defaults{note}")),
        };
        let inner = caption.inset(size * 0.4);
        f.c.text_fit(Rect::new(inner.x, caption.y + caption.h * 0.3, inner.w, caption.h * 0.7), size * 0.8, Tone::Dim, text.as_str(), Keep::Start);
    }
    let mut held_setting = None;
    if let Some(row) = col.next(rh) {
        let control = Control::Setting(setting);
        let sl = s.setting_slider().filter(|sl| sl.control == control);
        let written = matches!(source, Source::Written(_));
        let text = setting_text(s, setting, written);
        if let Some(sl) = sl {
            held_setting = Some((row, sl, text));
        }
        f.button(Id::Control(control), row, Look::ROW, |f| {
            if sl.is_none() {
                widgets::value_row(f, row, size, setting.title(), text.as_str(), written);
            }
        });
        f.track(track);
    }
    // A held slider is drawn last, so its card covers the rows below it.
    let within = Rect::new(track.x0, area.y, track.w, area.h);
    if let Some((row, sl, p)) = held {
        let text = Fmt::<48>::of(format_args!("{} {}", p.label(), shown(p, p.value(sl.value)).as_str()));
        widgets::held_slider(f, HeldSlider { slider: &sl, x0: track.x0, cy: row.y + row.h / 2.0, h: row.h * 0.75, within, icon: None, text: &text });
    }
    if let Some((row, sl, value)) = held_setting {
        let text = Fmt::<64>::of(format_args!("{} {}", setting.title(), value.as_str()));
        widgets::held_slider(f, HeldSlider { slider: &sl, x0: track.x0, cy: row.y + row.h / 2.0, h: row.h * 0.75, within, icon: None, text: &text });
    }
}

/// A setting's value at the level edited, as its row shows it: written there, its value; else Default,
/// with the value it reads.
fn setting_text(s: &State, setting: Setting, written: bool) -> Fmt<48> {
    let mut value = Fmt::<32>::new();
    setting.describe(s.panel().tile_value(setting), &mut value);
    if written { Fmt::of(format_args!("{}", value.as_str())) } else { Fmt::of(format_args!("Default · {}", value.as_str())) }
}

/// The curve: the game's levels along, the light its screen shows for them up to [`PLOT_NITS`] (or the
/// screen's peak), with the game as it is behind it, and lines with their nits at the screen's peak, where
/// the game's white is shown (paper white) and the screen's black. For SDR the levels
/// are its signal and as it is, white is at the reference; for HDR they are its PQ signal up to the
/// brightest it declares, and as it is, the light it asks for. The light is on PQ's scale, which follows
/// how bright it looks, so the shadows that the black point and contrast change show as clearly as the
/// highlights.
fn plot(f: &mut Frame, r: Rect, curve: &Curve, on: CurvePlot, size: f32) {
    if r.h < size * 3.0 {
        return;
    }
    // On whole pixels, so that the histogram's bars have sharp edges; room below for the black's label.
    let inner = Rect::new((r.x + size * 0.5).round(), (r.y + size * 1.6).round(), (r.w - size).round(), (r.h - size * 3.0).round());
    let screen_nits = on.headroom * REFERENCE_NITS;
    let top_nits = PLOT_NITS.max(screen_nits).min(10000.0);
    let top = pq(top_nits);
    let y_of = |nits: f64| inner.bottom() - (pq(nits) / top).min(1.0) as f32 * inner.h;
    // The light shown for level `e` (0 to 1) as it is, and through the curve.
    let shown = |e: f64, curved: bool| -> f64 {
        match on.hdr.map(|hdr| hdr.peak) {
            None if curved => curve.map_on([e, e, e], on.headroom)[1] * REFERENCE_NITS,
            None => Curve::default().map([e, e, e])[1] * REFERENCE_NITS,
            Some(peak) => {
                let light = pq_light(e * pq(peak * REFERENCE_NITS)) / REFERENCE_NITS;
                if curved { map_hdr(Some(curve), [light; 3], peak, on.headroom)[1] * REFERENCE_NITS } else { light * REFERENCE_NITS }
            }
        }
    };
    // The picture's histogram first, when one is measured, under the lines and their labels.
    let grey = grey_scale(inner, &y_of);
    if let Some(histogram) = on.histogram {
        histograms(f, inner, &histogram, on, &shown, &grey);
    }
    // The screen's peak, paper white and black, each labelled above its line; a label too close to the
    // one above is left out.
    let paper = (f64::from(curve.white)).min(screen_nits);
    let mut last_label = f32::NEG_INFINITY;
    for nits in [screen_nits, paper, on.black * REFERENCE_NITS] {
        let y = y_of(nits);
        // The screen's black is only labelled: its line would lie on the base strip.
        let black = nits <= on.black * REFERENCE_NITS;
        if !black {
            f.c.line((inner.x, y), (inner.right(), y), 1.0, AS_IS);
        }
        if y - last_label >= size * 1.35 {
            // The unit once, on the topmost.
            let unit = if last_label == f32::NEG_INFINITY { " nits" } else { "" };
            let label = if nits >= 10.0 {
                Fmt::<24>::of(format_args!("{nits:.0}{unit}"))
            } else if nits >= 1.0 {
                Fmt::of(format_args!("{nits:.1}{unit}"))
            } else {
                Fmt::of(format_args!("{nits:.3}{unit}"))
            };
            // Above its line at the left, where the curve is low; the black's below the plot at the right,
            // under the histogram's bars.
            if black {
                f.c.text_in(Rect::new(inner.x, inner.bottom() + size * 0.2, inner.w, size * 1.2), size, Tone::Dim, label.as_str(), Align::End);
            } else {
                f.c.text_in(Rect::new(inner.x + size * 0.4, y - size * 1.5, inner.w - size * 0.7, size * 1.2), size, Tone::Dim, label.as_str(), Align::Start);
            }
            last_label = y;
        }
    }
    let line = |f: &mut Frame, curved: bool, width: f32, colour: Rgb| {
        let mut last = (inner.x, y_of(shown(0.0, curved)));
        for i in 1..=PLOT_STEPS {
            let e = f64::from(i) / f64::from(PLOT_STEPS);
            let point = (inner.x + e as f32 * inner.w, y_of(shown(e, curved)));
            f.c.line(last, point, width, colour);
            last = point;
        }
    };
    // Coloured, to stand out from the greys of the histogram: as the content is, and through the curve.
    line(f, false, 1.5, Tone::Fn.rgb());
    line(f, true, 2.0, Tone::Accent.rgb());
}

/// The grey of light in the plot `inner`, drawn at `y_of`: black to paper white the darker half of the
/// greys, paper white to the top of the plot (the screen's peak) the lighter, so that what a curve takes
/// above paper white is brighter than any of SDR's greys. Each half follows how high the plot draws it.
fn grey_scale<'a>(inner: Rect, y_of: &'a dyn Fn(f64) -> f32) -> impl Fn(f64) -> Rgb + 'a {
    let height = move |nits: f64| ((inner.bottom() - y_of(nits)) / inner.h.max(1.0)).clamp(0.0, 1.0);
    let paper = height(REFERENCE_NITS).max(1e-3);
    move |nits: f64| {
        let h = height(nits);
        let v = if h <= paper { h / paper * 0.5 } else { 0.5 + (h - paper) / (1.0 - paper).max(1e-3) * 0.5 };
        let v = (v.clamp(0.0, 1.0) * f32::from(HISTOGRAM_WHITE)) as u32;
        Rgb((v << 16) | (v << 8) | v)
    }
}

/// The picture's histogram in the plot `inner`: where its pixels' levels are, as bars along the bottom
/// (for HDR, those beyond the content's peak in the last). A bar's left quarter is the grey (`grey`) of its
/// level's light as the content is, the rest the grey of the light the curve gives it, so that a bar
/// lighter or darker on the right than on the left is brightened or darkened. Heights are square roots of the shares,
/// so that few pixels still show.
fn histograms(f: &mut Frame, inner: Rect, histogram: &Histogram, on: CurvePlot, shown: &dyn Fn(f64, bool) -> f64, grey: &dyn Fn(f64) -> Rgb) {
    let n = HISTOGRAM_BINS;
    // A bin's middle level as the plot's: SDR's signal, or PQ's over the content's peak.
    let along = |bin: usize| {
        let level = (bin as f64 + 0.5) / n as f64;
        match on.hdr.filter(|_| histogram.hdr) {
            Some(hdr) => level / pq(hdr.peak * REFERENCE_NITS).max(1e-6),
            None => level,
        }
    };
    let mut columns = [0u32; HISTOGRAM_BINS];
    for (bin, count) in histogram.bins.iter().enumerate().filter(|(_, c)| **c > 0) {
        if let Some(c) = columns.get_mut(((along(bin) * n as f64) as usize).min(n - 1)) {
            *c += u32::from(*count);
        }
    }
    let most = columns.iter().copied().max().unwrap_or(0).max(1);
    let w = inner.w / n as f32;
    for (i, count) in columns.iter().enumerate().filter(|(_, c)| **c > 0) {
        let e = (i as f64 + 0.5) / n as f64;
        let bar = (*count as f32 / most as f32).sqrt() * inner.h * 0.4;
        // Edges on whole pixels, so that no pixel is partly filled.
        let (x, right) = ((inner.x + i as f32 * w).round(), (inner.x + (i + 1) as f32 * w).round());
        let y = (inner.bottom() - bar).round();
        let (width, height) = (right - x, inner.bottom() - y);
        let before = (width * 0.25).round().max(1.0);
        f.c.fill(Rect::new(x, y, before, height), grey(shown(e, false)));
        f.c.fill(Rect::new(x + before, y, width - before, height), grey(shown(e, true)));
    }
}

/// SMPTE ST 2084's decoding: the light in nits of a PQ signal, 0 to 1.
fn pq_light(e: f64) -> f64 {
    let p = e.clamp(0.0, 1.0).powf(32.0 / 2523.0);
    ((p - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * p)).powf(16384.0 / 2610.0) * 10000.0
}

/// SMPTE ST 2084's encoding of light in nits, 0 to 1.
fn pq(nits: f64) -> f64 {
    let p = (nits.max(0.0) / 10000.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p)).powf(2523.0 / 32.0)
}

/// A parameter's value with its unit.
fn shown(p: CurveParam, v: u16) -> Fmt<24> {
    match p {
        CurveParam::Black => Fmt::of(format_args!("{}.{} %", v / 10, v % 10)),
        CurveParam::Contrast => Fmt::of(format_args!("{}.{:02}", v / 100, v % 100)),
        CurveParam::White => Fmt::of(format_args!("{v} nits")),
        CurveParam::Highlights if v == 0 => Fmt::of(format_args!("White")),
        CurveParam::Highlights if v >= 100 => Fmt::of(format_args!("The screen's peak")),
        CurveParam::Highlights => Fmt::of(format_args!("{v} % of the room")),
        CurveParam::Start | CurveParam::Saturation => Fmt::of(format_args!("{v} %")),
    }
}
