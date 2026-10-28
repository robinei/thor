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
//! password shows the first step whatever was reached, even after Setup was done; a password set moves past
//! it. The step reached isn't saved: after a restart Setup begins at Wi-Fi again.

use std::sync::Arc;

use super::Setup;
use super::account::PasswordStep;
use crate::bottom::Tab;
use crate::bottom::editor::{Editor, Purpose};
use crate::effect::{Effect, Save};
use crate::out::Out;
use crate::{Event, Id, Pointer, SetupId};

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
        if self.account.available && !self.account.password {
            return Some(Step::Password);
        }
        if !self.first_start {
            return None;
        }
        Some(match self.step {
            Step::Password if self.account.password => Step::Wifi,
            step => step,
        })
    }

    /// Whether the password step may be gone past without a password: the account was read, and there is
    /// no helper to set one.
    pub fn password_passable(&self) -> bool {
        self.account_read && !self.account.available
    }

    /// Why the password typed last wasn't set, until one is.
    pub fn password_problem(&self) -> Option<&str> {
        self.password_problem.as_deref()
    }

    /// A tap on one of Setup's targets. Done ends Setup for good, the Library showing next with its
    /// selection focused (`selected`) and the default tab below.
    pub(crate) fn wizard_tap(&mut self, id: SetupId, pointer: &mut Pointer, selected: Option<&Arc<str>>, out: &mut Out) {
        let Some(step) = self.wizard() else { return };
        match (id, step) {
            (SetupId::Password, Step::Password) if self.account.available => {
                let purpose = Purpose::AccountPassword(PasswordStep::New { current: None });
                out.events.push(Event::EditOnKeyboard(Editor { purpose, text: String::new() }));
            }
            (SetupId::Next, Step::Password) if self.password_passable() => self.step = Step::Wifi,
            (SetupId::Next, Step::Wifi) => self.step = Step::Zone,
            (SetupId::Next, Step::Zone) => {
                self.first_start = false;
                pointer.focus_library_view(selected);
                out.effects.push(Effect::Save(Save::FirstStartDone));
                out.events.push(Event::ChooseTab(Tab::default()));
            }
            (SetupId::Back, Step::Zone) => self.step = Step::Wifi,
            _ => {}
        }
        if let Some(step) = self.wizard().filter(|now| *now != step) {
            pointer.focus_on(Id::Setup(step.first_target()));
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
    use crate::setup::TopView;
    use crate::setup::account::{Account, AccountCommand, DIFFERENT};
    use crate::testkit::*;
    use crate::{EditorId, Init, Reading, State};

    fn first_start() -> State {
        let mut s = State::new(Init { first_start: true, ..init() });
        s.applied(Event::Start);
        s
    }

    fn tap(s: &mut State, id: SetupId) -> Vec<Effect> {
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Setup(id))))
    }

    fn account(s: &mut State, available: bool, password: bool) {
        s.applied(Event::Reading(Reading::Account(Account { available, password, ssh: false })));
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
        assert_eq!((s.setup().top_view(), s.setup().wizard()), (TopView::Setup, Some(Step::Password)));
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
        assert_eq!((s.setup().top_view(), s.bottom().tab()), (TopView::LibraryView, Tab::default()));
        assert_eq!(s.pointer().focus(), Some(&entry("zelda-lttp")));
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

    /// An account read with no password brings the password step back, even once Setup is done, and
    /// nothing is launched until there is one.
    #[test]
    fn no_password_brings_setup_back_and_holds_launches() {
        let mut s = state();
        assert_eq!(s.setup().wizard(), None);
        account(&mut s, true, false);
        assert_eq!(s.setup().wizard(), Some(Step::Password));
        assert!(!launch_now(&mut s, "zelda-lttp").iter().any(|e| matches!(e, Effect::Launch { .. })), "held");
        assert!(s.session().launching().is_empty());
        account(&mut s, true, true);
        assert_eq!(s.setup().wizard(), None);
        assert!(launch_now(&mut s, "zelda-lttp").iter().any(|e| matches!(e, Effect::Launch { .. })));
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
