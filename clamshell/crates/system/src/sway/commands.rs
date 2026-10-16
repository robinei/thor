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

/// The tabs' look (ClamShell's palette: black, the bright text for the tab shown, grey for the rest).
const TAB_STYLE: &str = "font pango:Noto Sans 11; title_align center; titlebar_border_thickness 0; titlebar_padding 8 4; \
    client.focused #000000 #000000 #c8c8d0 #000000 #000000; \
    client.focused_inactive #000000 #000000 #c8c8d0 #000000 #000000; \
    client.unfocused #000000 #000000 #5a5a64 #000000 #000000; \
    client.urgent #000000 #000000 #e0a040 #000000 #000000";

/// The mark on a window we moved to the bottom screen, before its id (sway's marks are unique to a
/// window): reconciling leaves it there.
pub const DOWN_MARK: &str = "clamshell-down";

/// A window to the bottom screen's workspace, marked so it stays, in tabs with the others there, shown
/// there; then the main screen has the keyboard again.
pub fn down(window: i64, top: Option<&str>) -> String {
    let bottom = Instance::Bottom.name();
    format!(
        "[con_id={window}] mark --add {DOWN_MARK}-{window}; [con_id={window}] move container to workspace \"{bottom}\"; [con_id={window}] layout tabbed; {}",
        show_down(window, top)
    )
}

/// The bottom screen shows a window moved there (its tab focused); the main screen keeps the keyboard.
pub fn show_down(window: i64, top: Option<&str>) -> String {
    let back = top.map_or_else(String::new, |top| format!("; focus output \"{top}\""));
    format!("[con_id={window}] focus{back}")
}

/// A window moved down back to a workspace of the main screen (its instance's), or to the main screen,
/// unmarked, and focused.
pub fn up(window: i64, workspace: Option<&str>, top: Option<&str>) -> String {
    let to = match (workspace, top) {
        (Some(ws), _) => format!("{}[con_id={window}] move container to workspace \"{ws}\"", pin(ws, top)),
        (None, Some(top)) => format!("[con_id={window}] move container to output \"{top}\""),
        (None, None) => String::new(),
    };
    format!("[con_id={window}] unmark {DOWN_MARK}-{window}; {to}; [con_id={window}] focus")
}

/// A window that landed on the bottom screen by itself, to the main screen.
pub fn to_top(window: i64, top: Option<&str>) -> String {
    top.map_or_else(String::new, |top| format!("[con_id={window}] move container to output \"{top}\", focus"))
}

/// The bottom screen's output on or off.
pub fn power(bottom: &str, on: bool) -> String {
    format!("output \"{bottom}\" power {}", if on { "on" } else { "off" })
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
        // How the session behaves is ours, set here; the config file keeps to the machine (outputs,
        // inputs, starting ClamShell). No borders; focus never follows the pointer (the pad mouse crossing
        // to the second screen mustn't take the keyboard from a game), nor the pointer focus.
        "default_border none; default_floating_border none; focus_follows_mouse no; mouse_warping none".to_owned(),
        // A screen shows one window at a time: our workspaces' windows are tabs, the tab bar hidden over a
        // lone window (sway's `--i3` behaviour), in ClamShell's look: black, its font, the title centred,
        // the tab shown in the bright text and the others grey, no lines. (A rule per window:
        // `workspace_layout` is the config file's alone.)
        format!("hide_edge_borders --i3 smart; {TAB_STYLE}"),
        "for_window [workspace=\"^clamshell-\"] layout tabbed".to_owned(),
        format!("for_window [app_id=\"^{SHELF_APP_ID}$\"] move container to workspace \"{shelf}\""),
        format!("for_window [app_id=\"^{STEAM_APP_PREFIX}\"] move container to workspace \"{landing}\""),
        format!("for_window [class=\"^{STEAM_APP_PREFIX}\"] move container to workspace \"{landing}\""),
    ];
    if let Some(top) = top {
        rules.push(format!("{}{}", pin(&shelf, Some(top)), pin(&landing, Some(top)).trim_end_matches("; ")));
        // (A window landing there by itself goes to the main screen: see `windows::reconcile`, which knows
        // the windows moved there on purpose.)
        rules.push(format!("workspace \"{second}\" output \"{bottom}\"; focus output \"{bottom}\"; workspace \"{second}\"; focus output \"{top}\""));
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
        assert!(rules.iter().any(|r| r.starts_with(r#"workspace "clamshell-bottom" output "DSI-1""#)), "the bottom screen's workspace there");
        assert!(!rules.iter().any(|r| r.contains("for_window [workspace=\"^clamshell-bottom")), "no rule sends windows off it (reconciling does)");
        assert!(rules.iter().any(|r| r.contains("focus_follows_mouse no")), "the session's behaviour set by us");
        assert!(
            rules.iter().any(|r| r.contains("client.focused #000000") && r.contains("hide_edge_borders --i3 smart")),
            "the tabs in ClamShell's look, none over a lone window"
        );
        assert!(rules.iter().any(|r| r == r#"for_window [workspace="^clamshell-"] layout tabbed"#), "one window at a time");
    }

    #[test]
    fn windows_go_down_marked_and_back_up_unmarked_the_main_screen_keeping_the_keyboard() {
        assert_eq!(
            down(7, Some("DSI-2")),
            r#"[con_id=7] mark --add clamshell-down-7; [con_id=7] move container to workspace "clamshell-bottom"; [con_id=7] layout tabbed; [con_id=7] focus; focus output "DSI-2""#
        );
        assert_eq!(show_down(7, Some("DSI-2")), r#"[con_id=7] focus; focus output "DSI-2""#);
        assert_eq!(
            up(7, Some("clamshell-game-1-0"), Some("DSI-2")),
            r#"[con_id=7] unmark clamshell-down-7; workspace "clamshell-game-1-0" output "DSI-2"; [con_id=7] move container to workspace "clamshell-game-1-0"; [con_id=7] focus"#
        );
        assert_eq!(up(7, None, Some("DSI-2")), r#"[con_id=7] unmark clamshell-down-7; [con_id=7] move container to output "DSI-2"; [con_id=7] focus"#);
        assert_eq!(power("DSI-1", false), r#"output "DSI-1" power off"#);
    }

    #[test]
    fn our_workspaces_are_shown_on_the_main_screen() {
        assert_eq!(show("w", Some("DSI-2")), r#"workspace "w" output "DSI-2"; focus output "DSI-2"; workspace "w""#);
        assert_eq!(show("w", None), r#"workspace "w""#);
        assert_eq!(move_windows(&[1, 2], "w", None), r#"[con_id=1] move container to workspace "w"; [con_id=2] move container to workspace "w""#);
    }
}
