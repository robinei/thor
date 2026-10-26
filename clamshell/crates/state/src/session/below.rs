//! The bottom screen's window, such as a dual-screen emulator's second screen.
//!
//! Home + right stick down puts the window in front on the top screen below, returning the previous
//! occupant to its home group. Up brings the bottom window back, even while ClamShell's view covers it.
//! Which of the two the bottom screen shows, and its power, are [`crate::Bottom`]'s.
//!
//! Moving a game's or a dock app's window down is remembered as a rule ([`WindowRule`]), so the next
//! time the program opens a window like it, the window goes down by itself. Moving it up forgets the
//! rule. A rule holds the program and the window's app id. It holds the title too, but only when the
//! program has another window with that app id open, as a dual-screen emulator does, since titles change
//! with what a window shows.
//!
//! The rules are saved. A window that matches one is moved down once, after the event that opened it or
//! gave it the title ([`Session::follow_window_rules`]).

use std::sync::Arc;

use crate::Session;
use crate::bottom::notice::{NoticeKind, NoticeSlot};
use crate::effect::{Effect, Save};
use crate::out::Out;
use crate::session::windows::{WindowCommand, WindowFacts};

/// A window that goes to the bottom screen when its entry opens it. The rule names the entry by its id,
/// the app id, and optionally the title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    pub owner: Arc<str>,
    pub app_id: String,
    pub title: Option<String>,
}

impl WindowRule {
    pub(crate) fn matches(&self, owner: &str, w: &WindowFacts) -> bool {
        &*self.owner == owner && self.app_id == w.app_id && self.title.as_ref().is_none_or(|t| *t == w.title)
    }
}

/// The rules for which programs' windows go to the bottom screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowRules {
    list: Vec<WindowRule>,
}

impl WindowRules {
    /// The rules from their saved list.
    pub fn from_list(list: Vec<WindowRule>) -> WindowRules {
        WindowRules { list }
    }

    pub fn list(&self) -> &[WindowRule] {
        &self.list
    }

    /// Adds a rule unless there is one like it. Returns whether it did.
    fn add(&mut self, rule: WindowRule) -> bool {
        if self.list.contains(&rule) {
            return false;
        }
        self.list.push(rule);
        true
    }

    /// Forgets the rules that match a window of `owner`. Returns whether there were any.
    fn forget(&mut self, owner: &str, w: &WindowFacts) -> bool {
        let before = self.list.len();
        self.list.retain(|r| !r.matches(owner, w));
        self.list.len() != before
    }

    fn wants(&self, owner: &str, w: &WindowFacts) -> bool {
        self.list.iter().any(|r| r.matches(owner, w))
    }
}

impl Session {
    pub fn window_rules(&self) -> &WindowRules {
        &self.window_rules
    }

    /// Home + right stick down moves the window in front on the top screen to the bottom screen and
    /// remembers it for its program. Returns whether a window moved, for the bottom screen to show it.
    pub(crate) fn window_down(&mut self, notice: &mut NoticeSlot, now: u64, out: &mut Out) -> bool {
        let Some(w) = self.top_window().cloned() else {
            return false;
        };
        self.command(WindowCommand::Down(w.id));
        // Sent down already, so the rule added below doesn't send it again.
        if !self.sent_down.contains(&w.id) {
            self.sent_down.push(w.id);
        }
        // The title is only used to tell it apart from another window of its program with the same
        // app id.
        let others = self.windows.iter().any(|o| o.id != w.id && o.app_id == w.app_id && o.unit == w.unit);
        let title = others.then(|| w.title.clone());
        if let Some(owner) = self.owner_of(&w)
            && self.window_rules.add(WindowRule { owner: owner.clone(), app_id: w.app_id.clone(), title })
        {
            // A remembered rule applies to subsequent windows. Creating it must not move an already
            // placed sibling with the same app id, including another launch of this program.
            let siblings: Vec<i64> = self.windows_of(&owner).filter(|other| other.app_id == w.app_id).map(|other| other.id).collect();
            for id in siblings {
                if !self.sent_down.contains(&id) {
                    self.sent_down.push(id);
                }
            }
            self.window_rules_changed(true, notice, now, out);
        }
        true
    }

    /// Home + right stick up moves the window on the bottom screen back to its group on the top screen,
    /// in front, and forgets its rule. The bottom screen then shows ClamShell's view.
    pub(crate) fn window_up(&mut self, notice: &mut NoticeSlot, now: u64, out: &mut Out) {
        let Some(w) = self.bottom_window().cloned() else {
            return;
        };
        self.command(WindowCommand::Up(w.id));
        if let Some(owner) = self.owner_of(&w)
            && self.window_rules.forget(&owner, &w)
        {
            self.window_rules_changed(false, notice, now, out);
        }
    }

    /// A rule was added or forgotten. That is shown in a notice, and the rules are saved.
    fn window_rules_changed(&self, added: bool, notice: &mut NoticeSlot, now: u64, out: &mut Out) {
        notice.show(NoticeKind::BottomRule(added), now);
        out.effects.push(self.window_rules_save());
    }

    /// Saves the window rules.
    pub(crate) fn window_rules_save(&self) -> Effect {
        Effect::Save(Save::WindowRules(self.window_rules.clone()))
    }

    /// Runs after every event. A window whose program has a rule for it is sent down, once. Returns
    /// whether one was, for the bottom screen to show it.
    pub(crate) fn follow_window_rules(&mut self, library: &crate::Library) -> bool {
        let wanted: Vec<i64> = self
            .windows
            .iter()
            .filter(|w| !self.below(w.id) && !self.sent_down.contains(&w.id))
            .filter(|w| {
                let owner = self.owner_of(w).or_else(|| {
                    let key = w.group.as_ref()?.key()?;
                    library.entries.iter().find(|g| g.key == key).map(|g| g.id.clone())
                });
                owner.is_some_and(|o| self.window_rules.wants(&o, w))
            })
            .map(|w| w.id)
            .collect();
        let sent = !wanted.is_empty();
        for id in wanted {
            self.sent_down.push(id);
            self.command(WindowCommand::Down(id));
        }
        let windows = &self.windows;
        self.sent_down.retain(|id| windows.iter().any(|w| w.id == *id));
        sent
    }
}
