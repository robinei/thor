//! sway's tree as the state sees it (the windows, with the systemd unit of their process), and what to
//! ask sway after a window change. Pure: no IPC here.

use std::collections::HashSet;

use clamshell_core::instance::Instance;
use clamshell_core::windows::Window;

use super::commands;
use super::ipc::Node;

/// The windows in a tree: leaf containers with a process, with their workspace. `unit` gives a pid's
/// systemd unit.
pub fn windows(tree: &Node, unit: &mut impl FnMut(u32) -> Option<String>) -> Vec<Window> {
    let mut out = Vec::new();
    collect(tree, "", "", unit, &mut out);
    out
}

fn collect(node: &Node, output: &str, workspace: &str, unit: &mut impl FnMut(u32) -> Option<String>, out: &mut Vec<Window>) {
    let output = match node.kind.as_str() {
        "output" => node.name.as_deref().unwrap_or(""),
        _ => output,
    };
    let workspace = match node.kind.as_str() {
        "workspace" => node.name.as_deref().unwrap_or(""),
        _ => workspace,
    };
    if let (Some(pid), "con" | "floating_con") = (node.pid, node.kind.as_str()) {
        // Wayland windows have an app id; X11 ones a class.
        let app_id = node.app_id.as_deref().or_else(|| node.window_properties.as_ref().and_then(|p| p.class.as_deref())).unwrap_or("");
        out.push(Window {
            id: node.id,
            app_id: app_id.to_owned(),
            title: node.name.clone().unwrap_or_default(),
            focused: node.focused,
            unit: unit(pid),
            workspace: workspace.to_owned(),
            output: output.to_owned(),
            down: node.marks.iter().any(|m| m.starts_with(commands::DOWN_MARK)),
            visible: node.visible.unwrap_or(false),
        });
    }
    for child in node.children() {
        collect(child, output, workspace, unit, out);
    }
}

/// The workspace in front if it is one of ours (an instance's, the game's) and empty: what was on it has
/// closed.
pub fn emptied(tree: &Node) -> Option<&str> {
    fn has_focus(node: &Node) -> bool {
        node.focused || node.children().any(has_focus)
    }
    fn in_front(node: &Node) -> Option<&Node> {
        if node.kind == "workspace" {
            return Some(node).filter(|ws| has_focus(ws));
        }
        node.children().find_map(in_front)
    }
    let ws = in_front(tree)?;
    let name = ws.name.as_deref()?;
    (ws.children().next().is_none() && Instance::is_ours(name) && !Instance::Shelf.is(name)).then_some(name)
}

/// What to ask sway after a change of windows: each window of one of our instances that isn't on its
/// instance's workspace moved there (asked once per window and workspace, `tried`: a window sway won't
/// move isn't asked again at its every event), shelf's window shown when it is moved home (as the
/// program starts), and shelf shown when an emptied workspace of ours is in front. `top`: the main
/// screen's output.
pub fn reconcile(windows: &[Window], emptied: Option<&str>, top: Option<&str>, tried: &mut HashSet<(i64, String)>) -> Vec<String> {
    let mut out = Vec::new();
    let bottom = Instance::Bottom.name();
    for w in windows {
        // Moved to the bottom screen by us: left there.
        if w.down {
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
    if let Some(name) = emptied {
        log::debug!("sway: {name} emptied: showing shelf");
        out.push(commands::show(&Instance::Shelf.name(), top));
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
            (pid == 100).then(|| "clamshell-app-terminal-1-0.scope".to_owned())
        };
        let w = windows(&tree, &mut unit);
        assert_eq!(asked, vec![100, 200]);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].workspace.as_str(), w[1].workspace.as_str()), ("3", "3"));
        assert_eq!((w[0].id, w[0].app_id.as_str(), w[0].focused, w[0].unit.as_deref()), (7, "foot", true, Some("clamshell-app-terminal-1-0.scope")));
        assert_eq!((w[1].app_id.as_str(), w[1].title.as_str(), w[1].unit.as_deref()), ("steam_app_620", "Portal 2", None));
    }

    #[test]
    fn an_emptied_workspace_of_ours_in_front_is_noticed() {
        let tree = |name: &str, focused_ws: bool, windows: &str| -> Node {
            tree(&format!(
                r#"{{"type": "root", "nodes": [{{"type": "output", "nodes": [{{"type": "workspace", "name": "{name}", "focused": {focused_ws}, "nodes": [{windows}]}}]}}]}}"#
            ))
        };
        assert_eq!(emptied(&tree("clamshell-game", true, "")), Some("clamshell-game"));
        assert_eq!(emptied(&tree("clamshell-game", false, r#"{"type": "con", "focused": true}"#)), None, "not empty");
        assert_eq!(emptied(&tree("3", true, "")), None, "not ours");
        assert_eq!(emptied(&tree(&Instance::Shelf.name(), true, "")), None);
    }

    fn window(id: i64, app_id: &str, unit: Option<&str>, workspace: &str) -> Window {
        Window { id, app_id: app_id.into(), unit: unit.map(Into::into), workspace: workspace.into(), ..Window::default() }
    }

    #[test]
    fn windows_of_our_instances_go_home_once() {
        // (An app's instances share its workspace.)
        let late = window(4, "late", Some("clamshell-app-spawner-1-0.scope"), "1");
        let home = window(5, "first", Some("clamshell-app-spawner-1-0.scope"), "clamshell-app-spawner");
        let second = window(7, "second", Some("clamshell-app-spawner-1-1.scope"), "clamshell-app-spawner");
        let stray = window(6, "foot", None, "1");
        let mut tried = HashSet::new();
        let asked = reconcile(&[late.clone(), home, second, stray], None, Some("DSI-2"), &mut tried);
        assert_eq!(asked, vec![r#"workspace "clamshell-app-spawner" output "DSI-2"; [con_id=4] move container to workspace "clamshell-app-spawner""#]);
        assert!(reconcile(&[late], None, Some("DSI-2"), &mut tried).is_empty(), "not asked again");
    }

    #[test]
    fn windows_moved_down_stay_and_strays_on_the_bottom_screen_go_up() {
        let bottom = Instance::Bottom.name();
        let ours = "clamshell-game-1-0.scope";
        let down = Window { down: true, ..window(4, "emu", Some(ours), &bottom) };
        let stray = window(5, "foot", None, &bottom);
        let mut tried = HashSet::new();
        let asked = reconcile(&[down, stray], None, Some("DSI-2"), &mut tried);
        assert_eq!(asked, vec![r#"[con_id=5] move container to output "DSI-2", focus"#], "the marked one left down");
    }

    #[test]
    fn the_tree_says_which_output_a_window_is_on_and_whether_it_is_down() {
        let tree = tree(
            r#"{"type": "root", "nodes": [{"type": "output", "name": "DSI-1", "nodes": [{"type": "workspace", "name": "clamshell-bottom",
                "nodes": [{"type": "con", "id": 7, "pid": 100, "app_id": "emu", "marks": ["clamshell-down-7"], "nodes": []}]}]}]}"#,
        );
        let w = windows(&tree, &mut |_| None);
        assert_eq!((w[0].output.as_str(), w[0].down), ("DSI-1", true));
    }

    #[test]
    fn shelf_is_shown_when_its_window_goes_home_or_a_workspace_empties() {
        let shelf = window(1, clamshell_core::apps::SHELF_APP_ID, None, "1");
        let mut tried = HashSet::new();
        let asked = reconcile(&[shelf], Some("clamshell-game"), None, &mut tried);
        assert_eq!(
            asked,
            vec![
                r#"[con_id=1] move container to workspace "clamshell-shelf"; workspace "clamshell-shelf""#.to_owned(),
                r#"workspace "clamshell-shelf""#.to_owned(),
            ]
        );
    }
}
