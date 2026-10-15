//! sway's windows, through its IPC socket (`SWAYSOCK`; the i3 protocol: "i3-ipc", length, type, JSON).
//!
//! - Launches go through sway's `exec`, aimed at their workspace: sway places the launch's first window
//!   there itself, before it is shown, even after we have switched back (the workspace isn't shown first).
//! - sway's own rules, set when we connect: shelf's window and Steam's games (`steam_app_…`) go to their
//!   workspaces, those are kept on the main screen, and a window that opens on the second screen's
//!   workspace (behind the launcher) goes to the main screen.
//! - Later windows of a launch (sway only places the first) are moved to their instance's workspace by
//!   the systemd scope of their process: the kernel's record of which launch they come from.
//! - The window list after every window event; an emptied workspace of ours in front shows shelf.
//!
//! Without sway there is no window list (other compositors offer no common way).

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use clamshell_core::Event;
use clamshell_core::windows::{GAME_WORKSPACE, SHELF_WORKSPACE, Window};

use crate::Sink;

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const SUBSCRIBE: u32 = 2;
const GET_OUTPUTS: u32 = 3;
const GET_TREE: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show (and create, if new) a workspace.
    Workspace(String),
    /// Run a job (see [`crate::job`]) for this workspace, without showing it.
    Launch { workspace: String, job: PathBuf },
    /// Ask a window to close (as its close button would).
    Close(i64),
    /// Move windows to a workspace (on the main screen), without showing it.
    Move { windows: Vec<i64>, workspace: String },
}

fn send(stream: &mut UnixStream, kind: u32, payload: &str) -> io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(io::Error::other)?;
    let mut message = Vec::with_capacity(14 + payload.len());
    message.extend_from_slice(MAGIC);
    message.extend_from_slice(&len.to_le_bytes());
    message.extend_from_slice(&kind.to_le_bytes());
    message.extend_from_slice(payload.as_bytes());
    stream.write_all(&message)
}

/// One message: its type (events have the high bit set) and payload.
fn receive(stream: &mut UnixStream) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 14];
    stream.read_exact(&mut header)?;
    let field = |at: usize| header.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let (Some(len), Some(kind)) = (field(6), field(10)) else {
        return Err(io::Error::other("short header"));
    };
    if header.get(..6) != Some(MAGIC.as_slice()) {
        return Err(io::Error::other("not an i3-ipc message"));
    }
    let mut payload = vec![0; len as usize];
    stream.read_exact(&mut payload)?;
    Ok((kind, payload))
}

/// A request and its reply.
fn ask(stream: &mut UnixStream, kind: u32, payload: &str) -> io::Result<Vec<u8>> {
    send(stream, kind, payload)?;
    loop {
        let (reply, body) = receive(stream)?;
        if reply == kind {
            return Ok(body);
        }
    }
}

/// The windows in a tree (`get_tree`'s JSON): leaf containers with a process. `unit` reads a pid's
/// systemd unit.
pub fn windows(tree: &Value, unit: &impl Fn(u32) -> Option<String>) -> Vec<Window> {
    let mut out = Vec::new();
    collect(tree, "", unit, &mut out);
    out
}

fn collect(node: &Value, workspace: &str, unit: &impl Fn(u32) -> Option<String>, out: &mut Vec<Window>) {
    let workspace = match node.get("type").and_then(Value::as_str) {
        Some("workspace") => node.get("name").and_then(Value::as_str).unwrap_or(""),
        _ => workspace,
    };
    let pid = node.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok());
    let kind = node.get("type").and_then(Value::as_str);
    if let (Some(pid), Some("con" | "floating_con")) = (pid, kind) {
        // Wayland windows have an app id; X11 ones a class.
        let app_id = node
            .get("app_id")
            .and_then(Value::as_str)
            .or_else(|| node.pointer("/window_properties/class").and_then(Value::as_str))
            .unwrap_or("");
        out.push(Window {
            id: node.get("id").and_then(Value::as_i64).unwrap_or(0),
            app_id: app_id.to_owned(),
            title: node.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
            focused: node.get("focused").and_then(Value::as_bool).unwrap_or(false),
            unit: unit(pid),
            workspace: workspace.to_owned(),
        });
    }
    for key in ["nodes", "floating_nodes"] {
        for child in node.get(key).and_then(Value::as_array).into_iter().flatten() {
            collect(child, workspace, unit, out);
        }
    }
}

/// The workspace in front if it is one of ours (an instance's, the game's) and empty: what was on it has
/// closed.
pub fn emptied(tree: &Value) -> Option<String> {
    fn find(node: &Value) -> Option<&Value> {
        if node.get("type").and_then(Value::as_str) == Some("workspace") {
            return Some(node).filter(|ws| has_focus(ws));
        }
        ["nodes", "floating_nodes"].iter().flat_map(|k| node.get(*k).and_then(Value::as_array).into_iter().flatten()).find_map(find)
    }
    fn has_focus(node: &Value) -> bool {
        node.get("focused").and_then(Value::as_bool) == Some(true)
            || ["nodes", "floating_nodes"].iter().flat_map(|k| node.get(*k).and_then(Value::as_array).into_iter().flatten()).any(has_focus)
    }
    let ws = find(tree)?;
    let empty = ["nodes", "floating_nodes"].iter().all(|k| ws.get(*k).and_then(Value::as_array).is_none_or(Vec::is_empty));
    let name = ws.get("name").and_then(Value::as_str)?;
    (empty && name.starts_with("clamshell-") && name != SHELF_WORKSPACE).then(|| name.to_owned())
}

/// The systemd unit a process runs in: the last part of its cgroup path (`….scope`, `….service`).
pub fn unit_of(pid: u32) -> Option<String> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    let last = path.rsplit('/').next()?;
    (last.ends_with(".scope") || last.ends_with(".service")).then(|| last.to_owned())
}

fn socket() -> Option<PathBuf> {
    std::env::var_os("SWAYSOCK").map(PathBuf::from)
}

/// The main screen's output: the first active one that isn't the second screen's.
fn top_output(stream: &mut UnixStream, bottom: &str) -> Option<String> {
    let outputs: Value = serde_json::from_slice(&ask(stream, GET_OUTPUTS, "").ok()?).ok()?;
    outputs.as_array()?.iter().filter(|o| o.get("active").and_then(Value::as_bool) == Some(true)).find_map(|o| {
        o.get("name").and_then(Value::as_str).filter(|n| *n != bottom).map(str::to_owned)
    })
}

/// Our workspaces live on the main screen: assigned there before they are created (else a tap on the
/// second screen, which focuses its output, would put them behind the launcher).
fn pin(name: &str, top: Option<&str>) -> String {
    top.map_or_else(String::new, |top| format!("workspace \"{name}\" output \"{top}\"; "))
}

/// Shows a workspace (creating it if new), on the main screen.
fn show(name: &str, top: Option<&str>) -> String {
    let focus = top.map_or_else(String::new, |top| format!("focus output \"{top}\"; "));
    format!("{}{focus}workspace \"{name}\"", pin(name, top))
}

/// The second screen's workspace: windows opening there go to the main screen.
const BOTTOM_WORKSPACE: &str = "clamshell-bottom";

/// Starts a job for a workspace (on the main screen) without showing it: shown for the `exec`, and
/// back at once, in one command (nothing is drawn in between); sway keeps the target by name.
fn launch(workspace: &str, job: &Path, exe: &Path, top: Option<&str>) -> io::Result<String> {
    let (exe, job) = (exe.to_string_lossy(), job.to_string_lossy());
    if exe.contains(['"', '\\']) || job.contains(['"', '\\']) {
        return Err(io::Error::other("a path sway can't quote"));
    }
    Ok(format!("{}workspace \"{workspace}\"; exec \"{exe}\" --run \"{job}\"; workspace back_and_forth", pin(workspace, top)))
}

/// sway's rules for our windows, given once when we connect (and again on reconnecting; sway keeps
/// them until it restarts). Then shelf's window, if already open, is put in its place and shown.
fn rules(top: Option<&str>, bottom: &str) -> Vec<String> {
    let mut rules = vec![
        format!("for_window [app_id=\"^{}$\"] move container to workspace \"{SHELF_WORKSPACE}\"", clamshell_core::apps::SHELF_APP_ID),
        format!("for_window [app_id=\"^steam_app_\"] move container to workspace \"{GAME_WORKSPACE}\""),
        format!("for_window [class=\"^steam_app_\"] move container to workspace \"{GAME_WORKSPACE}\""),
    ];
    if let Some(top) = top {
        rules.push(format!("{}{}", pin(SHELF_WORKSPACE, Some(top)), pin(GAME_WORKSPACE, Some(top)).trim_end_matches("; ")));
        rules.push(format!("workspace \"{BOTTOM_WORKSPACE}\" output \"{bottom}\"; focus output \"{bottom}\"; workspace \"{BOTTOM_WORKSPACE}\""));
        rules.push(format!("for_window [workspace=\"^{BOTTOM_WORKSPACE}$\"] move container to output \"{top}\", focus"));
    }
    rules.push(format!(
        "[app_id=\"^{}$\"] move container to workspace \"{SHELF_WORKSPACE}\"; {}",
        clamshell_core::apps::SHELF_APP_ID,
        show(SHELF_WORKSPACE, top)
    ));
    rules
}

/// Follows sway's windows (sending [`Event::Windows`] after each change) and carries out commands;
/// reconnects if sway goes away. Does nothing without sway. `bottom`: the second screen's output.
pub fn spawn(sink: impl Sink, commands: Receiver<Command>, bottom: String) -> io::Result<()> {
    let Some(path) = socket() else {
        log::info!("windows: not on sway, no window list");
        return Ok(());
    };
    let commands_path = path.clone();
    let bottom_output = bottom.clone();
    let exe = std::env::current_exe()?;
    thread::Builder::new().name("sway commands".into()).spawn(move || {
        for command in commands {
            let result = UnixStream::connect(&commands_path).and_then(|mut s| {
                let top = top_output(&mut s, &bottom_output);
                let text = match &command {
                    Command::Workspace(name) => show(name, top.as_deref()),
                    Command::Launch { workspace, job } => launch(workspace, job, &exe, top.as_deref())?,
                    Command::Close(id) => format!("[con_id={id}] kill"),
                    Command::Move { windows, workspace } => {
                        let moves: Vec<String> = windows.iter().map(|id| format!("[con_id={id}] move container to workspace \"{workspace}\"")).collect();
                        format!("{}{}", pin(workspace, top.as_deref()), moves.join("; "))
                    }
                };
                ask(&mut s, RUN_COMMAND, &text).map(|_| ())
            });
            if let Err(e) = result {
                log::warn!("sway: {command:?}: {e}");
            }
        }
    })?;
    thread::Builder::new().name("sway windows".into()).spawn(move || {
        loop {
            match follow(&path, &sink, &bottom) {
                Ok(()) => return,
                Err(e) => log::warn!("sway: {e}; trying again"),
            }
            thread::sleep(Duration::from_secs(2));
        }
    })?;
    Ok(())
}

/// Subscribes to window events and sends the window list after each; Ok when the UI is gone.
fn follow(path: &Path, sink: &impl Sink, bottom: &str) -> io::Result<()> {
    let mut events = UnixStream::connect(path)?;
    let mut queries = UnixStream::connect(path)?;
    ask(&mut events, SUBSCRIBE, r#"["window"]"#)?;
    let top = top_output(&mut queries, bottom);
    for rule in rules(top.as_deref(), bottom) {
        let reply = ask(&mut queries, RUN_COMMAND, &rule)?;
        if !String::from_utf8_lossy(&reply).contains("\"success\": true") && !String::from_utf8_lossy(&reply).contains("\"success\":true") {
            log::warn!("sway: {rule}: {}", String::from_utf8_lossy(&reply));
        }
    }
    // Each window's scope, by process (a process's scope never changes; emptied now and then, as
    // process ids are reused).
    let units: std::cell::RefCell<std::collections::HashMap<u32, Option<String>>> = std::cell::RefCell::default();
    // Moves asked for, so a window sway won't move isn't asked again (and again, by its events).
    let mut tried: std::collections::HashSet<(i64, String)> = std::collections::HashSet::new();
    loop {
        let tree: Value = serde_json::from_slice(&ask(&mut queries, GET_TREE, "")?).map_err(io::Error::other)?;
        if units.borrow().len() > 256 {
            units.borrow_mut().clear();
        }
        let list = windows(&tree, &|pid| units.borrow_mut().entry(pid).or_insert_with(|| unit_of(pid)).clone());
        let top = top_output(&mut queries, bottom);
        // Windows of our scopes go to their workspace; the move brings another event and a fresh list.
        let mut moved = false;
        for w in &list {
            if let Some(home) = w.home().filter(|h| *h != w.workspace && tried.insert((w.id, (*h).to_owned()))) {
                let mut command = format!("{}[con_id={}] move container to workspace \"{home}\"", pin(home, top.as_deref()), w.id);
                // Shelf's window, moved home when the program starts: show it.
                if home == SHELF_WORKSPACE {
                    command.push_str("; ");
                    command.push_str(&show(SHELF_WORKSPACE, top.as_deref()));
                }
                moved |= ask(&mut queries, RUN_COMMAND, &command).is_ok();
            }
        }
        // An app or the game closed and left its empty workspace in front: back to shelf.
        if let Some(name) = emptied(&tree) {
            log::debug!("sway: {name} emptied: showing shelf");
            moved |= ask(&mut queries, RUN_COMMAND, &show(SHELF_WORKSPACE, top.as_deref())).is_ok();
        }
        if !moved && !sink.send(Event::Windows(list)) {
            return Ok(());
        }
        // Wait for the next window event (and any that came with it).
        let (kind, _) = receive(&mut events)?;
        if kind & 0x8000_0000 == 0 {
            log::debug!("sway: unexpected message {kind}");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_gives_windows_with_their_units() {
        let tree: Value = serde_json::from_str(
            r#"{"type": "root", "nodes": [{"type": "output", "nodes": [{"type": "workspace", "name": "3",
                "nodes": [{"type": "con", "id": 7, "pid": 100, "app_id": "foot", "name": "~", "focused": true, "nodes": []}],
                "floating_nodes": [{"type": "floating_con", "id": 9, "pid": 200, "app_id": null,
                    "window_properties": {"class": "steam_app_620"}, "name": "Portal 2", "focused": false}]}]}]}"#,
        )
        .unwrap();
        let unit = |pid| (pid == 100).then(|| "clamshell-app-terminal-1-0.scope".to_owned());
        let w = windows(&tree, &unit);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].workspace.as_str(), w[1].workspace.as_str()), ("3", "3"));
        assert_eq!((w[0].id, w[0].app_id.as_str(), w[0].focused, w[0].unit.as_deref()), (7, "foot", true, Some("clamshell-app-terminal-1-0.scope")));
        assert_eq!((w[1].app_id.as_str(), w[1].title.as_str(), w[1].unit.as_deref()), ("steam_app_620", "Portal 2", None));
    }

    #[test]
    fn an_emptied_workspace_of_ours_in_front_is_noticed() {
        let tree = |name: &str, focused_ws: bool, windows: &str| -> Value {
            serde_json::from_str(&format!(
                r#"{{"type": "root", "nodes": [{{"type": "output", "nodes": [{{"type": "workspace", "name": "{name}", "focused": {focused_ws}, "nodes": [{windows}]}}]}}]}}"#
            ))
            .unwrap()
        };
        assert_eq!(emptied(&tree("clamshell-game", true, "")), Some("clamshell-game".into()));
        assert_eq!(emptied(&tree("clamshell-game", false, r#"{"type": "con", "focused": true}"#)), None, "not empty");
        assert_eq!(emptied(&tree("3", true, "")), None, "not ours");
        assert_eq!(emptied(&tree(SHELF_WORKSPACE, true, "")), None);
    }

    #[test]
    fn launches_go_through_sways_exec_for_their_workspace() {
        let text = launch("w", Path::new("/run/j.json"), Path::new("/usr/bin/clamshell"), Some("DSI-2")).unwrap();
        assert_eq!(text, r#"workspace "w" output "DSI-2"; workspace "w"; exec "/usr/bin/clamshell" --run "/run/j.json"; workspace back_and_forth"#);
        assert!(launch("w", Path::new("/a\"b"), Path::new("/x"), None).is_err());
        let rules = rules(Some("DSI-2"), "DSI-1");
        assert!(rules.iter().any(|r| r.contains("for_window [class=\"^steam_app_\"]")));
        assert!(rules.iter().any(|r| r.starts_with("for_window [workspace=\"^clamshell-bottom$\"] move container to output \"DSI-2\"")));
    }

    #[test]
    fn our_workspaces_are_shown_on_the_main_screen() {
        assert_eq!(show("w", Some("DSI-2")), r#"workspace "w" output "DSI-2"; focus output "DSI-2"; workspace "w""#);
        assert_eq!(show("w", None), r#"workspace "w""#);
    }

    #[test]
    fn messages_round_trip() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        send(&mut a, GET_TREE, "{}").unwrap();
        assert_eq!(receive(&mut b).unwrap(), (GET_TREE, b"{}".to_vec()));
    }

    #[test]
    fn this_process_has_a_unit() {
        // Under a desktop session every process is in some scope or service.
        if std::path::Path::new("/proc/self/cgroup").exists() {
            assert!(unit_of(std::process::id()).is_some_and(|u| u.ends_with(".scope") || u.ends_with(".service")));
        }
    }
}
