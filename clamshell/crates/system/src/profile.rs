//! Startup identification. Device-specific matching stays in the profile crate.
use clamshell_device::DeviceProfile;
use std::path::Path;

/// The machine's profile (see [`DeviceProfile::detect`]). An identifying file that couldn't be read is
/// warned of, and read as empty: ClamShell runs on as what is left to tell.
pub fn detect(root: &Path, forced: Option<&str>) -> Result<DeviceProfile, String> {
    let (profile, unread) = DeviceProfile::detect(root, forced)?;
    for problem in unread {
        log::warn!("device identity: {problem}");
    }
    Ok(profile)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::fs;
    #[test]
    fn kernel_identity_is_read_from_the_supplied_root_and_unknown_overrides_fail() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect(root.path(), None).unwrap(), DeviceProfile::generic());
        let dt = root.path().join("sys/firmware/devicetree/base");
        fs::create_dir_all(&dt).unwrap();
        fs::write(dt.join("compatible"), b"ayn,thor\0qcom,sm8550\0").unwrap();
        assert_eq!(detect(root.path(), None).unwrap(), DeviceProfile::thor());
        assert_eq!(detect(root.path(), Some("vm")).unwrap(), DeviceProfile::vm());
        assert!(detect(root.path(), Some("not-a-device")).is_err());
    }
}
