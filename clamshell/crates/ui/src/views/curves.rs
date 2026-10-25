//! The HDR curves' screens, drawn over the settings tab. Their rules are on
//! [`clamshell_state::curve::screens::Curving`].
//!
//! - The picker and a curve's menu, the profiles' picker ([`super::profiles`]).
//! - A curve's edit screen: the curve drawn as the light shown for the game's grey levels on its screen,
//!   against the game as it is, then a row for each parameter. A finger held or slid on a row brings up
//!   its slider, and the game on the top screen follows as it moves.

use clamshell_state::curve::screens::CurveScreen;
use clamshell_state::curve::{Curve, CurveParam, REFERENCE_NITS, map_hdr};
use clamshell_state::profiles::picker::{Kind, ProfileId};
use clamshell_state::session::presentation::CurvePlot;
use clamshell_state::{Control, Id};

use crate::gfx::{Align, Column, Rect, Rgb};
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
/// The plot's axes and the screen's peak, and the game as it is, drawn behind the curve.
const AXIS: Rgb = Rgb(0x2a2a30);
const AS_IS: Rgb = Rgb(0x4a4a54);

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
    let area = widgets::header(f, Some(Id::Profile(Kind::Curve, ProfileId::Back)), s.curving().name(id), false, HeaderRight::None);
    let size = f.text(Text::Small) * ROW_TEXT;
    let rh = f.body().h * ROW_H;
    let rows_h = rh * CurveParam::ALL.len() as f32;
    let pad = f.pad();
    plot(f, Rect::new(area.x, area.y, area.w, (area.h - rows_h - pad).max(0.0)), &curve, s.curve_plot(), size);
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
    // A held slider is drawn last, so its card covers the rows below it.
    if let Some((row, sl, p)) = held {
        let text = Fmt::<48>::of(format_args!("{} {}", p.label(), shown(p, p.value(sl.value)).as_str()));
        let within = Rect::new(track.x0, area.y, track.w, area.h);
        widgets::held_slider(f, HeldSlider { slider: &sl, x0: track.x0, cy: row.y + row.h / 2.0, h: row.h * 0.75, within, icon: None, text: &text });
    }
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
    let inner = Rect::new(r.x + size * 0.5, r.y + size * 2.2, r.w - size, r.h - size * 2.7);
    let screen_nits = on.headroom * REFERENCE_NITS;
    let top_nits = PLOT_NITS.max(screen_nits).min(10000.0);
    f.c.line((inner.x, inner.bottom()), (inner.right(), inner.bottom()), 1.0, AXIS);
    f.c.line((inner.x, inner.y), (inner.x, inner.bottom()), 1.0, AXIS);
    let top = pq(top_nits);
    let y_of = |nits: f64| inner.bottom() - (pq(nits) / top).min(1.0) as f32 * inner.h;
    // The screen's peak, paper white and black, each labelled above its line; a label too close to the
    // one above is left out.
    let paper = (f64::from(curve.white)).min(screen_nits);
    let mut last_label = f32::NEG_INFINITY;
    for nits in [screen_nits, paper, on.black * REFERENCE_NITS] {
        let y = y_of(nits);
        f.c.line((inner.x, y), (inner.right(), y), 1.0, AS_IS);
        if y - last_label >= size * 1.8 {
            // The unit once, on the topmost.
            let unit = if last_label == f32::NEG_INFINITY { " nits" } else { "" };
            let label = if nits >= 10.0 {
                Fmt::<24>::of(format_args!("{nits:.0}{unit}"))
            } else if nits >= 1.0 {
                Fmt::of(format_args!("{nits:.1}{unit}"))
            } else {
                Fmt::of(format_args!("{nits:.3}{unit}"))
            };
            // At the left, where the curve is low; the black's at the right, where it is high.
            let (x, align) = if nits <= on.black * REFERENCE_NITS { (inner.x, Align::End) } else { (inner.x + size * 0.4, Align::Start) };
            f.c.text_in(Rect::new(x, y - size * 1.5, inner.w - size * 0.7, size * 1.2), size, Tone::Dim, label.as_str(), align);
            last_label = y;
        }
    }
    // The game's signal, and its peak, as it declares it or as assumed.
    let caption = match on.hdr {
        Some(hdr) => {
            let (signal, nits) = (if hdr.scrgb { "scRGB" } else { "PQ" }, hdr.nits);
            if hdr.declared {
                Fmt::<64>::of(format_args!("HDR game, {signal}, to {nits:.0} nits"))
            } else {
                Fmt::of(format_args!("HDR game, {signal}, peak not given ({nits:.0} nits assumed)"))
            }
        }
        None => Fmt::of(format_args!("SDR game")),
    };
    // At the right, clear of the peak's label.
    f.c.text_in(Rect::new(inner.x, r.y, inner.w - size * 0.3, size * 1.4), size, Tone::Dim, caption.as_str(), Align::End);
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
    let line = |f: &mut Frame, curved: bool, width: f32, colour: Rgb| {
        let mut last = (inner.x, y_of(shown(0.0, curved)));
        for i in 1..=PLOT_STEPS {
            let e = f64::from(i) / f64::from(PLOT_STEPS);
            let point = (inner.x + e as f32 * inner.w, y_of(shown(e, curved)));
            f.c.line(last, point, width, colour);
            last = point;
        }
    };
    line(f, false, 1.0, AS_IS);
    line(f, true, 2.0, Tone::High.rgb());
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
        CurveParam::White | CurveParam::Peak => Fmt::of(format_args!("{v} nits")),
        CurveParam::Start | CurveParam::Saturation => Fmt::of(format_args!("{v} %")),
    }
}
