//! The volume and brightness tabs.
//!
//! The volume tab shows the output volume with its mute button, the outputs (a tap chooses which one
//! plays), then each app that is playing, with its own level and mute button. The brightness tab shows
//! the brightness level, the same as the bar's, then each screen's backlight as a share of it. The two
//! are multiplied.
//!
//! A level row is set to where the finger is on its track and follows the finger as it moves. The pad's
//! left and right step it.

use clamshell_state::levels::Level;
use clamshell_state::{AudioId, Id, ListId, RowPart};

use crate::gfx::icons::Icon;
use crate::gfx::{Align, Column, Keep, Rect};
use crate::toolkit::format::{self, Fmt};
use crate::toolkit::scroll::Scrolled;
use crate::toolkit::style::{self, Look, Text, Tone};
use crate::toolkit::{Frame, Track, widgets};

/// A row's height, as a fraction of the area's height.
const ROW_H: f32 = 0.11;
/// A level row: the widths of the name and number columns as fractions of the area's width, and the
/// track's thickness, the thumb and icons as fractions of the row's height.
const NAME_W: f32 = 0.28;
/// The width of the brightness tab's name column, wider because backlight names are long.
const BACKLIGHT_NAME_W: f32 = 0.4;
const NUMBER_W: f32 = 0.09;
/// The width of a screen's share ("100 %"), as a fraction of the area's width.
const PERCENT_W: f32 = 0.13;
const TRACK: f32 = 0.06;
const THUMB: f32 = 0.16;
const ICON: f32 = 0.5;
/// An output's row height as a fraction of a level row's height, and the dot marking the default output
/// as a fraction of the output row's height.
const OUTPUT_H: f32 = 0.8;
const DOT: f32 = 0.09;
/// The line height of a group heading, as a multiple of its text size.
const HEADING_LINE: f32 = 1.8;

/// The tab's sizes, from its area's height.
struct Sizes {
    pad: f32,
    row_h: f32,
    text: f32,
    heading: f32,
    icon: f32,
}

impl Sizes {
    fn of(f: &Frame) -> Sizes {
        let row_h = f.body().h * ROW_H;
        Sizes { pad: f.pad(), row_h, text: f.text(Text::Body), heading: f.text(Text::Small), icon: row_h * ICON }
    }

    /// Where the rows go: the content area, a little down from the bar.
    fn area(&self, body: Rect) -> Rect {
        style::content(body).shifted(0.0, self.pad * 0.5)
    }
}

/// A level row's track over `r`, lit up to the level with the thumb at the level. The width runs from 0
/// to 100, inset by the thumb so that it stays within the row at either end. The track is dim when there
/// is no level or while muted.
fn level(f: &mut Frame, level: Level, r: Rect, muted: bool) {
    let value = f.s.levels().level(level);
    let thumb = r.h * THUMB;
    let along = Track { x0: r.x + thumb, w: r.w - 2.0 * thumb };
    let lit = if muted { Tone::Dim } else { Tone::High };
    f.button(level.id(), r, Look::ROW, |f| widgets::track(f.c, along, r.y + r.h / 2.0, r.h * TRACK, thumb, value, lit));
    f.track(along);
}

/// The level as a number in `r`, right-aligned and followed by `suffix`, or "-" when unknown.
fn number(f: &mut Frame, r: Rect, value: Option<u8>, suffix: &str, size: f32) {
    let text = Fmt::<8>::of(format_args!("{}", format::Level(value, suffix)));
    f.c.text_in(r, size, Tone::Normal, &text, Align::End);
}

/// A small heading over a group of rows.
fn heading(f: &mut Frame, col: &mut Column, size: f32, text: &str) {
    let r = col.take(size * HEADING_LINE);
    f.c.text(r.x, r.y, size, Tone::Dim, text);
}

/// A dim line where rows would be.
fn empty(f: &mut Frame, col: &mut Column, h: f32, size: f32, text: &str) {
    let r = col.take(h);
    f.c.text(r.x, r.y, size, Tone::Dim, text);
}

/// The tone for a mute button, lit while muted.
fn mute_tone(muted: bool) -> Tone {
    if muted { Tone::Accent } else { Tone::Normal }
}

pub(super) fn draw_volume(f: &mut Frame) {
    let body = f.body();
    let s = f.s;
    let z = Sizes::of(f);
    let area = z.area(body);
    let (name_w, number_w) = (area.w * NAME_W, area.w * NUMBER_W);
    let mut col = Column::new(area);

    // The output volume: mute, track, number.
    let mut row = col.take(z.row_h);
    let mute = row.take_left(name_w);
    let number_r = row.take_right(number_w);
    let muted = s.levels().muted();
    f.button(Id::Audio(AudioId::MuteOutput), mute, Look::ROW, |f| {
        let icon = Icon::volume(s.levels().volume().unwrap_or(0), muted);
        let x = mute.x + z.pad * 0.5;
        f.c.icon(icon, Rect::new(x, mute.y + (mute.h - z.icon) / 2.0, z.icon, z.icon), mute_tone(muted));
        let label = Rect::new(x + z.icon * 1.4, mute.y, mute.w, mute.h);
        f.c.text_in(label, z.text, mute_tone(muted), if muted { "Muted" } else { "Volume" }, Align::Start);
    });
    level(f, Level::Volume, row, muted);
    number(f, number_r, s.levels().volume(), "", z.text);
    col.skip(z.pad * 0.5);

    // Where the sound goes.
    heading(f, &mut col, z.heading, "Output");
    let outputs = &s.levels().audio().outputs;
    if outputs.is_empty() {
        empty(f, &mut col, z.row_h * 0.6, z.text, "No outputs");
    }
    // Keep room below for the apps' heading and one row.
    let apps_room = z.heading * HEADING_LINE + z.row_h + z.pad;
    for (i, o) in outputs.iter().enumerate() {
        let h = z.row_h * OUTPUT_H;
        if col.y() + h > col.end() - apps_room {
            break;
        }
        let line = col.take(h);
        // A little wider than the area, for the pressed highlight.
        let r = Rect::new(line.x - z.pad * 0.5, line.y, line.w + z.pad, line.h);
        f.button(Id::Audio(AudioId::Output(i as u16)), r, Look::ROW, |f| {
            let dot = r.h * DOT;
            if o.default {
                f.c.circle(r.x + z.pad * 0.5 + dot * 2.0, r.y + r.h / 2.0, dot, Tone::Go);
            }
            let x = r.x + z.pad * 0.5 + dot * 5.0;
            let name = Rect::new(x, r.y, r.x + r.w - x - z.pad, r.h);
            f.c.text_fit(name, z.text, Tone::high_if(o.default), &o.name, Keep::Start);
        });
    }

    // Each app playing.
    col.skip(z.pad * 0.5);
    if !col.fits(z.heading * HEADING_LINE) {
        return;
    }
    heading(f, &mut col, z.heading, "Apps");
    let streams = &s.levels().audio().streams;
    if streams.is_empty() {
        let r = col.rest();
        f.c.text(r.x, r.y, z.text, Tone::Dim, "Nothing is playing");
    }
    let rows = Scrolled::begin(f, ListId::Streams, col.rest(), z.row_h, streams.len());
    for i in rows.shown() {
        let Some(stream) = streams.get(i) else {
            continue;
        };
        let mut row = rows.row(i);
        let name = row.take_left(name_w);
        let mute = row.take_right(number_w);
        let tone = if stream.muted { Tone::Dim } else { Tone::Normal };
        f.c.text_fit(Rect::new(name.x, name.y, name_w - z.pad, name.h), z.text, tone, &stream.app, Keep::Start);
        let i = i as u16;
        level(f, Level::Stream(i), row, stream.muted);
        f.button(Id::Row { list: ListId::Streams, i, part: RowPart::Mute }, mute, Look::ROW, |f| {
            f.c.icon(Icon::volume(stream.volume, stream.muted), mute.square_center(z.icon), mute_tone(stream.muted));
        });
    }
    rows.end(f, area.right() + z.pad * 0.4);
}

pub(super) fn draw_brightness(f: &mut Frame) {
    let body = f.body();
    let s = f.s;
    let z = Sizes::of(f);
    let area = z.area(body);
    let (name_w, number_w, percent_w) = (area.w * BACKLIGHT_NAME_W, area.w * NUMBER_W, area.w * PERCENT_W);
    let mut col = Column::new(area);

    // The brightness level, the same as the bar's. Every screen follows it.
    let mut row = col.take(z.row_h);
    let name = row.take_left(name_w);
    let number_r = row.take_right(number_w);
    f.c.icon(Icon::Brightness, Rect::new(name.x, name.y + (name.h - z.icon) / 2.0, z.icon, z.icon), Tone::Normal);
    f.c.text_in(Rect::new(name.x + z.icon * 1.4, name.y, name.w, name.h), z.text, Tone::Normal, "Brightness", Align::Start);
    level(f, Level::Brightness, row, false);
    number(f, number_r, s.levels().level(Level::Brightness), "", z.text);
    col.skip(z.pad * 0.5);

    // Each screen's share of the level.
    heading(f, &mut col, z.heading, "Each screen, of the brightness above");
    if s.levels().backlights().is_empty() {
        let r = col.rest();
        f.c.text(r.x, r.y, z.text, Tone::Dim, "No backlight");
        return;
    }
    let backlights = s.levels().backlights();
    let rows = Scrolled::begin(f, ListId::Backlights, col.rest(), z.row_h, backlights.len());
    for i in rows.shown() {
        let Some(backlight) = backlights.get(i) else {
            continue;
        };
        let mut row = rows.row(i);
        let name = row.take_left(name_w);
        let percent = row.take_right(percent_w);
        f.c.text_fit(
            Rect::new(name.x, name.y, name_w - z.pad, name.h),
            z.text,
            Tone::Normal,
            clamshell_state::levels::backlight_label(s.profile(), backlight),
            Keep::Start,
        );
        let level_id = Level::Backlight(i as u16);
        level(f, level_id, row, false);
        number(f, percent, s.levels().level(level_id), " %", z.text);
    }
    rows.end(f, area.right() + z.pad * 0.4);
}
