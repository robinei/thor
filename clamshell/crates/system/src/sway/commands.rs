//! The sway commands we send, as text. `top` is the main screen's output, where our workspaces live
//! (`None` until known: then nothing is pinned to an output).

use std::path::Path;

use clamshell_core::bottom_screen::DownRule;
use clamshell_core::instance::Instance;
use clamshell_core::windows::SHELF_APP_ID;

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

/// What a down rule's switch is set to while it is off: no output has the name, so its move fails and the
/// window stays where it opened.
const DOWN_OFF: &str = "clamshell-down-off";

/// A down rule's switch (a sway variable): by what it matches, so defining it again (sway restarted, or
/// ClamShell) reuses the same one.
fn down_switch(rule: &DownRule) -> String {
    let text = format!("{}\0{}", rule.app_id, rule.title.as_deref().unwrap_or("\0"));
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("$clamshell_down_{hash:016x}")
}

/// A pattern matching `text` exactly (PCRE's `\Q...\E`, in a sway string); none for text sway's quoting
/// can't carry (a backslash).
fn literal(text: &str) -> Option<String> {
    (!text.contains('\\')).then(|| format!("^\\Q{}\\E$", text.replace('"', "\\\"")))
}

/// A down rule defined in sway: its switch off, and the rule that moves a matching window, as it opens
/// (before it is drawn), to the output its switch names. None if it can't be said in sway's terms.
pub fn define_down_rule(rule: &DownRule) -> Option<String> {
    let app_id = literal(&rule.app_id)?;
    let title = match &rule.title {
        Some(t) => format!(" title=\"{}\"", literal(t)?),
        None => String::new(),
    };
    let switch = down_switch(rule);
    Some(format!("set {switch} {DOWN_OFF}; for_window [app_id=\"{app_id}\"{title}] move container to output ${switch}"))
}

/// A defined down rule switched on (its windows to the `bottom` output) or off.
pub fn switch_down_rule(rule: &DownRule, on: bool, bottom: &str) -> String {
    format!("set {} {}", down_switch(rule), if on { bottom } else { DOWN_OFF })
}

/// The landing rules for windows with app id `app_id` (as it is, or an X11 window's class): they open on
/// the landing workspace, from which ClamShell moves them to their entry's. None if sway's quoting can't
/// carry it.
pub fn define_land_rule(app_id: &str) -> Option<String> {
    let id = literal(app_id)?;
    let landing = Instance::Landing.name();
    Some(format!(
        "for_window [app_id=\"{id}\"] move container to workspace \"{landing}\"; for_window [class=\"{id}\"] move container to workspace \"{landing}\""
    ))
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
/// them until it restarts): what depends on the running machine (the outputs' names). How the session
/// behaves otherwise (borders, focus, tabs) is `clamshell/sway/clamshell.conf`, which every machine's
/// config includes. Then shelf's window, if already open, is put in its place and shown. `bottom`: the
/// second screen's output.
pub fn rules(top: Option<&str>, bottom: &str) -> Vec<String> {
    let (shelf, landing) = (Instance::Shelf.name(), Instance::Landing.name());
    // The second screen's workspace: windows opening there go to the main screen.
    let second = Instance::Bottom.name();
    let mut rules = vec![format!("for_window [app_id=\"^{SHELF_APP_ID}$\"] move container to workspace \"{shelf}\"")];
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
    fn down_rules_are_switched_by_a_variable_and_match_literally() {
        let rule = DownRule { app_id: "fo.t".into(), title: Some(r#"a "b""#.into()) };
        let define = define_down_rule(&rule).unwrap();
        let switch = down_switch(&rule);
        assert_eq!(
            define,
            format!(r#"set {switch} clamshell-down-off; for_window [app_id="^\Qfo.t\E$" title="^\Qa \"b\"\E$"] move container to output ${switch}"#)
        );
        assert_eq!(switch_down_rule(&rule, true, "DSI-1"), format!("set {switch} DSI-1"));
        assert_eq!(down_switch(&rule), down_switch(&rule.clone()), "the same rule, the same switch");
        assert!(define_down_rule(&DownRule { app_id: r"a\b".into(), title: None }).is_none(), "a backslash: none");
    }

    #[test]
    fn launches_go_through_sways_exec_for_their_workspace() {
        let text = launch("w", Path::new("/run/j.json"), Path::new("/usr/bin/clamshell"), Some("DSI-2")).unwrap();
        assert_eq!(text, r#"workspace "w" output "DSI-2"; workspace "w"; exec "/usr/bin/clamshell" --run "/run/j.json"; workspace back_and_forth"#);
        assert!(launch("w", Path::new("/a\"b"), Path::new("/x"), None).is_err());
        let rules = rules(Some("DSI-2"), "DSI-1");
        assert_eq!(
            define_land_rule("steam_app_620").unwrap(),
            r#"for_window [app_id="^\Qsteam_app_620\E$"] move container to workspace "clamshell-game"; for_window [class="^\Qsteam_app_620\E$"] move container to workspace "clamshell-game""#
        );
        assert!(rules.iter().any(|r| r.starts_with(r#"workspace "clamshell-bottom" output "DSI-1""#)), "the bottom screen's workspace there");
        assert!(!rules.iter().any(|r| r.contains("for_window [workspace=\"^clamshell-bottom")), "no rule sends windows off it (reconciling does)");
    }

    #[test]
    fn windows_go_down_marked_and_back_up_unmarked_the_main_screen_keeping_the_keyboard() {
        assert_eq!(
            down(7, Some("DSI-2")),
            r#"[con_id=7] mark --add clamshell-down-7; [con_id=7] move container to workspace "clamshell-bottom"; [con_id=7] layout tabbed; [con_id=7] focus; focus output "DSI-2""#
        );
        assert_eq!(show_down(7, Some("DSI-2")), r#"[con_id=7] focus; focus output "DSI-2""#);
        assert_eq!(
            up(7, Some("clamshell-run-zelda_1"), Some("DSI-2")),
            r#"[con_id=7] unmark clamshell-down-7; workspace "clamshell-run-zelda_1" output "DSI-2"; [con_id=7] move container to workspace "clamshell-run-zelda_1"; [con_id=7] focus"#
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
