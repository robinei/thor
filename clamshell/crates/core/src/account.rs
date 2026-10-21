//! The device's account and its setup. The session's user starts without a password (locked: no sudo,
//! no ssh); one is set here, and remote login (ssh) turned on once there is one. The system's side is
//! the image's root helper (`thor-account`, which polkit lets the session's user run): read while the
//! setup tab is open and after each change. Off the device (no helper) all of it shows as unavailable.
//!
//! The setup tab gathers what a new device needs: Wi-Fi, the time zone, the password, remote login.
//! It opens by itself on ClamShell's first start (until Done), and from the settings' Setup tile.

use std::fmt;

use crate::State;
use crate::editor::{Editor, Purpose};
use crate::effect::{Effect, Save};
use crate::hardware::NoticeKind;
use crate::tab::Tab;

/// The account as the helper read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Account {
    /// The helper is there (the device's image), and answered.
    pub available: bool,
    /// The session's user has a password.
    pub password: bool,
    /// Remote login (sshd) is on.
    pub ssh: bool,
}

/// A password: kept out of logs (effects and events are logged as they go).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// What the helper is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountCommand {
    /// Read the account.
    Read,
    /// Set the password: `current` when there is one (none: the first).
    Password { current: Option<Secret>, new: Secret },
    /// Remote login on or off.
    Ssh(bool),
}

/// Which of the password's two texts the editor asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordStep {
    /// The current one (there is one).
    Current,
    /// The new one, after the current one if there is one.
    New { current: Option<Secret> },
}

/// The setup tab's targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetupId {
    /// Wi-Fi's row: the Wi-Fi tab.
    Wifi,
    /// The time zone's row: the time tab.
    Zone,
    Password,
    Ssh,
    Done,
}

impl State {
    pub fn account(&self) -> Account {
        self.sys.account
    }

    /// Whether this is the first start: the setup not done yet.
    pub fn first_run(&self) -> bool {
        self.first_run
    }

    /// A tap on one of the setup tab's targets.
    pub(crate) fn on_setup(&mut self, id: SetupId) -> Vec<Effect> {
        let account = self.sys.account;
        match id {
            SetupId::Wifi => self.choose_tab(Tab::Wifi),
            SetupId::Zone => self.choose_tab(Tab::Time),
            SetupId::Password if account.available => {
                let step = if account.password { PasswordStep::Current } else { PasswordStep::New { current: None } };
                self.bottom.open_editor(Editor { purpose: Purpose::AccountPassword(step), text: String::new() });
                vec![]
            }
            // (Remote login needs a password to log in with.)
            SetupId::Ssh if account.available && account.password => vec![Effect::Account(AccountCommand::Ssh(!account.ssh))],
            SetupId::Password | SetupId::Ssh => vec![],
            // The first time: never again (and the device's default tab); later: back to the settings it
            // was opened from.
            SetupId::Done if self.first_run => {
                self.first_run = false;
                let mut effects = vec![Effect::Save(Save::SetupDone)];
                effects.extend(self.choose_tab(Tab::default()));
                effects
            }
            SetupId::Done => self.choose_tab(Tab::Settings),
        }
    }

    /// The password's editor saved: the new one asked for after the current one, or the password set.
    pub(crate) fn password_typed(&mut self, step: PasswordStep, text: String) -> Vec<Effect> {
        match step {
            PasswordStep::Current => {
                let step = PasswordStep::New { current: Some(Secret(text)) };
                self.bottom.open_editor(Editor { purpose: Purpose::AccountPassword(step), text: String::new() });
                vec![]
            }
            PasswordStep::New { current } => vec![Effect::Account(AccountCommand::Password { current, new: Secret(text) })],
        }
    }

    /// A change asked of the helper is done (what it did), or failed (why): shown for a moment.
    pub(crate) fn on_account_changed(&mut self, result: Result<String, String>) {
        self.notify(NoticeKind::Account(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::testkit::*;
    use crate::{EditorId, Event, Id, Init, Reading};

    fn setup(s: &mut State, id: SetupId) -> Vec<Effect> {
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Setup(id)))).1
    }

    fn type_and_save(s: &mut State, text: &str) -> Vec<Effect> {
        if let Some(e) = s.bottom.editor_mut() {
            e.text = text.to_owned();
        }
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Save)))).1
    }

    fn password(s: &State) -> Option<&PasswordStep> {
        match s.bottom().editor() {
            Some(Editor { purpose: Purpose::AccountPassword(step), .. }) => Some(step),
            _ => None,
        }
    }

    #[test]
    fn the_first_password_then_the_current_one_before_a_new_one() {
        let mut s = State::new(Init { setup: true, ..init() });
        assert_eq!(s.bottom().tab(), Tab::Setup, "the first start opens the setup");
        assert!(setup(&mut s, SetupId::Password).is_empty());
        assert_eq!(password(&s), None, "no helper: nothing to set");
        s.apply(Reading::Account(Account { available: true, password: false, ssh: false }).into());
        setup(&mut s, SetupId::Password);
        assert_eq!(password(&s), Some(&PasswordStep::New { current: None }), "none yet: the new one at once");
        let set = |current: Option<&str>, new: &str| {
            vec![Effect::Account(AccountCommand::Password { current: current.map(|c| Secret(c.to_owned())), new: Secret(new.to_owned()) })]
        };
        assert_eq!(type_and_save(&mut s, "first"), set(None, "first"));
        s.apply(Reading::Account(Account { available: true, password: true, ssh: false }).into());
        setup(&mut s, SetupId::Password);
        assert_eq!(password(&s), Some(&PasswordStep::Current));
        assert!(type_and_save(&mut s, "first").is_empty(), "the current one, then");
        assert_eq!(password(&s), Some(&PasswordStep::New { current: Some(Secret("first".to_owned())) }));
        assert_eq!(type_and_save(&mut s, "second"), set(Some("first"), "second"));
        assert_eq!(format!("{:?}", set(Some("first"), "second")).matches("first").count(), 0, "kept out of logs");
    }

    #[test]
    fn remote_login_needs_a_password() {
        let mut s = state();
        s.apply(Reading::Account(Account { available: true, password: false, ssh: false }).into());
        assert!(setup(&mut s, SetupId::Ssh).is_empty(), "refused without one");
        s.apply(Reading::Account(Account { available: true, password: true, ssh: false }).into());
        assert_eq!(setup(&mut s, SetupId::Ssh), vec![Effect::Account(AccountCommand::Ssh(true))]);
    }

    #[test]
    fn done_the_first_time_is_saved_and_the_setup_stays_reachable() {
        let mut s = State::new(Init { setup: true, ..init() });
        assert!(s.first_run());
        let effects = setup(&mut s, SetupId::Done);
        assert_eq!(effects.first(), Some(&Effect::Save(Save::SetupDone)));
        assert!(!s.first_run());
        assert_eq!(s.bottom().tab(), Tab::default());
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Settings(crate::SettingsId::Setup))));
        assert_eq!(s.bottom().tab(), Tab::Setup, "from the settings' tile");
        let effects = setup(&mut s, SetupId::Done);
        assert!(!effects.contains(&Effect::Save(Save::SetupDone)), "saved once");
        assert_eq!(s.bottom().tab(), Tab::Settings, "back where it came from");
        let s = State::new(init());
        assert_eq!(s.bottom().tab(), Tab::default(), "done before: not opened");
    }

    #[test]
    fn a_change_is_shown() {
        let mut s = state();
        s.apply(Event::AccountChanged(Err("wrong password".to_owned())));
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::Account(Err("wrong password".to_owned()))));
    }
}
