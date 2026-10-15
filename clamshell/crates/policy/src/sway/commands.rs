//! The sway commands we send, as text. `top` is the main screen's output, where our workspaces live
//! (`None` until known: then nothing is pinned to an output).

use std::path::Path;

use clamshell_core::apps::SHELF_APP_ID;
use clamshell_core::instance::{Instance, STEAM_APP_PREFIX};

use super::ipc::Error;

/// Our workspaces live on the main screen: assigned there before they are created (else a tap on the
/// second screen, which focuses its output, would put them behind the launcher).
fn pin(name: &str, top: Option<&str>) -> String {
    top.map_or_else(String::new, |top| format!("workspace \"{name}\" output \"{top}\"; "))
}

/// Shows a workspace (creating it if new), on the main screen.
pub fn show(name: &str, top: Option<&str>) -> String {
    let focus = top.map_or_else(String::new, |top| format!("focus output \"{top}\"; "));
    format!("{}{focus}workspace \"{name}\"", pin(name, top))
}

/// Starts a job for a workspace (on the main screen) without showing it: shown for the `exec`, and
/// back at once, in one command (nothing is drawn in between); sway keeps the target by name.
pub fn launch(workspace: &str, job: &Path, exe: &Path, top: Option<&str>) -> Result<String, Error> {
    let (exe, job) = (exe.to_string_lossy(), job.to_string_lossy());
    if exe.contains(['"', '\\']) || job.contains(['"', '\\']) {
        return Err(Error::Unquotable);
    }
    Ok(format!("{}workspace \"{workspace}\"; exec \"{exe}\" --run \"{job}\"; workspace back_and_forth", pin(workspace, top)))
}

/// Moves windows to a workspace (on the main screen), without showing it.
pub fn move_windows(windows: &[i64], workspace: &str, top: Option<&str>) -> String {
    let moves: Vec<String> = windows.iter().map(|id| format!("[con_id={id}] move container to workspace \"{workspace}\"")).collect();
    format!("{}{}", pin(workspace, top), moves.join("; "))
}

/// Asks a window to close, as its close button would.
pub fn close(window: i64) -> String {
    format!("[con_id={window}] kill")
}

/// sway's rules for our windows, given once when we connect (and again on reconnecting; sway keeps
/// them until it restarts). Then shelf's window, if already open, is put in its place and shown.
/// `bottom`: the second screen's output.
pub fn rules(top: Option<&str>, bottom: &str) -> Vec<String> {
    let (shelf, landing) = (Instance::Shelf.name(), Instance::Landing.name());
    // The second screen's workspace: windows opening there go to the main screen.
    let second = Instance::Bottom.name();
    let mut rules = vec![
        format!("for_window [app_id=\"^{SHELF_APP_ID}$\"] move container to workspace \"{shelf}\""),
        format!("for_window [app_id=\"^{STEAM_APP_PREFIX}\"] move container to workspace \"{landing}\""),
        format!("for_window [class=\"^{STEAM_APP_PREFIX}\"] move container to workspace \"{landing}\""),
    ];
    if let Some(top) = top {
        rules.push(format!("{}{}", pin(&shelf, Some(top)), pin(&landing, Some(top)).trim_end_matches("; ")));
        rules.push(format!("workspace \"{second}\" output \"{bottom}\"; focus output \"{bottom}\"; workspace \"{second}\""));
        // (The rule's command in quotes: unquoted, sway would end the rule at the comma and run `focus`
        // once, now.)
        rules.push(format!("for_window [workspace=\"^{second}$\"] 'move container to output \"{top}\", focus'"));
    }
    rules.push(format!("[app_id=\"^{SHELF_APP_ID}$\"] move container to workspace \"{shelf}\"; {}", show(&shelf, top)));
    rules
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn launches_go_through_sways_exec_for_their_workspace() {
        let text = launch("w", Path::new("/run/j.json"), Path::new("/usr/bin/clamshell"), Some("DSI-2")).unwrap();
        assert_eq!(text, r#"workspace "w" output "DSI-2"; workspace "w"; exec "/usr/bin/clamshell" --run "/run/j.json"; workspace back_and_forth"#);
        assert!(launch("w", Path::new("/a\"b"), Path::new("/x"), None).is_err());
        let rules = rules(Some("DSI-2"), "DSI-1");
        assert!(rules.iter().any(|r| r.contains("for_window [class=\"^steam_app_\"]")));
        assert!(rules.iter().any(|r| r.starts_with("for_window [workspace=\"^clamshell-bottom$\"] 'move container to output \"DSI-2\", focus'")));
    }

    #[test]
    fn our_workspaces_are_shown_on_the_main_screen() {
        assert_eq!(show("w", Some("DSI-2")), r#"workspace "w" output "DSI-2"; focus output "DSI-2"; workspace "w""#);
        assert_eq!(show("w", None), r#"workspace "w""#);
        assert_eq!(move_windows(&[1, 2], "w", None), r#"[con_id=1] move container to workspace "w"; [con_id=2] move container to workspace "w""#);
    }
}
