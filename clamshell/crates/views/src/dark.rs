//! The second screen when it is dark, while a game that asks for this is running. Nothing is drawn, and
//! a tap anywhere lights it again.

use clamshell_state::Id;

use clamshell_ui::gfx::Rect;
use clamshell_ui::toolkit::Frame;
use clamshell_ui::toolkit::style::Look;

pub(super) fn draw(f: &mut Frame, screen: Rect) {
    f.button(Id::Wake, screen, Look::PLAIN, |_| {});
}
