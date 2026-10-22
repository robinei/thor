//! What the outside is kept up to date with. After an event and the steps that follow it,
//! [`State::wanted`] says what the outside should have as things are now ([`Wanted`]), and [`Sent`]
//! compares that with what it was last sent and sends what differs, in a fixed order. An event that
//! leaves the outside in an unknown state, such as the UI starting, has some of it sent again
//! ([`Resend`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::effect::Effect;
use crate::out::Out;
use crate::shelf::files::{Browse, BrowseCommand};
use crate::shelf::{Shelf, ShelfView};
use crate::{Running, State};

/// What the outside should have as things are now. It is built after every event, so it holds
/// references and small values only.
pub(crate) struct Wanted<'a> {
    shelf: &'a Shelf,
    /// What shelf shows while metadata is being fetched (see [`Shelf::fetch_first`]).
    shown: Option<ShelfView>,
    /// The open browsed folder and its directory (see [`Shelf::browsed`]), and whether hidden files are
    /// listed.
    browsed: Option<(&'a Arc<str>, &'a Path)>,
    hidden: bool,
    /// The running entries, whose files are in use (see [`Shelf::in_use`]).
    running: &'a [Running],
}

/// What the outside was last sent.
#[derive(Debug, Default)]
pub(crate) struct Sent {
    /// What shelf showed when the entries it shows were last asked for first.
    shown: Option<ShelfView>,
    /// The folder listed and watched.
    browsing: Option<Browse>,
    /// A hash of the files in use (see [`BrowseCommand::InUse`]); 0 for none.
    in_use: u64,
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

    /// The open browsed folder is listed and watched when it becomes the open one, and the watch is
    /// dropped when there is none. The files in use are told when they change.
    pub(crate) fn browse(&mut self, w: &Wanted, out: &mut Out) {
        let open = w.browsed.map(|(folder, dir)| (folder, dir, w.hidden));
        if self.browsing.as_ref().map(|b| (&b.folder, b.dir.as_path(), b.hidden)) != open {
            self.browsing = open.map(|(folder, dir, hidden)| Browse { folder: folder.clone(), dir: dir.to_path_buf(), hidden });
            out.effects.push(Effect::Browse(BrowseCommand::Open(self.browsing.clone())));
        }
        // Compared by a hash, so that nothing is collected while they stay the same.
        let in_use = w.shelf.in_use(w.running);
        let mut hasher = std::hash::DefaultHasher::new();
        let mut any = false;
        for path in in_use.clone() {
            std::hash::Hash::hash(path, &mut hasher);
            any = true;
        }
        let hash = if any { std::hash::Hasher::finish(&hasher) } else { 0 };
        if hash != self.in_use {
            self.in_use = hash;
            out.effects.push(Effect::Browse(BrowseCommand::InUse(in_use.map(PathBuf::from).collect())));
        }
    }
}

impl State {
    /// What the outside should have as things are now.
    pub(crate) fn wanted(&self) -> Wanted<'_> {
        Wanted {
            shelf: &self.shelf,
            shown: self.shelf.fetch_first(),
            browsed: self.shelf.browsed(),
            hidden: self.shelf.files().hidden(),
            running: self.session.running_entries(),
        }
    }

    /// Sends the outside what differs from what it was last sent.
    pub(crate) fn sync(&mut self, out: &mut Out) {
        let mut sent = std::mem::take(&mut self.sent);
        let wanted = self.wanted();
        sent.fetch_first(&wanted, out);
        sent.browse(&wanted, out);
        self.sent = sent;
    }
}
