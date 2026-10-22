//! Generic drawing that knows nothing of ClamShell's state. It has geometry ([`geom`]), colours
//! ([`color`]), the canvas that records shapes, lines and images ([`canvas`]) into a recording
//! ([`record`]), the painter that plays a recording into a pixmap ([`paint`]), icons ([`icons`]), text
//! ([`text`]), fonts ([`fonts`]) and images decoded off the drawing thread ([`images`]).

pub mod canvas;
pub mod color;
pub mod fonts;
pub mod geom;
pub mod icons;
pub mod images;
pub mod paint;
pub mod record;
pub mod text;

pub use canvas::{Canvas, DrawCache};
pub use color::Rgb;
pub use fonts::spawn_font_finder;
pub use geom::{Align, Column, Rect, Span};
pub use text::{Keep, LINE, WRAP_LINE};
