//! Launching entries: played, shown, opened again, enqueued, and what is done to an entry by the dock or
//! the Library.

use std::sync::Arc;

use crate::out::Out;
use crate::session::running::{Instead, LaunchRequest, Launched, Start};
use crate::session::windows::WindowCommand;
use crate::{Effect, EntryAct, Launch, LaunchEvent, Save, State};
use clamshell_library::Builtin;

impl State {
    /// Something done to entry `id`.
    pub(crate) fn on_entry(&mut self, id: Arc<str>, act: EntryAct, out: &mut Out) {
        match act {
            EntryAct::Play => self.play(id, out),
            // A built-in is shown as it is started.
            EntryAct::Show if Builtin::of(&id).is_some() => self.play(id, out),
            EntryAct::Show => self.show(&id, out),
            EntryAct::OpenAnother => self.open_another(id, out),
            EntryAct::Suspend => self.session.suspend(&id, self.clock.mono(), out),
            EntryAct::ToggleAutostart => self.session.toggle_autostart(id, out),
            EntryAct::TogglePin => self.dock.toggle_pin(id, &self.session, out),
            // A built-in isn't a process: Quit closes the Device view, and the Library always runs.
            EntryAct::Quit | EntryAct::ForceKill if Builtin::of(&id).is_some() => {
                if Builtin::of(&id) == Some(Builtin::Device) {
                    self.close_device();
                }
            }
            EntryAct::Quit => self.session.quit(&id, self.clock.mono(), self.library_view.library(), out),
            EntryAct::ForceKill => self.session.force_kill(&id, self.clock.mono(), self.library_view.library(), out),
        }
    }

    pub(crate) fn on_launch_event(&mut self, event: LaunchEvent) {
        match event {
            LaunchEvent::Position { entry, position } => self.library_view.set_position(&entry, position, self.clock.unix()),
            // A launch that never started doesn't change what runs, but it is no longer starting.
            LaunchEvent::LaunchFailed { entry, unit } => {
                self.session.launch_failed(entry, &unit, self.library_view.library(), &self.clock, self.bottom.notice_mut())
            }
        }
    }

    /// The Library's action on its selected entry, or a tap on a stopped dock entry: shows the entry if it runs,
    /// or else starts it. A group isn't started; it is shown open in the Library.
    fn play(&mut self, entry: Arc<str>, out: &mut Out) {
        // A browsed file that is also a library entry plays as that entry.
        let entry = self.library_view.entry_of_file(&entry).unwrap_or(entry);
        if self.session.running(&entry).is_some() {
            self.show(&entry, out);
            return;
        }
        let title = self.library_view.entry(&entry).map(|g| g.title.clone());
        // The notice comes at once, since a start may take a while: Android's first start, a Steam
        // game's, or Proton's setup.
        if self.launch(entry, None, Start::Front, out) == Launched::Yes
            && let Some(title) = title
        {
            self.bottom.notice_mut().show(crate::bottom::notice::NoticeKind::Starting(title), self.clock.mono());
        }
    }

    /// Shows running entry `id` (see [`crate::Session::show`]), on the bottom screen if all its windows are there.
    fn show(&mut self, id: &str, out: &mut Out) {
        let below = self.session.windows_of(id).any(|w| self.session.below(w.id));
        if self.session.show(id, &self.clock, self.library_view.library(), self.bottom.notice_mut(), out)
            || (below && !self.bottom.shows_windows(&self.session))
        {
            self.bottom.show_below();
        }
    }

    /// Shows built-in `b` on the main screen, with the pad's focus on it: the Device view's first row, or the
    /// Library's selection.
    pub(crate) fn show_builtin(&mut self, b: Builtin) {
        self.session.cancel_front_launch();
        self.session.command(WindowCommand::Show(b));
        match b {
            Builtin::Device => self.pointer.focus_on(crate::Id::Device(crate::DeviceId::Wifi)),
            Builtin::Library => self.pointer.focus_library_view(self.library_view.selected()),
        }
    }

    /// Closes the Device view: the Library shows in its place if it showed.
    pub(crate) fn close_device(&mut self) {
        let shown = self.session.shown_builtin() == Some(Builtin::Device);
        self.session.close_device();
        if shown {
            self.pointer.focus_library_view(self.library_view.selected());
        }
    }

    /// The dock menu's Open another: launches `id` again, if that is offered (see [`State::opens_another`]).
    fn open_another(&mut self, id: Arc<str>, out: &mut Out) {
        if self.opens_another(&id) {
            self.launch(id, None, Start::Front, out);
        }
    }

    /// Launches `entry` (see [`crate::Session::launch`]), or does what the session says instead: the Device view
    /// is opened, a group shown open in the Library, or a running entry shown. As a launch starts, where the Library
    /// and the second screen are is saved.
    pub(crate) fn launch(&mut self, entry: Arc<str>, command: Option<Launch>, start: Start, out: &mut Out) -> Launched {
        // Nothing of the user's runs until the account reads with a password (or there is no helper): it may
        // have none yet, which any program of the user could then set (see `crate::setup::wizard`).
        if self.setup.holds_programs() {
            return Launched::No;
        }
        let request = LaunchRequest { entry: entry.clone(), command, start };
        let launched = self.session.launch(request, self.library_view.library(), self.prefs.settings(), &self.clock, out);
        match launched {
            Launched::Yes => out.effects.push(Effect::Save(Save::Resume(self.resume()))),
            Launched::Instead(Instead::Builtin(b)) => self.show_builtin(b),
            Launched::Instead(Instead::Group) if self.library_view.reveal(&entry) => self.show_builtin(Builtin::Library),
            Launched::Instead(Instead::Show) => self.show(&entry, out),
            Launched::Instead(Instead::Group) | Launched::No => {}
        }
        launched
    }

    /// Enqueue was pressed on what the Library's details show. The entries are added to the running launch of
    /// their runner, or else launched (see [`crate::session::enqueue`]).
    pub(crate) fn enqueue(&mut self, out: &mut Out) {
        let Some(enqueued) = self.library_view.take_enqueued() else {
            return;
        };
        if self.session.enqueue(&enqueued, self.bottom.notice_mut(), self.clock.mono(), out) {
            return;
        }
        if enqueued.alone {
            // A file alone is opened as A opens it, which includes resuming where it was left.
            self.play(enqueued.entry, out);
        } else {
            let entry = self.library_view.entry_of_file(&enqueued.entry).unwrap_or(enqueued.entry);
            self.launch(entry, Some(enqueued.runner.command_for(&enqueued.files)), Start::Front, out);
        }
    }
}
