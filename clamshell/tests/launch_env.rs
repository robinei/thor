//! The real Steam-hook entry point stays usable while a session holds the runtime lock.
#![allow(clippy::unwrap_used)]
use clamshell_state::{
    Save,
    settings::{Setting, Settings, VulkanDriver},
};
use clamshell_system::store::Store;

#[test]
fn launch_env_resolves_the_driver_and_preserves_the_env_override_under_a_session_lock() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data/clamshell");
    let config = root.path().join("config/clamshell");
    let runtime = root.path().join("runtime/clamshell");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(config.join("library.json"), "{\"places\":[]}").unwrap();
    let mut settings = Settings::default();
    settings.set(None, Setting::VulkanDriver, Some(VulkanDriver::Newest as u16));
    settings.set_env(None, "\"VK_DRIVER_FILES=/chosen/with spaces.json\"");
    Store::open(&data).save(&Save::Settings(settings)).unwrap();
    let lock = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(runtime.join("session.lock")).unwrap();
    lock.lock().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_clamshell"))
        .args(["--launch-env", "steam:42"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_RUNTIME_DIR", root.path().join("runtime"))
        .env("CLAMSHELL_DEVICE", "generic")
        .env("CLAMSHELL_STEAM", "off")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().filter_map(|line| line.strip_prefix("VK_DRIVER_FILES=")).next_back(), Some("/chosen/with spaces.json"));
}
