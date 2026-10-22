//! Generic drawing, knowing nothing of ClamShell's state: geometry ([`geom`]), colours ([`color`]), the
//! canvas over a pixmap ([`canvas`]: shapes, lines, images), icons ([`icons`]), text ([`text`]),
//! fonts ([`fonts`]) and images decoded off the drawing thread ([`images`]).

pub mod canvas;
pub mod color;
pub mod fonts;
pub mod geom;
pub mod icons;
pub mod images;
pub mod text;

pub use canvas::{Canvas, DrawCache};
pub use color::Rgb;
pub use fonts::spawn_font_finder;
pub use geom::{Align, Column, Rect, Span};
pub use text::{Keep, LINE, WRAP_LINE};
