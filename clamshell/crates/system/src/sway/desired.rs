//! The placements sent by state, retained across IPC failures. Observations initialise a new window;
//! after that only a state command changes its place. Focus observations are allowed to select tabs,
//! except while a focus command is waiting to take effect.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clamshell_state::Instance;
use clamshell_state::session::windows::Window;

use super::{Command, commands};

#[derive(Clone, Default)]
pub(super) struct Desired(Arc<Mutex<Layout>>);

#[derive(Default)]
struct Layout {
    places: HashMap<i64, Place>,
    focus: Option<(Focus, bool)>,
    bottom: Option<(i64, bool)>,
}

struct Place {
    group: String,
    down: bool,
}

enum Focus {
    Group(String),
    Window(i64),
}

impl Desired {
    /// Records the request before trying IPC, and normalises it to what state means. Moving a group
    /// leaves windows moved down alone, including a late initial placement after its down rule ran.
    pub(super) fn request(&self, command: &Command) -> Command {
        let mut layout = crate::os::lock(&self.0);
        match command {
            Command::Workspace(group) => layout.focus = Some((Focus::Group(group.clone()), true)),
            Command::Focus(id) => layout.focus = Some((Focus::Window(*id), true)),
            Command::Move { windows, workspace } => {
                let windows: Vec<i64> = windows.iter().copied().filter(|id| !layout.places.get(id).is_some_and(|p| p.down)).collect();
                for id in &windows {
                    layout.places.insert(*id, Place { group: workspace.clone(), down: false });
                }
                return Command::Move { windows, workspace: workspace.clone() };
            }
            Command::Down(id) => {
                layout.places.insert(*id, Place { group: crate::sway::BOTTOM.to_owned(), down: true });
                layout.bottom = Some((*id, true));
                if matches!(&layout.focus, Some((Focus::Window(w), _)) if w == id) {
                    layout.focus = None;
                }
            }
            Command::Up { window, workspace } => {
                layout.places.insert(*window, Place { group: workspace.clone(), down: false });
                layout.focus = Some((Focus::Window(*window), true));
                if layout.bottom.is_some_and(|(id, _)| id == *window) {
                    layout.bottom = None;
                }
            }
            Command::ShowDown(id) => layout.bottom = Some((*id, true)),
            Command::Launch { .. } | Command::Close(_) | Command::Power(_) | Command::DownRules(_) | Command::LandRules(_) => {}
        }
        command.clone()
    }

    /// A successful IPC reply acknowledges even an idempotent focus, which brings no window event.
    /// Without this a later user selection would be mistaken for an unfulfilled request.
    pub(super) fn carried_out(&self, command: &Command) {
        let mut layout = crate::os::lock(&self.0);
        match (&mut layout.focus, command) {
            (Some((Focus::Window(wanted), pending)), Command::Focus(id)) if wanted == id => *pending = false,
            (Some((Focus::Window(wanted), pending)), Command::Up { window, .. }) if wanted == window => *pending = false,
            (Some((Focus::Window(wanted), pending)), Command::Move { windows, .. }) if windows.contains(wanted) => *pending = false,
            (Some((Focus::Group(wanted), pending)), Command::Workspace(group)) if wanted == group => *pending = false,
            _ => {}
        }
        match (&mut layout.bottom, command) {
            (Some((wanted, pending)), Command::Down(id) | Command::ShowDown(id)) if wanted == id => *pending = false,
            _ => {}
        }
    }

    pub(super) fn focus_after_move(&self, windows: &[i64]) -> Option<String> {
        let layout = crate::os::lock(&self.0);
        match &layout.focus {
            Some((Focus::Window(id), _)) if windows.contains(id) => Some(commands::focus(*id)),
            _ => None,
        }
    }

    pub(super) fn reconcile(&self, windows: &[Window], top: Option<&str>) -> Vec<String> {
        let mut layout = crate::os::lock(&self.0);
        layout.places.retain(|id, _| windows.iter().any(|w| w.id == *id));
        let mut out = Vec::new();
        let focus_id = match &layout.focus {
            Some((Focus::Window(id), pending)) if *pending || windows.iter().any(|w| w.id == *id && w.focused && !w.down) => Some(*id),
            _ => None,
        };
        let mut moving_focus = false;
        for w in windows {
            let place = layout.places.entry(w.id).or_insert_with(|| Place { group: w.workspace.clone(), down: w.down });
            if place.down {
                if !w.down || w.workspace != crate::sway::BOTTOM {
                    out.push(commands::down(w.id, top));
                }
            } else if w.down {
                out.push(commands::up(w.id, Some(&place.group), top));
                moving_focus |= focus_id == Some(w.id);
            } else if w.workspace != place.group {
                out.push(commands::move_windows(&[w.id], &place.group, top));
                moving_focus |= focus_id == Some(w.id);
            }
        }
        let observed = windows.iter().find(|w| w.focused && !w.down && !layout.places.get(&w.id).is_some_and(|p| p.down));
        match &mut layout.focus {
            Some((Focus::Group(group), pending)) if *pending => {
                if observed.is_some_and(|w| w.workspace == *group && (!Instance::Shelf.is(group) || w.is_shelf())) {
                    *pending = false;
                } else if windows.iter().any(|w| !w.down && w.workspace == *group) {
                    out.push(commands::show(group, top));
                }
            }
            Some((Focus::Window(id), pending)) if *pending => {
                if moving_focus {
                    // The move and focus are sent together; the observation predates both.
                } else if observed.is_some_and(|w| w.id == *id) {
                    *pending = false;
                } else if windows.iter().any(|w| w.id == *id && !w.down) {
                    out.push(commands::focus(*id));
                } else {
                    layout.focus = None;
                }
            }
            _ => {
                if let Some(w) = observed {
                    layout.focus = Some((Focus::Window(w.id), false));
                }
            }
        }
        if moving_focus && let Some(id) = focus_id {
            out.push(commands::focus(id));
        }
        if layout.bottom.is_some_and(|(id, _)| !layout.places.get(&id).is_some_and(|p| p.down)) {
            layout.bottom = None;
        }
        let observed_bottom = windows.iter().find(|w| w.down && w.visible && layout.places.get(&w.id).is_some_and(|p| p.down));
        match &mut layout.bottom {
            Some((id, pending)) if *pending => {
                if windows.iter().any(|w| w.id == *id && w.down && w.visible) {
                    *pending = false;
                } else if windows.iter().any(|w| w.id == *id && w.down) {
                    out.push(commands::show_down(*id, top));
                } else if !windows.iter().any(|w| w.id == *id) {
                    layout.bottom = None;
                }
            }
            _ => {
                if let Some(w) = observed_bottom {
                    layout.bottom = Some((w.id, false));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: i64, group: &str, focused: bool) -> Window {
        Window { id, workspace: group.into(), focused, visible: focused, ..Window::default() }
    }

    #[test]
    fn a_failed_move_is_reconciled_and_kept_across_reconnects() {
        let desired = Desired::default();
        let mut w = window(7, "old", true);
        assert!(desired.reconcile(std::slice::from_ref(&w), Some("TOP")).is_empty());
        desired.request(&Command::Move { windows: vec![7], workspace: "new".into() });
        assert_eq!(desired.reconcile(std::slice::from_ref(&w), Some("TOP")), vec![commands::move_windows(&[7], "new", Some("TOP")), commands::focus(7)]);
        w.workspace = "new".into();
        assert!(desired.reconcile(std::slice::from_ref(&w), Some("TOP")).is_empty());
        let desired = desired.clone();
        w.workspace = "old".into();
        assert_eq!(desired.reconcile(&[w], Some("TOP")), vec![commands::move_windows(&[7], "new", Some("TOP")), commands::focus(7)]);
        assert!(desired.reconcile(&[], Some("TOP")).is_empty());
    }

    #[test]
    fn a_late_group_move_cannot_take_a_window_back_up() {
        let desired = Desired::default();
        let w = window(7, "group", true);
        desired.reconcile(std::slice::from_ref(&w), Some("TOP"));
        desired.request(&Command::Down(7));
        assert_eq!(
            desired.request(&Command::Move { windows: vec![7], workspace: "group".into() }),
            Command::Move { windows: vec![], workspace: "group".into() }
        );
        assert_eq!(desired.reconcile(&[w], Some("TOP")), vec![commands::down(7, Some("TOP"))]);
        let down = Window { down: true, visible: true, ..window(7, crate::sway::BOTTOM, false) };
        assert!(desired.reconcile(std::slice::from_ref(&down), Some("TOP")).is_empty());
        desired.request(&Command::Up { window: 7, workspace: "group".into() });
        assert_eq!(desired.reconcile(&[down], Some("TOP")), vec![commands::up(7, Some("group"), Some("TOP")), commands::focus(7)]);
    }

    #[test]
    fn pending_focus_is_retried_then_user_selection_is_accepted() {
        let desired = Desired::default();
        let one = window(7, "group", true);
        let two = window(8, "group", false);
        desired.reconcile(&[one.clone(), two.clone()], None);
        desired.request(&Command::Focus(8));
        assert_eq!(desired.reconcile(&[one.clone(), two.clone()], None), vec![commands::focus(8)]);
        assert!(desired.reconcile(&[Window { focused: false, ..one.clone() }, Window { focused: true, ..two.clone() }], None).is_empty());
        assert!(desired.reconcile(&[one.clone(), two.clone()], None).is_empty());
        let desired = desired.clone();
        assert!(desired.reconcile(&[Window { focused: false, ..one }, Window { focused: true, ..two }], None).is_empty());
    }

    #[test]
    fn an_idempotent_focus_is_acknowledged_without_a_window_event() {
        let desired = Desired::default();
        let one = window(7, "group", true);
        let two = window(8, "group", false);
        desired.reconcile(&[one.clone(), two.clone()], None);
        let command = desired.request(&Command::Focus(7));
        desired.carried_out(&command);
        assert!(desired.reconcile(&[Window { focused: false, ..one }, Window { focused: true, ..two }], None).is_empty());
    }
}
