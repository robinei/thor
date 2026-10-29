//! An entry's pictures, as the Library and the dock draw them: its box art or screenshot, a list row's
//! thumbnail, and its icon, greyed for an entry shown as stopped.

use clamshell_state::Entry;
use clamshell_ui::gfx::icons::Icon;
use clamshell_ui::gfx::images::{Images, Slot};
use clamshell_ui::gfx::{Align, Canvas, Rect, Rgb};
use clamshell_ui::toolkit::style::Tone;

/// The brightness of an image drawn grey, as for an entry on the dock that isn't running.
const GREY: f32 = 0.55;

/// A colour greyed the way an image is: its luminance at [`GREY`]'s brightness.
fn greyed(color: u32) -> u32 {
    let channel = |shift: u32| ((color >> shift) & 0xff) as f32;
    let luma = (0.3 * channel(16) + 0.59 * channel(8) + 0.11 * channel(0)) * GREY;
    let v = luma.round().clamp(0.0, 255.0) as u32;
    (v << 16) | (v << 8) | v
}

/// An entry's picture. This is its box art, or its screenshot (such as a video's frame) if it has
/// neither box art nor an icon.
pub(super) fn picture(entry: &Entry) -> Option<(&std::path::Path, Slot)> {
    match (&entry.box_front, &entry.icon, &entry.details.shot) {
        (Some(art), _, _) => Some((art, Slot::Art)),
        (None, None, Some(shot)) => Some((shot, Slot::Shot)),
        _ => None,
    }
}

/// Whether an entry's picture is its screenshot, such as a video's frame, which is landscape.
pub(super) fn wide_picture(entry: &Entry) -> bool {
    matches!(picture(entry), Some((_, Slot::Shot)))
}

/// Draws an entry's box art as large as fits in `r`, portrait or landscape, at its top left. An entry
/// with neither box art nor an icon gets its screenshot, such as a video's frame, instead. If there is
/// no picture, or it hasn't loaded, the placeholder or an app's icon is drawn in a portrait box as high
/// as `r`. Returns the rectangle drawn.
pub(super) fn cover(c: &mut Canvas, images: &mut Images, entry: &Entry, r: Rect, radius: f32, portrait: f32) -> Rect {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    if let Some(img) = picture(entry).and_then(|(path, slot)| images.get(&entry.id, slot, path, pw, ph)) {
        let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
        c.image(img, r.x, r.y);
        return Rect::new(r.x, r.y, iw, ih);
    }
    let box_rect = Rect::new(r.x, r.y, (r.h * portrait).min(r.w), r.h);
    art(c, images, entry, box_rect, radius, Align::Start, false);
    box_rect
}

/// An entry's screenshot fitted into `r`, at its bottom left. Nothing is drawn if there is no screenshot
/// or it hasn't loaded yet.
pub(super) fn shot(c: &mut Canvas, images: &mut Images, entry: &Entry, r: Rect) {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    if let Some(img) = entry.details.shot.as_deref().and_then(|path| images.get(&entry.id, Slot::Shot, path, pw, ph)) {
        c.image(img, r.x, r.y + r.h - img.height() as f32 / scale);
    }
}

/// An entry's picture in `r`, or its icon if it has none (see [`icon`]). The picture is fitted in and
/// centred across `r`, and placed vertically as `align` says, `Start` being the top. `grey` is for an
/// entry shown as stopped.
pub(super) fn art(c: &mut Canvas, images: &mut Images, entry: &Entry, r: Rect, radius: f32, align: Align, grey: bool) {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    match picture(entry).and_then(|(path, slot)| images.get(&entry.id, slot, path, pw, ph)) {
        Some(img) => {
            let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
            let y = match align {
                Align::Start => r.y,
                Align::Center => r.y + (r.h - ih) / 2.0,
                Align::End => r.y + r.h - ih,
            };
            if grey {
                c.image_grey(img, r.x + (r.w - iw) / 2.0, y, GREY);
            } else {
                c.image(img, r.x + (r.w - iw) / 2.0, y);
            }
        }
        None => icon(c, images, entry, r, radius, grey),
    }
}

/// A list row's picture in the square `r`. A folder with an icon or glyph shows that, since its picture
/// is for the details. Otherwise the entry's picture (box art or a video's frame) is cropped to fill
/// `r`, keeping its middle, and without a picture its icon is drawn.
pub(super) fn thumb(c: &mut Canvas, images: &mut Images, entry: &Entry, r: Rect, radius: f32, grey: bool) {
    if entry.is_group() && (entry.icon.is_some() || entry.glyph.is_some()) {
        return icon(c, images, entry, r, radius, grey);
    }
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    match picture(entry).and_then(|(path, _)| images.get(&entry.id, Slot::Thumb, path, pw, ph)) {
        Some(img) if grey => c.image_grey(img, r.x, r.y, GREY),
        Some(img) => c.image(img, r.x, r.y),
        None => icon(c, images, entry, r, radius, grey),
    }
}

/// An entry's icon, square in the middle of `r`. In order of preference it is the entry's own icon image
/// (an app's or a folder's), one of the UI's glyphs by name (for a browsed folder or file), the UI's app
/// glyph for an app, and otherwise, for a game, a placeholder in the game's colour.
pub(super) fn icon(c: &mut Canvas, images: &mut Images, entry: &Entry, r: Rect, radius: f32, grey: bool) {
    let scale = c.scale();
    let side = (r.w.min(r.h) * 0.8 * scale).round() as u32;
    if let Some(img) = entry.icon.as_deref().and_then(|path| images.get(&entry.id, Slot::Icon, path, side, side)) {
        let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
        let (x, y) = (r.x + (r.w - iw) / 2.0, r.y + (r.h - ih) / 2.0);
        if grey {
            c.image_grey(img, x, y, GREY);
        } else {
            c.image(img, x, y);
        }
        return;
    }
    match entry.glyph.as_deref().and_then(super::icons::named).or(entry.app.then_some(Icon::App)) {
        Some(glyph) => {
            let side = r.w.min(r.h) * 0.7;
            c.icon(glyph, Rect::new(r.x + (r.w - side) / 2.0, r.y + (r.h - side) / 2.0, side, side), if grey { Tone::Dim } else { Tone::Normal });
        }
        None => c.rounded(r, radius, Rgb(if grey { greyed(entry.color) } else { entry.color })),
    }
}
