//! Setup: the first start's steps, one at a time on the main screen, while the second screen shows only
//! the keyboard (the tab each step needs is chosen below it, and drawn as the keyboard and its editor).
//!
//! 1. A password, typed twice ([`crate::setup::account`]). It can't be skipped: until the account has one,
//!    nothing else of the user's runs (launches are held, see [`crate::State::launch`]), so the helper's
//!    setting a first password without asking for one is safe (`docs/security.md`). Off the device, where
//!    there is no helper, the step says so and Next goes on.
//! 2. Wi-Fi: the Wi-Fi tab's networks. Next goes on, joined or not.
//! 3. The time zone: the time tab. Done ends Setup for good.
//!
//! What shows is worked out from the account as read and the step reached: an account read with no
//! password, or one that couldn't be read, shows the first step whatever was reached, even after Setup was
//! done; a password set moves past it. At the first start the first step shows while it is being read; at
//! a later one nothing shows meanwhile, though programs are held until it reads with a password
//! ([`crate::setup::Setup::holds_programs`]). The step reached isn't saved: after a restart Setup begins at
//! Wi-Fi again.

use super::Setup;
use super::account::{Account, PasswordStep};
use crate::bottom::Tab;
use crate::bottom::editor::{Editor, Purpose};
use crate::effect::{Effect, Save};
use crate::out::Out;
use crate::{Event, SetupId};

/// A step of Setup, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Step {
    #[default]
    Password,
    Wifi,
    Zone,
}

impl Step {
    /// The second screen's tab the step needs: its editor on the keyboard, or the tab whose readings it
    /// shows (their watches follow the tab).
    pub fn tab(self) -> Tab {
        match self {
            Step::Password => Tab::Keyboard,
            Step::Wifi => Tab::Wifi,
            Step::Zone => Tab::Time,
        }
    }
}

impl Setup {
    /// The step of Setup showing, if it shows (see the module's doc).
    pub fn wizard(&self) -> Option<Step> {
        match (&self.account, self.first_start) {
            (Account::Read { password: false, .. } | Account::Failed(_), _) | (Account::Unread, true) => Some(Step::Password),
            (_, false) => None,
            (account, true) => Some(match self.step {
                Step::Password if account.password() => Step::Wifi,
                step => step,
            }),
        }
    }

    /// Whether the password step may be gone past without a password: there is no helper to set one.
    pub fn password_passable(&self) -> bool {
        self.account == Account::NoHelper
    }

    /// Why the password typed last wasn't set, until one is.
    pub fn password_problem(&self) -> Option<&str> {
        self.password_problem.as_deref()
    }

    /// A tap on one of Setup's targets. Done ends Setup for good, the Library showing next with its
    /// selection focused (`selected`) and the default tab below.
    pub(crate) fn wizard_tap(&mut self, id: SetupId, out: &mut Out) {
        let Some(step) = self.wizard() else { return };
        match (id, step) {
            (SetupId::Password, Step::Password) if self.account.available() => {
                let purpose = Purpose::AccountPassword(PasswordStep::New { current: None });
                out.events.push(Event::EditOnKeyboard(Editor { purpose, text: String::new() }));
            }
            (SetupId::Next, Step::Password) if self.password_passable() => self.step = Step::Wifi,
            (SetupId::Next, Step::Wifi) => self.step = Step::Zone,
            (SetupId::Next, Step::Zone) => {
                self.first_start = false;
                out.effects.push(Effect::Save(Save::FirstStartDone));
                out.events.push(Event::ChooseTab(Tab::default()));
            }
            (SetupId::Back, Step::Zone) => self.step = Step::Wifi,
            _ => {}
        }
    }
}

impl Step {
    /// Where the pad's focus starts on the step.
    pub fn first_target(self) -> SetupId {
        match self {
            Step::Password => SetupId::Password,
            Step::Wifi | Step::Zone => SetupId::Next,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::setup::account::{Account, AccountCommand, DIFFERENT};
    use crate::testkit::*;
    use crate::{EditorId, Id, Init, Reading, State};

    fn first_start() -> State {
        let mut s = State::new(Init { first_start: true, account: Account::Unread, ..init() });
        s.applied(Event::Start);
        s
    }

    fn tap(s: &mut State, id: SetupId) -> Vec<Effect> {
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Setup(id))))
    }

    /// A reading: with a helper (`available`), whether there is a password; without, no helper.
    fn account(s: &mut State, available: bool, password: bool) {
        let account = if available { Account::Read { password, ssh: false } } else { Account::NoHelper };
        s.applied(Event::Reading(Reading::Account(account)));
    }

    fn type_and_save(s: &mut State, text: &str) -> Vec<Effect> {
        if let Some(e) = s.editor_mut() {
            e.text = text.to_owned();
        }
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Save))))
    }

    /// The password step can't be passed until the account has one, typed twice the same; then Wi-Fi, the
    /// time zone, and Done ends Setup for good with the Library and the default tab.
    #[test]
    fn setup_goes_password_wifi_zone_and_done_ends_it_for_good() {
        let mut s = first_start();
        assert_eq!((s.top(), s.setup().wizard()), (crate::Top::Setup(Step::Password), Some(Step::Password)));
        assert_eq!(s.pointer().focus(), Some(&Id::Setup(SetupId::Password)));
        account(&mut s, true, false);
        tap(&mut s, SetupId::Next);
        assert_eq!(s.setup().wizard(), Some(Step::Password), "not passed without a password");
        tap(&mut s, SetupId::Password);
        type_and_save(&mut s, "secret12");
        let fx = type_and_save(&mut s, "secret13");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Account(_))), "{fx:?}");
        assert_eq!(s.setup().password_problem(), Some(DIFFERENT));
        tap(&mut s, SetupId::Password);
        type_and_save(&mut s, "secret12");
        let fx = type_and_save(&mut s, "secret12");
        assert!(fx.iter().any(|e| matches!(e, Effect::Account(AccountCommand::Password { current: None, .. }))), "{fx:?}");
        s.applied(Event::AccountChanged(Ok("Password set".into())));
        account(&mut s, true, true);
        assert_eq!(s.setup().wizard(), Some(Step::Wifi));
        assert_eq!(s.setup().password_problem(), None);
        assert_eq!(s.bottom().tab(), Tab::Wifi, "its tab below, its networks watched");
        tap(&mut s, SetupId::Next);
        assert_eq!((s.setup().wizard(), s.bottom().tab()), (Some(Step::Zone), Tab::Time));
        tap(&mut s, SetupId::Back);
        assert_eq!(s.setup().wizard(), Some(Step::Wifi));
        tap(&mut s, SetupId::Next);
        let fx = tap(&mut s, SetupId::Next);
        assert!(fx.contains(&Effect::Save(Save::FirstStartDone)), "{fx:?}");
        assert_eq!((s.top(), s.bottom().tab()), (crate::Top::Library(crate::LibraryMode::Browse), Tab::default()));
    }

    /// Off the device there is no helper: once that is known, Next passes the password step.
    #[test]
    fn without_a_helper_the_password_step_is_passed() {
        let mut s = first_start();
        tap(&mut s, SetupId::Next);
        assert_eq!(s.setup().wizard(), Some(Step::Password), "the account not read yet");
        account(&mut s, false, false);
        tap(&mut s, SetupId::Next);
        assert_eq!(s.setup().wizard(), Some(Step::Wifi));
    }

    /// The account is read at every start, and one with no password brings the password step back, even
    /// once Setup is done; nothing is launched until there is one.
    #[test]
    fn no_password_brings_setup_back_and_holds_launches() {
        let mut s = State::new(Init { account: Account::Unread, ..init() });
        assert!(s.applied(Event::Start).contains(&Effect::Watch(crate::Watch::Account, true)), "read at start");
        assert_eq!(s.setup().wizard(), None);
        account(&mut s, true, false);
        assert_eq!(s.setup().wizard(), Some(Step::Password));
        assert!(!launch_now(&mut s, "zelda-lttp").iter().any(|e| matches!(e, Effect::Launch { .. })), "held");
        assert!(s.session().launching().is_empty());
        let fx = s.applied(Event::Reading(Reading::Account(Account::Read { password: true, ssh: false })));
        assert!(fx.contains(&Effect::Watch(crate::Watch::Account, false)), "read, and Setup gone: {fx:?}");
        assert_eq!(s.setup().wizard(), None);
        assert!(launch_now(&mut s, "zelda-lttp").iter().any(|e| matches!(e, Effect::Launch { .. })));
    }

    /// An account that couldn't be read isn't one without a helper: the password step stays, Next can't
    /// pass it, programs are held, and it is read again; even at a later start, and over a window in front.
    #[test]
    fn an_unreadable_account_keeps_setup_and_holds_programs() {
        let mut s = first_start();
        s.applied(Event::Reading(Reading::Account(Account::Failed("no polkit agent".into()))));
        tap(&mut s, SetupId::Next);
        assert_eq!(s.setup().wizard(), Some(Step::Password), "not passable");
        assert!(s.setup().holds_programs() && s.setup().reads_account(false));
        // A later start: nothing shows while it is read, but programs wait for it.
        let mut s = State::new(Init { account: Account::Unread, ..init() });
        s.applied(Event::Start);
        assert_eq!(s.setup().wizard(), None);
        assert!(!launch_now(&mut s, "zelda-lttp").iter().any(|e| matches!(e, Effect::Launch { .. })), "held while unread");
        s.observed(vec![win(7, "app")]);
        s.applied(crate::WindowEvent::Activate(7).into());
        s.applied(Event::Reading(Reading::Account(Account::Failed("no polkit agent".into()))));
        assert_eq!(s.setup().wizard(), Some(Step::Password));
        assert!(s.session().top_window().is_none(), "Setup in front of the window");
    }

    /// While Setup shows, keys typed on the keyboard go only to its editor.
    #[test]
    fn keys_without_an_editor_go_nowhere_while_setup_shows() {
        let mut s = first_start();
        account(&mut s, false, false);
        tap(&mut s, SetupId::Next);
        let pos = crate::bottom::keyboard::find(crate::bottom::keyboard::Key::Char('m')).unwrap();
        let fx = s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Key(pos))));
        assert!(fx.is_empty(), "{fx:?}");
        assert!(s.library_view().filter().is_empty());
    }
}
