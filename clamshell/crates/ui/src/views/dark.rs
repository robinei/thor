//! The dark second screen, while a game runs that wants it so: nothing drawn, and a tap anywhere lights
//! it again.

use clamshell_core::Id;

use super::Frame;
use super::style::Look;
use crate::draw::Rect;

pub(super) fn draw(f: &mut Frame, screen: Rect) {
    f.button(Id::Wake, screen, Look::PLAIN, |_| {});
}
