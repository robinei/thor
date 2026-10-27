//! The calibration tool consumes facts from the actual binary.
#![allow(clippy::unwrap_used)]
use std::process::Command;
#[test]
fn panel_queries_follow_the_selected_profile() {
    let binary = env!("CARGO_BIN_EXE_clamshell");
    for (profile, top, bottom) in [("thor", "DSI-2", "DSI-1"), ("vm", "Virtual-1", "Virtual-2")] {
        let panel = Command::new(binary).args(["device", "panel"]).env("CLAMSHELL_DEVICE", profile).output().unwrap();
        assert!(panel.status.success());
        let panel: serde_json::Value = serde_json::from_slice(&panel.stdout).unwrap();
        assert_eq!(panel.get("top"), Some(&serde_json::json!(top)));
        assert_eq!(panel.get("bottom"), Some(&serde_json::json!(bottom)));
        let top_colour = panel.get("colour").unwrap().get("top").unwrap();
        assert_eq!(top_colour.get("link_bits_per_channel"), Some(&if profile == "thor" { serde_json::json!(8) } else { serde_json::Value::Null }));
        assert_eq!(top_colour.get("transfer"), Some(&serde_json::json!(if profile == "thor" { "gamma22" } else { "srgb" })));
        assert_eq!(top_colour.get("hdr_capable"), Some(&serde_json::json!(profile == "thor")));
        if profile == "thor" {
            let calibration = panel.get("calibration").unwrap();
            assert_eq!(calibration.get("step"), Some(&serde_json::json!(4)));
            assert_eq!(calibration.get("rates").unwrap().as_array().unwrap().len(), 2);
        } else {
            assert_eq!(panel.get("calibration"), Some(&serde_json::Value::Null));
        }
    }
}
