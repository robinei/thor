//! The dark second screen, while a game runs that wants it so: nothing drawn, and a tap anywhere lights
//! it again.

use clamshell_core::Id;

use crate::gfx::Rect;
use crate::toolkit::Frame;
use crate::toolkit::style::Look;

pub(super) fn draw(f: &mut Frame, screen: Rect) {
    f.button(Id::Wake, screen, Look::PLAIN, |_| {});
}
