//! The package script and calibration tool consume facts from the actual binary.
#![allow(clippy::unwrap_used)]
use std::{path::Path, process::Command};
#[test]
fn package_outputs_and_panel_queries_follow_the_selected_profile() {
    let binary = env!("CARGO_BIN_EXE_clamshell");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages/thor-session/root/usr/lib/thor/thor-outputs");
    for (profile, expected, top, bottom) in [
        (
            "thor",
            "output DSI-2 transform 90 position 0 0\noutput DSI-1 transform 90\ninput \"0:0:top_touchscreen\" map_to_output DSI-2\ninput \"0:0:bottom_touchscreen\" map_to_output DSI-1\n",
            "DSI-2",
            "DSI-1",
        ),
        ("vm", "output Virtual-1 transform 0 position 0 0\noutput Virtual-2 transform 0\ninput type:touch map_to_output Virtual-2\n", "Virtual-1", "Virtual-2"),
    ] {
        let runtime = tempfile::tempdir().unwrap();
        let queried = Command::new(binary).args(["device", "outputs"]).env("CLAMSHELL_DEVICE", profile).output().unwrap();
        assert!(queried.status.success());
        assert_eq!(String::from_utf8(queried.stdout).unwrap(), expected);
        let directory = Path::new(binary).parent().unwrap();
        let path = format!("{}:{}", directory.display(), std::env::var("PATH").unwrap_or_default());
        assert!(
            Command::new("sh")
                .arg(&script)
                .env("PATH", path)
                .env("CLAMSHELL_DEVICE", profile)
                .env("XDG_RUNTIME_DIR", runtime.path())
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(std::fs::read_to_string(runtime.path().join("thor-outputs.conf")).unwrap(), expected);
        assert!(!runtime.path().join("thor-outputs.env").exists());
        let panel = Command::new(binary).args(["device", "panel"]).env("CLAMSHELL_DEVICE", profile).output().unwrap();
        assert!(panel.status.success());
        let panel: serde_json::Value = serde_json::from_slice(&panel.stdout).unwrap();
        assert_eq!(panel.get("top"), Some(&serde_json::json!(top)));
        assert_eq!(panel.get("bottom"), Some(&serde_json::json!(bottom)));
        if profile == "thor" {
            let calibration = panel.get("calibration").unwrap();
            assert_eq!(calibration.get("step"), Some(&serde_json::json!(4)));
            assert_eq!(calibration.get("rates").unwrap().as_array().unwrap().len(), 2);
        } else {
            assert_eq!(panel.get("calibration"), Some(&serde_json::Value::Null));
        }
    }
}
