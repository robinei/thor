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
    collect(tree, "", unit, &mut out);
    out
}

fn collect(node: &Node, workspace: &str, unit: &mut impl FnMut(u32) -> Option<String>, out: &mut Vec<Window>) {
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
        });
    }
    for child in node.children() {
        collect(child, workspace, unit, out);
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
    for w in windows {
        let Some(home) = w.home().filter(|h| !h.is(&w.workspace)) else {
            continue;
        };
        let name = home.name();
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
        Window { id, app_id: app_id.into(), title: String::new(), focused: false, unit: unit.map(Into::into), workspace: workspace.into() }
    }

    #[test]
    fn windows_of_our_instances_go_home_once() {
        let late = window(4, "late", Some("clamshell-app-spawner-1-0.scope"), "1");
        let home = window(5, "first", Some("clamshell-app-spawner-1-0.scope"), "clamshell-app-spawner-1-0");
        let stray = window(6, "foot", None, "1");
        let mut tried = HashSet::new();
        let asked = reconcile(&[late.clone(), home, stray], None, Some("DSI-2"), &mut tried);
        assert_eq!(asked, vec![r#"workspace "clamshell-app-spawner-1-0" output "DSI-2"; [con_id=4] move container to workspace "clamshell-app-spawner-1-0""#]);
        assert!(reconcile(&[late], None, Some("DSI-2"), &mut tried).is_empty(), "not asked again");
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
