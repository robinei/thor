//! Android's launcher keeps its authenticated ADB server private to this machine.
#![allow(clippy::unwrap_used)]
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn the_android_adb_server_binds_only_to_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str, body: &str| {
        let path = dir.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("waydroid", "printf 'Session: RUNNING\\nIP address: 192.0.2.1\\n'");
    script("busctl", "exit 0");
    script("ss", "echo 'LISTEN 0 1 127.0.0.1:5039 '");
    script("systemd-run", "printf '%s\\n' \"$@\" > \"$HOME/server-args\"");
    script("adb", "case \"$*\" in *'getprop sys.boot_completed') echo 1 ;; esac");
    let status = Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/android/clamshell-android"))
        .args(["--stop", "waydroid.example"])
        .env_clear()
        .env("HOME", dir.path())
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        fs::read_to_string(dir.path().join("server-args")).unwrap(),
        "--user\n--quiet\n--collect\n--unit=clamshell-adb\n-E\nANDROID_ADB_SERVER_PORT\nadb\n-L\ntcp:localhost:5039\nnodaemon\nserver\n"
    );
}
