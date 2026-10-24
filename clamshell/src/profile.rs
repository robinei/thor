//! Package scripts consume the same facts as the session, without repeating device identification.
use clamshell_device::{Profile, TouchSelector};

pub fn outputs(profile: &Profile) -> String {
    let mut out = format!(
        "output {} transform {} position 0 0\noutput {} transform {}\n",
        profile.top.connector,
        profile.top.rotation.degrees(),
        profile.bottom.connector,
        profile.bottom.rotation.degrees()
    );
    for touch in profile.touchscreens {
        let selector = match touch.selector {
            TouchSelector::Named(name) => format!("\"{name}\""),
            TouchSelector::All => "type:touch".into(),
        };
        out.push_str(&format!("input {selector} map_to_output {}\n", profile.screen(touch.screen).connector));
    }
    out
}
pub fn panel(profile: &Profile) -> serde_json::Value {
    serde_json::json!({ "top": profile.top.connector, "bottom": profile.bottom.connector })
}
