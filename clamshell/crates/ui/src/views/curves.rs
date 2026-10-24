//! The HDR curves' screens, drawn over the settings tab. Their rules are on
//! [`clamshell_state::curve::screens::Curving`].
//!
//! - The picker and a curve's menu, the profiles' picker ([`super::profiles`]).
//! - A curve's edit screen: the curve drawn as the light shown for the game's grey levels, against SDR as
//!   it is, then a row for each parameter. A finger held or slid on a row brings up its slider, and the
//!   game on the top screen follows as it moves.

use clamshell_state::curve::screens::CurveScreen;
use clamshell_state::curve::{Curve, CurveParam, REFERENCE_NITS};
use clamshell_state::profiles::picker::{Kind, ProfileId};
use clamshell_state::{Control, Id};

use crate::gfx::{Column, Keep, Rect, Rgb};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, Text, Tone};
use crate::toolkit::widgets::{HeaderRight, HeldSlider};
use crate::toolkit::{Frame, Track, widgets};

/// A parameter row's height, as a fraction of the area's height, and its text, as a fraction of the small
/// size.
const ROW_H: f32 = 0.075;
const ROW_TEXT: f32 = 0.8;
/// The light the plot reaches at its top, in nits, the same for every curve so that curves compare.
const PLOT_NITS: f64 = 1000.0;
/// Points along the plotted curve.
const PLOT_STEPS: u16 = 96;
/// The plot's axes, and SDR as it is, drawn behind the curve.
const AXIS: Rgb = Rgb(0x2a2a30);
const SDR: Rgb = Rgb(0x4a4a54);

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
    plot(f, Rect::new(area.x, area.y, area.w, (area.h - rows_h - pad).max(0.0)), &curve, size);
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

/// The curve: the game's grey levels along, the light shown for them up to [`PLOT_NITS`], with SDR as it
/// is (white at the reference) behind it. The light is on PQ's scale, which follows how bright it looks,
/// so the shadows that the black point and contrast change show as clearly as the highlights.
fn plot(f: &mut Frame, r: Rect, curve: &Curve, size: f32) {
    if r.h < size * 3.0 {
        return;
    }
    let inner = Rect::new(r.x + size * 0.5, r.y + size * 1.5, r.w - size, r.h - size * 2.0);
    f.c.line((inner.x, inner.bottom()), (inner.right(), inner.bottom()), 1.0, AXIS);
    f.c.line((inner.x, inner.y), (inner.x, inner.bottom()), 1.0, AXIS);
    f.c.text_fit(Rect::new(inner.x + size * 0.4, r.y, inner.w * 0.5, size * 1.4), size, Tone::Dim, "1000 nits", Keep::Start);
    let top = pq(PLOT_NITS);
    let at = |e: f64, nits: f64| (inner.x + e as f32 * inner.w, inner.bottom() - (pq(nits) / top).min(1.0) as f32 * inner.h);
    let line = |f: &mut Frame, c: &Curve, width: f32, colour: Rgb| {
        let mut last = at(0.0, 0.0);
        for i in 1..=PLOT_STEPS {
            let e = f64::from(i) / f64::from(PLOT_STEPS);
            let point = at(e, c.map([e, e, e])[1] * REFERENCE_NITS);
            f.c.line(last, point, width, colour);
            last = point;
        }
    };
    line(f, &Curve::default(), 1.0, SDR);
    line(f, curve, 2.0, Tone::High.rgb());
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
