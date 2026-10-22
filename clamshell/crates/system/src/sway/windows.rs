//! sway's tree as the state sees it (the windows, with the systemd unit of their process), and what to
//! ask sway after a window change. Pure: no IPC here.

use std::collections::HashSet;
use std::sync::Arc;

use clamshell_core::bottom_screen::DownRule;
use clamshell_core::instance::Instance;
use clamshell_core::windows::Window;

use super::commands;
use super::ipc::Node;

/// The windows in a tree: leaf containers with a process, with their workspace. `unit` gives a pid's
/// systemd unit.
pub fn windows(tree: &Node, unit: &mut impl FnMut(u32) -> (Option<String>, Vec<u32>)) -> Vec<Window> {
    let mut out = Vec::new();
    collect(tree, "", unit, &mut out);
    out
}

fn collect(node: &Node, workspace: &str, unit: &mut impl FnMut(u32) -> (Option<String>, Vec<u32>), out: &mut Vec<Window>) {
    let workspace = match node.kind.as_str() {
        "workspace" => node.name.as_deref().unwrap_or(""),
        _ => workspace,
    };
    if let (Some(pid), "con" | "floating_con") = (node.pid, node.kind.as_str()) {
        // Wayland windows have an app id; X11 ones a class.
        let app_id = node.app_id.as_deref().or_else(|| node.window_properties.as_ref().and_then(|p| p.class.as_deref())).unwrap_or("");
        let (unit, pids) = unit(pid);
        out.push(Window {
            id: node.id,
            app_id: app_id.to_owned(),
            title: node.name.clone().unwrap_or_default(),
            focused: node.focused,
            unit,
            pids,
            workspace: workspace.to_owned(),
            down: node.marks.iter().any(|m| m.starts_with(commands::DOWN_MARK)),
            visible: node.visible.unwrap_or(false),
        });
    }
    for child in node.children() {
        collect(child, workspace, unit, out);
    }
}

/// What to ask sway after a change of windows (`down`: the down rules in force): each window of one of our
/// instances that isn't on its instance's workspace moved there (asked once per window and workspace,
/// `tried`: a window sway won't move isn't asked again at its every event), and shelf's window shown when it
/// is moved home (as the program starts). A workspace in front that empties (its last window closed or moved
/// down) stays, black: Home or the dock goes on from there. `top`: the main screen's output.
pub fn reconcile(windows: &[Window], top: Option<&str>, down: &[DownRule], land: &[Arc<str>], tried: &mut HashSet<(i64, String)>) -> Vec<String> {
    let mut out = Vec::new();
    let bottom = Instance::Bottom.name();
    for w in windows {
        // Moved to the bottom screen by us, or by a down rule as it opened: left there.
        if w.down || (w.workspace == bottom && down.iter().any(|r| r.matches(w))) {
            continue;
        }
        // Landed on the bottom screen by itself (a tap there focuses its output): to its home, or the
        // main screen.
        if w.workspace == bottom && w.home().is_none() {
            if tried.insert((w.id, String::new())) {
                out.push(commands::to_top(w.id, top));
            }
            continue;
        }
        // Known by its app id (a Steam game's, in Steam's scope): ClamShell places it, by that.
        if land.iter().any(|id| **id == *w.app_id) {
            continue;
        }
        let Some(home) = w.home().filter(|h| !h.on(&w.workspace)) else {
            continue;
        };
        let name = home.workspace();
        if !tried.insert((w.id, name.clone())) {
            continue;
        }
        let mut command = commands::move_windows(&[w.id], &name, top);
        if home == Instance::Shelf {
            command.push_str("; ");
            command.push_str(&commands::show(&name, top));
        }
        out.push(command);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn tree(json: &str) -> Node {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_tree_gives_windows_with_their_units() {
        let tree = tree(
            r#"{"type": "root", "nodes": [{"type": "output", "nodes": [{"type": "workspace", "name": "3",
                "nodes": [{"type": "con", "id": 7, "pid": 100, "app_id": "foot", "name": "~", "focused": true, "nodes": []}],
                "floating_nodes": [{"type": "floating_con", "id": 9, "pid": 200, "app_id": null,
                    "window_properties": {"class": "steam_app_620"}, "name": "Portal 2", "focused": false}]}]}]}"#,
        );
        let mut asked = Vec::new();
        let mut unit = |pid| {
            asked.push(pid);
            ((pid == 100).then(|| "clamshell-run-foot_1-1-0.scope".to_owned()), vec![pid, 1000])
        };
        let w = windows(&tree, &mut unit);
        assert_eq!(asked, vec![100, 200]);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].workspace.as_str(), w[1].workspace.as_str()), ("3", "3"));
        assert_eq!((w[0].id, w[0].app_id.as_str(), w[0].focused, w[0].unit.as_deref()), (7, "foot", true, Some("clamshell-run-foot_1-1-0.scope")));
        assert_eq!((w[1].app_id.as_str(), w[1].title.as_str(), w[1].unit.as_deref()), ("steam_app_620", "Portal 2", None));
        assert_eq!(w[1].pids, [200, 1000], "its process and its parents");
    }

    fn window(id: i64, app_id: &str, unit: Option<&str>, workspace: &str) -> Window {
        Window { id, app_id: app_id.into(), unit: unit.map(Into::into), workspace: workspace.into(), ..Window::default() }
    }

    #[test]
    fn a_window_known_by_its_app_id_is_left_to_clamshell() {
        let mut tried = HashSet::new();
        // A Steam game's, in the scope of Steam's entry: not moved to Steam's workspace.
        let game = window(7, "steam_app_620", Some("clamshell-run-desktop_steam-1-0.scope"), "clamshell-game");
        assert!(reconcile(std::slice::from_ref(&game), Some("DSI-2"), &[], &["steam_app_620".into()], &mut tried).is_empty());
        assert_eq!(reconcile(&[game], Some("DSI-2"), &[], &[], &mut tried).len(), 1, "unknown: by its scope");
    }

    #[test]
    fn windows_of_our_instances_go_home_once() {
        // (An entry's launches share its workspace.)
        let late = window(4, "late", Some("clamshell-run-spawner_1-1-0.scope"), "1");
        let home = window(5, "first", Some("clamshell-run-spawner_1-1-0.scope"), "clamshell-run-spawner_1");
        let second = window(7, "second", Some("clamshell-run-spawner_1-1-1.scope"), "clamshell-run-spawner_1");
        let stray = window(6, "foot", None, "1");
        let mut tried = HashSet::new();
        let asked = reconcile(&[late.clone(), home, second, stray], Some("DSI-2"), &[], &[], &mut tried);
        assert_eq!(asked, vec![r#"workspace "clamshell-run-spawner_1" output "DSI-2"; [con_id=4] move container to workspace "clamshell-run-spawner_1""#]);
        assert!(reconcile(&[late], Some("DSI-2"), &[], &[], &mut tried).is_empty(), "not asked again");
    }

    #[test]
    fn windows_moved_down_stay_and_strays_on_the_bottom_screen_go_up() {
        let bottom = Instance::Bottom.name();
        let ours = "clamshell-run-zelda_1-1-0.scope";
        let down = Window { down: true, ..window(4, "emu", Some(ours), &bottom) };
        let stray = window(5, "foot", None, &bottom);
        let mut tried = HashSet::new();
        let asked = reconcile(&[down, stray], Some("DSI-2"), &[], &[], &mut tried);
        assert_eq!(asked, vec![r#"[con_id=5] move container to output "DSI-2", focus"#], "the marked one left down");
    }

    #[test]
    fn a_window_a_down_rule_put_on_the_bottom_screen_stays_there() {
        let bottom = Instance::Bottom.name();
        let ours = "clamshell-run-zelda_1-1-0.scope";
        let placed = window(4, "emu", Some(ours), &bottom);
        let rule = DownRule { app_id: "emu".into(), title: None };
        let mut tried = HashSet::new();
        assert!(reconcile(std::slice::from_ref(&placed), Some("DSI-2"), &[rule], &[], &mut tried).is_empty(), "left for ClamShell to mark");
        assert_eq!(reconcile(&[placed], Some("DSI-2"), &[], &[], &mut tried).len(), 1, "without the rule: home");
    }

    #[test]
    fn the_tree_says_whether_a_window_is_down() {
        let tree = tree(
            r#"{"type": "root", "nodes": [{"type": "output", "name": "DSI-1", "nodes": [{"type": "workspace", "name": "clamshell-bottom",
                "nodes": [{"type": "con", "id": 7, "pid": 100, "app_id": "emu", "marks": ["clamshell-down-7"], "nodes": []}]}]}]}"#,
        );
        let w = windows(&tree, &mut |_| (None, vec![]));
        assert!(w[0].down);
    }

    #[test]
    fn shelf_is_shown_when_its_window_goes_home() {
        let shelf = window(1, clamshell_core::windows::SHELF_APP_ID, None, "1");
        let mut tried = HashSet::new();
        let asked = reconcile(&[shelf], None, &[], &[], &mut tried);
        assert_eq!(asked, vec![r#"[con_id=1] move container to workspace "clamshell-shelf"; workspace "clamshell-shelf""#.to_owned()]);
    }
}
