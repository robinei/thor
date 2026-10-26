//! The notifications over the second screen, below the bar: the newest few, each with its summary, the
//! first line of its body and a close mark, and below them how many more there are. A tap on one invokes
//! its default action and dismisses it, and the close mark only dismisses it; both act as they are
//! pressed (`Core::press_overlay`). They sit on a fill barely above black, which sets them apart from
//! ClamShell's view or the window below them without a border, and are drawn only while the screen is
//! lit, since their time runs only then (see clamshell-state's
//! `notifications`).

use clamshell_state::notifications::Urgency;
use clamshell_state::{Id, Notification};

use crate::gfx::icons::Icon;
use crate::gfx::{Keep, LINE, Rect};
use crate::toolkit::Frame;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{Look, SURFACE, Text, Tone};

use super::bar;

/// How many are shown; the rest are counted.
const SHOWN: usize = 3;
/// The close mark's side, as a fraction of the summary's text size.
const CLOSE: f32 = 1.1;

pub(super) fn draw(f: &mut Frame, screen: Rect) {
    let notifications = f.s.notifications();
    if notifications.is_empty() || !f.s.bottom_lit() {
        return;
    }
    let mut body = screen;
    body.take_top(bar::height(screen));
    f.body = body;
    let (pad, title, small) = (f.pad(), f.text(Text::Body), f.text(Text::Small));
    let row_h = pad + title * LINE + small * LINE + pad;
    let more = notifications.len().saturating_sub(SHOWN);
    let shown = notifications.len().min(SHOWN);
    let panel_h = row_h * shown as f32 + if more > 0 { small * LINE + pad } else { 0.0 };
    f.c.fill(Rect::new(body.x, body.y, body.w, panel_h), SURFACE);
    for (i, n) in notifications.iter().take(SHOWN).enumerate() {
        let row = Rect::new(body.x, body.y + row_h * i as f32, body.w, row_h);
        draw_one(f, n, row, pad, title, small);
    }
    if more > 0 {
        let line = Rect::new(body.x + pad, body.y + row_h * shown as f32, body.w - 2.0 * pad, small * LINE);
        let text: Fmt<24> = Fmt::of(format_args!("{more} more"));
        f.c.text_fit(line, small, Tone::Dim, &text, Keep::Start);
    }
}

/// One notification in `row`: a tap on the row is a tap on it, and its close mark sits at the right end.
fn draw_one(f: &mut Frame, n: &Notification, row: Rect, pad: f32, title: f32, small: f32) {
    let close = Rect::new(row.x + row.w - row.h, row.y, row.h, row.h);
    let text_w = row.w - row.h - pad;
    f.button(Id::Notification { id: n.id, close: false }, Rect::new(row.x, row.y, row.w - row.h, row.h), Look::PLAIN, |f| {
        let summary = Rect::new(row.x + pad, row.y + pad, text_w, title * LINE);
        let tone = if n.urgency == Urgency::Critical { Tone::Accent } else { Tone::High };
        f.c.text_fit(summary, title, tone, &n.summary, Keep::Start);
        let line = Rect::new(summary.x, summary.y + summary.h, text_w, small * LINE);
        f.c.text_fit(line, small, Tone::Normal, n.body.lines().next().unwrap_or(""), Keep::Start);
    });
    f.button(Id::Notification { id: n.id, close: true }, close, Look::PLAIN, |f| {
        f.c.icon(Icon::Close, close.square_center(title * CLOSE), Tone::Dim);
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use clamshell_state::HardwareKey;
    use clamshell_state::{Event, Id, Init, Notification, NotificationEvent, NotificationId, State, Urgency};

    use crate::gfx::DrawCache;
    use crate::gfx::images::Images;
    use crate::render::{Placement, Retained, record};
    use crate::{Layer, Screen};

    fn notified(s: &mut State, id: u32) {
        let n = Notification {
            id: NotificationId::Bus(id),
            app: "mail".into(),
            summary: format!("note {id}"),
            body: "first line\nsecond line".into(),
            urgency: Urgency::Normal,
            action: None,
            timeout_ms: 0,
        };
        s.apply(NotificationEvent::Notified(Box::new(n)).into(), &mut Vec::new());
    }

    fn overlay(s: &State) -> Retained {
        let mut retained = Retained::default();
        let at = Placement { size_px: (1240, 1080), scale: 1, drift: (0, 0) };
        record(&mut retained, at, Layer::Overlay(Screen::Bottom), s, &mut DrawCache::load(), &mut Images::synchronous());
        retained
    }

    #[test]
    fn the_newest_three_show_below_the_bar_each_with_its_close_mark() {
        let mut s = State::new(Init::default());
        s.apply(Event::tick(1000), &mut Vec::new());
        assert!(overlay(&s).next.ink().is_empty(), "nothing to show, nothing drawn");
        for id in 1..=5 {
            notified(&mut s, id);
        }
        let shown = overlay(&s);
        let ids: Vec<_> = shown.targets.iter().map(|t| t.id.clone()).collect();
        let note = |id, close| Id::Notification { id: NotificationId::Bus(id), close };
        assert_eq!(ids, [note(5, false), note(5, true), note(4, false), note(4, true), note(3, false), note(3, true)], "newest first");
        let (row, close) = (shown.targets[0].rect, shown.targets[1].rect);
        assert!(row.y >= 80.0, "below the bar: {row:?}");
        assert!(close.x >= row.x + row.w - 1.0 && (close.y - row.y).abs() < 1.0, "the close mark at the row's end: {close:?}");
        assert!(shown.targets[2].rect.y >= row.y + row.h - 1.0, "one under the other");
        // The bottom screen off by hand: nothing drawn, nothing to tap.
        s.apply(Event::HardwareKey(HardwareKey::BottomPower), &mut Vec::new());
        let off = overlay(&s);
        assert!(off.targets.is_empty() && off.next.ink().is_empty());
    }
}
