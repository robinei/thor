//! What a frame draws, as a list of commands. The canvas records them ([`super::Canvas`]) and a player
//! turns them into pixels ([`super::paint::play`]).
//!
//! Commands hold what the canvas's methods were given, in logical pixels, so the player computes the same
//! pixels from them as drawing directly would. Floats are kept as their bits: layout is deterministic, so
//! the same bits mean the same drawing.

use super::Rect;
use super::icons::Icon;
use super::images::Image;

/// A float compared and hashed by its bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Px(u32);

impl Px {
    pub fn new(v: f32) -> Px {
        Px(v.to_bits())
    }

    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }

    pub(super) fn rect(r: Rect) -> [Px; 4] {
        [Px::new(r.x), Px::new(r.y), Px::new(r.w), Px::new(r.h)]
    }

    pub(super) fn to_rect([x, y, w, h]: [Px; 4]) -> Rect {
        Rect::new(x.get(), y.get(), w.get(), h.get())
    }
}

/// A colour as 8-bit RGBA, not premultiplied. Every colour the UI uses is made from 8-bit channels, so
/// it turns back into the same `tiny_skia::Color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba8([u8; 4]);

impl From<tiny_skia::Color> for Rgba8 {
    fn from(c: tiny_skia::Color) -> Self {
        let c = c.to_color_u8();
        Rgba8([c.red(), c.green(), c.blue(), c.alpha()])
    }
}

impl From<Rgba8> for tiny_skia::Color {
    fn from(Rgba8([r, g, b, a]): Rgba8) -> Self {
        tiny_skia::Color::from_rgba8(r, g, b, a)
    }
}

/// A rectangle of whole buffer pixels, from (x0, y0) up to but not including (x1, y1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds { x0: 0, y0: 0, x1: 0, y1: 0 };

    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds {
        Bounds { x0, y0, x1, y1 }
    }

    /// The whole of a `w` x `h` buffer.
    pub fn of_size((w, h): (u32, u32)) -> Bounds {
        Bounds::new(0, 0, i32::try_from(w).unwrap_or(i32::MAX), i32::try_from(h).unwrap_or(i32::MAX))
    }

    /// The pixels `(x0, y0, x1, y1)` touches, and one more on each side for antialiasing.
    pub(super) fn around((x0, y0, x1, y1): (f32, f32, f32, f32)) -> Bounds {
        Bounds::new(x0.floor() as i32 - 1, y0.floor() as i32 - 1, x1.ceil() as i32 + 1, y1.ceil() as i32 + 1)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn intersect(&self, o: Bounds) -> Bounds {
        Bounds::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, o: Bounds) -> Bounds {
        match (self.is_empty(), o.is_empty()) {
            (true, _) => o,
            (_, true) => *self,
            _ => Bounds::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1)),
        }
    }

    pub(super) fn tuple(&self) -> (i32, i32, i32, i32) {
        (self.x0, self.y0, self.x1, self.y1)
    }
}

/// One drawing command, with what the canvas method it records was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// The whole buffer in one colour. Every recording starts with it.
    Clear(Rgba8),
    Fill {
        r: [Px; 4],
        color: Rgba8,
    },
    HLine {
        x0: Px,
        x1: Px,
        y: Px,
        px: u32,
        color: Rgba8,
    },
    VLine {
        x: Px,
        y0: Px,
        y1: Px,
        px: u32,
        color: Rgba8,
    },
    Rounded {
        r: [Px; 4],
        radius: Px,
        color: Rgba8,
    },
    Outline {
        r: [Px; 4],
        radius: Px,
        width: Px,
        color: Rgba8,
    },
    Circle {
        cx: Px,
        cy: Px,
        r: Px,
        color: Rgba8,
    },
    Line {
        a: [Px; 2],
        b: [Px; 2],
        width: Px,
        color: Rgba8,
    },
    /// An image, in grey at `grey`/256 of its brightness if that is set.
    Image {
        image: Image,
        x: Px,
        y: Px,
        grey: Option<u32>,
    },
    Icon {
        icon: Icon,
        r: [Px; 4],
        color: Rgba8,
    },
    /// A line of text, bytes `start..start + len` of the recording's text. `fonts` is the glyph cache's
    /// generation, which changes when a font arrives for characters that were waiting for one.
    Text {
        x: Px,
        y: Px,
        size: Px,
        color: Rgba8,
        start: u32,
        len: u32,
        fonts: u32,
    },
}

/// A command and the buffer pixels it may touch: its shape's, a pixel larger for antialiasing, within
/// the clip in force when it was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cmd {
    pub op: Op,
    pub bounds: Bounds,
}

/// A frame's commands, the bytes of its text, and where it goes: the buffer's size, the scale and the
/// burn-in offset. The vectors are reused from frame to frame, so recording allocates nothing once warm.
#[derive(Debug, Default)]
pub struct Recording {
    pub(super) size: (u32, u32),
    pub(super) scale: Px,
    pub(super) offset: (i32, i32),
    pub(super) cmds: Vec<Cmd>,
    pub(super) text: Vec<u8>,
}

impl Recording {
    /// Empties it for a frame of `size` buffer pixels at `scale`, moved by `offset`.
    pub(super) fn start(&mut self, size: (u32, u32), scale: f32, offset: (i32, i32)) {
        self.size = size;
        self.scale = Px::new(scale);
        self.offset = offset;
        self.cmds.clear();
        self.text.clear();
    }

    /// The bytes of a text command's text; none for any other command.
    pub(super) fn text_of(&self, op: &Op) -> &[u8] {
        match *op {
            Op::Text { start, len, .. } => self.text.get(start as usize..start as usize + len as usize).unwrap_or(&[]),
            _ => &[],
        }
    }
}
