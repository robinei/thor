//! What the outside is kept up to date with. After an event and the steps that follow it,
//! [`State::wanted`] says what the outside should have as things are now ([`Wanted`]), and [`Sent`]
//! compares that with what it was last sent and sends what differs, in a fixed order. An event that
//! leaves the outside in an unknown state, such as the UI starting, has some of it sent again
//! ([`Resend`]).

use crate::State;
use crate::effect::Effect;
use crate::out::Out;
use crate::shelf::{Shelf, ShelfView};

/// What the outside should have as things are now. It is built after every event, so it holds
/// references and small values only.
pub(crate) struct Wanted<'a> {
    shelf: &'a Shelf,
    /// What shelf shows while metadata is being fetched (see [`Shelf::fetch_first`]).
    shown: Option<ShelfView>,
}

/// What the outside was last sent.
#[derive(Debug, Default)]
pub(crate) struct Sent {
    /// What shelf showed when the entries it shows were last asked for first.
    shown: Option<ShelfView>,
}

/// Why the outside is to be sent something again, whatever it was last sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resend {
    /// The library was read again or fetching started or stopped: shelf's entries shown are asked for
    /// first again.
    Shown,
}

impl Sent {
    /// Forgets what was sent for `why`, so that it is sent again.
    pub(crate) fn resend(&mut self, why: Resend) {
        match why {
            Resend::Shown => self.shown = None,
        }
    }

    /// While metadata is being fetched and what shelf shows has changed, asks for the shown entries'
    /// metadata first.
    pub(crate) fn fetch_first(&mut self, w: &Wanted, out: &mut Out) {
        let Some(view) = w.shown else {
            return;
        };
        if self.shown == Some(view) {
            return;
        }
        self.shown = Some(view);
        let mut ids = Vec::new();
        w.shelf.in_view(&mut ids);
        out.effects.push(Effect::FetchFirst(ids));
    }
}

impl State {
    /// What the outside should have as things are now.
    pub(crate) fn wanted(&self) -> Wanted<'_> {
        Wanted { shelf: &self.shelf, shown: self.shelf.fetch_first() }
    }

    /// Sends the outside what differs from what it was last sent.
    pub(crate) fn sync(&mut self, out: &mut Out) {
        let mut sent = std::mem::take(&mut self.sent);
        sent.fetch_first(&self.wanted(), out);
        self.sent = sent;
    }
}
