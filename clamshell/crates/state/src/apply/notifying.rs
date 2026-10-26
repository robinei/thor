//! Notifications and the other areas: a critical one wakes the screens, their times run only while the
//! bottom screen is lit, and a tap on one is a use of that screen (see [`crate::notifications`]).

use crate::State;
use crate::idle::Stage;
use crate::notifications::{CloseReason, Notification, NotificationEvent, NotificationId, Urgency};
use crate::out::Out;

impl State {
    pub(crate) fn on_notification(&mut self, event: NotificationEvent, out: &mut Out) {
        let now = self.clock.mono();
        match event {
            NotificationEvent::Notified(notification) => self.notify(*notification, out),
            NotificationEvent::Closed(id) => {
                self.notifications.close(NotificationId::Bus(id), CloseReason::Closed, out);
            }
            // A tap on dark screens only wakes them, as anywhere else.
            NotificationEvent::Tapped(_) | NotificationEvent::Dismissed(_) if self.idle.swallows_press(now) => self.idle.own_input(now),
            NotificationEvent::Tapped(id) => {
                self.touched_notification(now);
                self.notifications.tapped(id, out);
            }
            NotificationEvent::Dismissed(id) => {
                self.touched_notification(now);
                self.notifications.close(id, CloseReason::Dismissed, out);
            }
        }
    }

    /// Takes `notification`. A critical one wakes the screens, as input does, and turns the bottom screen
    /// on if it was off by hand or by itself, or dark for a game. An ordinary one waits until the screen
    /// is lit.
    pub(crate) fn notify(&mut self, notification: Notification, out: &mut Out) {
        if notification.urgency == Urgency::Critical {
            let now = self.clock.mono();
            self.idle.own_input(now);
            self.bottom.light(now);
        }
        self.notifications.notified(notification, out);
    }

    /// A notification over the bottom screen was tapped: input, and a use of that screen.
    fn touched_notification(&mut self, now: u64) {
        self.idle.own_input(now);
        self.bottom.used(&self.session, &self.clock);
    }

    /// Whether the bottom screen is lit, so that what is over it is seen: it is on, and so are the
    /// screens. Notifications are drawn, and their times run, only then.
    pub fn bottom_lit(&self) -> bool {
        !self.bottom.off() && !self.setup.calibrating() && self.idle.stage() < Stage::Off
    }
}
