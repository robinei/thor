//! Desktop notifications, which ClamShell shows itself over the bottom screen: those programs send on
//! the session bus (`org.freedesktop.Notifications`, served by clamshell-system's
//! `session::notifications`), and ClamShell's own, the battery warnings.
//!
//! They are held by id, newest first. A notification waits until the bottom screen is lit, and from then
//! shows for the time its program asked for ([`Notification::lasts`]); while the screen is off its time
//! starts again, so it is never missed. A tap on it invokes its default action, if it has one, and
//! dismisses it; its close mark only dismisses it. A program may close one it sent, and one it sends
//! again with the same id replaces it. A critical one wakes the screens (see `crate::notifying`).
//!
//! Each one closed is reported to the bus with the reason the specification gives
//! ([`Effect::NotificationClosed`]); ClamShell's own are not, since no program waits on them.

use std::sync::Arc;

use crate::effect::Effect;
use crate::out::Out;

/// How long a notification shows when its program leaves it to the server, in seconds.
pub const DEFAULT_SECS: u64 = 5;
/// How many are held at most. Past that the oldest is closed, so that a program that sends without end
/// doesn't fill the memory.
const MOST: usize = 32;

/// A notification's id: the one the bus thread gave a program's, or one of ClamShell's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationId {
    Bus(u32),
    /// The battery warning. A lower one replaces the one before.
    Battery,
}

/// How urgent a notification is, as its program says (the `urgency` hint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    /// Shows until dismissed, and wakes the screens.
    Critical,
}

/// A notification as it came: plain text, the key of its default action, and how long it asked to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: NotificationId,
    /// The program's name, as it gave it.
    pub app: String,
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    /// The action a tap invokes (the one with the key `default`), if the program offers one.
    pub action: Option<Arc<str>>,
    /// How long it asked to show, in milliseconds: -1 leaves it to the server, 0 means until dismissed.
    pub timeout_ms: i32,
}

impl Notification {
    /// How long it shows once seen, in whole seconds, or none if it stays until dismissed: [`DEFAULT_SECS`]
    /// when the program leaves it to us, never for a critical one.
    pub fn lasts(&self) -> Option<u64> {
        if self.urgency == Urgency::Critical {
            return None;
        }
        match self.timeout_ms {
            0 => None,
            ms if ms < 0 => Some(DEFAULT_SECS),
            ms => Some(u64::from(ms.unsigned_abs()).div_ceil(1000)),
        }
    }
}

/// What happens to the notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationEvent {
    /// A program sent one, from the bus thread. One with an id held replaces it.
    Notified(Box<Notification>),
    /// A program closed one it sent (`CloseNotification`), by id.
    Closed(u32),
    /// A tap on one: its default action is invoked and it is dismissed. The UI sends it as the press
    /// comes down, as for the window arrows.
    Tapped(NotificationId),
    /// A tap on its close mark: it is dismissed.
    Dismissed(NotificationId),
}

/// Why a notification closed, as the specification numbers it in `NotificationClosed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired,
    Dismissed,
    /// By the program's `CloseNotification`.
    Closed,
    /// Any other reason: here, dropped as the oldest of too many.
    Undefined,
}

impl CloseReason {
    /// The number the specification gives the reason.
    pub fn code(self) -> u32 {
        match self {
            CloseReason::Expired => 1,
            CloseReason::Dismissed => 2,
            CloseReason::Closed => 3,
            CloseReason::Undefined => 4,
        }
    }
}

/// A notification held, with when it was first seen this time the bottom screen is lit.
#[derive(Debug, Clone)]
struct Held {
    notification: Notification,
    seen_at: Option<u64>,
}

/// The notifications held, newest first.
#[derive(Debug, Default)]
pub struct Notifications {
    held: Vec<Held>,
}

impl Notifications {
    /// The notifications, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &Notification> {
        self.held.iter().map(|h| &h.notification)
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    pub fn get(&self, id: NotificationId) -> Option<&Notification> {
        self.iter().find(|n| n.id == id)
    }

    /// When the notification `id` expires, in monotonic seconds, if it is being seen and expires.
    pub fn expires_at(&self, id: NotificationId) -> Option<u64> {
        let held = self.held.iter().find(|h| h.notification.id == id)?;
        Some(held.seen_at? + held.notification.lasts()?)
    }

    /// Takes `notification` as the newest, in place of one with its id. Past [`MOST`] the oldest is
    /// closed.
    pub(crate) fn notified(&mut self, notification: Notification, out: &mut Out) {
        self.held.retain(|h| h.notification.id != notification.id);
        self.held.insert(0, Held { notification, seen_at: None });
        while self.held.len() > MOST {
            if let Some(oldest) = self.held.pop() {
                closed(oldest.notification.id, CloseReason::Undefined, out);
            }
        }
    }

    /// Closes notification `id` for `reason`. Returns whether it was held.
    pub(crate) fn close(&mut self, id: NotificationId, reason: CloseReason, out: &mut Out) -> bool {
        let Some(i) = self.held.iter().position(|h| h.notification.id == id) else {
            return false;
        };
        self.held.remove(i);
        closed(id, reason, out);
        true
    }

    /// A tap on notification `id`: its default action is invoked, then it is dismissed. Clients take the
    /// action before the close (a client that ends on the close would miss it).
    pub(crate) fn tapped(&mut self, id: NotificationId, out: &mut Out) {
        let action = self.get(id).and_then(|n| n.action.clone());
        if let (NotificationId::Bus(bus), Some(key)) = (id, action) {
            out.effects.push(Effect::ActionInvoked { id: bus, key });
        }
        self.close(id, CloseReason::Dismissed, out);
    }

    /// Runs after any event. While the bottom screen is lit (`seen`), each notification's time runs from
    /// when it was first seen, and one whose time is up is closed as expired. While it isn't, their
    /// times start again.
    pub(crate) fn follow(&mut self, now: u64, seen: bool, out: &mut Out) {
        for held in &mut self.held {
            held.seen_at = if seen { Some(held.seen_at.unwrap_or(now)) } else { None };
        }
        while let Some(i) = self.held.iter().position(|h| h.seen_at.zip(h.notification.lasts()).is_some_and(|(at, lasts)| now >= at + lasts)) {
            let gone = self.held.remove(i);
            closed(gone.notification.id, CloseReason::Expired, out);
        }
    }
}

/// Tells the bus that notification `id` closed, if it is a program's.
fn closed(id: NotificationId, reason: CloseReason, out: &mut Out) {
    if let NotificationId::Bus(id) = id {
        out.effects.push(Effect::NotificationClosed { id, reason });
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests;
