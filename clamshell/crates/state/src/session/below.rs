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

/// A live window already considered for automatic placement, with the rule that actually moved it.
/// Siblings skipped when a rule is created have no applied rule.
#[derive(Debug, Clone)]
pub(super) struct SentDown {
    window: i64,
    rule: Option<WindowRule>,
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

    /// Forgets the exact applied rule, independent of a live window's changing facts.
    fn forget(&mut self, rule: &WindowRule) -> bool {
        let before = self.list.len();
        self.list.retain(|r| r != rule);
        self.list.len() != before
    }

    fn matching(&self, owner: &str, w: &WindowFacts) -> Option<&WindowRule> {
        self.list.iter().find(|r| r.matches(owner, w))
    }
}

impl Session {
    pub fn window_rules(&self) -> &WindowRules {
        &self.window_rules
    }

    /// Home + right stick down moves the window in front on the top screen to the bottom screen and
    /// remembers it for its program. Returns whether a window moved, for the bottom screen to show it.
    /// The rule and the record are the stack's root's, though a dialog of it may be in front: the stack
    /// moves as one, rules apply only to roots ([`Session::wanted_down`]), and Up finds the record by the
    /// root on the bottom screen.
    pub(crate) fn window_down(&mut self, notice: &mut NoticeSlot, now: u64, out: &mut Out) -> bool {
        let Some(w) = self.top.and_then(|root| self.window(root)).cloned() else {
            return false;
        };
        self.command(WindowCommand::Down(w.id));
        // A displaced window keeps the rule that moved it, even if its facts have changed.
        if self.sent_down.iter().any(|sent| sent.window == w.id && sent.rule.as_ref().is_some_and(|rule| self.window_rules.list.contains(rule))) {
            return true;
        }
        // The title is only used to tell it apart from another window of its program with the same
        // app id.
        let others = self.windows.iter().any(|o| o.id != w.id && o.app_id == w.app_id && o.unit == w.unit);
        let title = others.then(|| w.title.clone());
        let rule = self.owner_of(&w).map(|owner| WindowRule { owner, app_id: w.app_id.clone(), title });
        self.note_down(w.id, rule.clone());
        if let Some(rule) = rule {
            let owner = rule.owner.clone();
            if self.window_rules.add(rule) {
                // A remembered rule applies to subsequent windows, preserving already placed siblings.
                let siblings: Vec<i64> = self.windows_of(&owner).filter(|other| other.app_id == w.app_id).map(|other| other.id).collect();
                for id in siblings {
                    if !self.sent_down.iter().any(|sent| sent.window == id) {
                        self.note_down(id, None);
                    }
                }
                self.window_rules_changed(true, notice, now, out);
            }
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
        if let Some(rule) = self.sent_down.iter_mut().find(|sent| sent.window == w.id).and_then(|sent| sent.rule.take())
            && self.window_rules.forget(&rule)
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

    /// Runs after every event. A window whose program has a rule for it is sent down, once. One displaced
    /// from the bottom screen by it is looked at in turn, so that its own rule applies in this settle too.
    /// Returns whether one was sent, for the bottom screen to show it.
    pub(crate) fn follow_window_rules(&mut self, library: &crate::Library) -> bool {
        let mut sent = false;
        // (Each window is sent once, so this ends.)
        while let Some((id, rule)) = self.wanted_down(library) {
            sent = true;
            self.note_down(id, Some(rule));
            self.command(WindowCommand::Down(id));
        }
        let windows = &self.windows;
        self.sent_down.retain(|sent| windows.iter().any(|w| w.id == sent.window));
        sent
    }

    fn note_down(&mut self, window: i64, rule: Option<WindowRule>) {
        if let Some(sent) = self.sent_down.iter_mut().find(|sent| sent.window == window) {
            sent.rule = rule;
        } else {
            self.sent_down.push(SentDown { window, rule });
        }
    }

    #[cfg(test)]
    pub(crate) fn down_history_valid(&self) -> bool {
        self.sent_down
            .iter()
            .enumerate()
            .all(|(i, sent)| self.window(sent.window).is_some() && !self.sent_down.iter().take(i).any(|other| other.window == sent.window))
    }

    /// The first main window on the top screen, not sent down before, whose program's rule wants it below.
    fn wanted_down(&self, library: &crate::Library) -> Option<(i64, WindowRule)> {
        self.windows.iter().filter(|w| !self.below(w.id) && !self.sent_down.iter().any(|sent| sent.window == w.id) && self.root_of(w.id) == w.id).find_map(
            |w| {
                let owner = self.owner_of(w).or_else(|| {
                    let key = w.group.as_ref()?.key()?;
                    library.entry_by_key(key).map(|g| g.id.clone())
                })?;
                self.window_rules.matching(&owner, w).map(|rule| (w.id, rule.clone()))
            },
        )
    }
}
