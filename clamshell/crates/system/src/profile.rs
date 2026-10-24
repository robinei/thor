//! Startup identification. Device-specific matching stays in the profile crate.
use clamshell_device::Profile;
use std::{fs, io, path::Path};

/// Read the kernel's identifiers, with an optional explicit profile for tests or development.
pub fn detect(root: &Path, forced: Option<&str>) -> Result<Profile, String> {
    let read = |relative: &str| {
        let path = root.join(relative);
        match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::warn!("device identity: {}: {error}", path.display());
                }
                Vec::new()
            }
        }
    };
    let compatible = read("sys/firmware/devicetree/base/compatible");
    let vendor = read("sys/class/dmi/id/sys_vendor");
    let product = read("sys/class/dmi/id/product_name");
    Profile::identify(forced, &compatible, &String::from_utf8_lossy(&vendor), &String::from_utf8_lossy(&product))
        .map_err(|name| format!("unknown CLAMSHELL_DEVICE {name:?}; use thor, vm or generic"))
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn kernel_identity_is_read_from_the_supplied_root_and_unknown_overrides_fail() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect(root.path(), None).unwrap(), Profile::generic());
        let dt = root.path().join("sys/firmware/devicetree/base");
        fs::create_dir_all(&dt).unwrap();
        fs::write(dt.join("compatible"), b"ayn,thor\0qcom,sm8550\0").unwrap();
        assert_eq!(detect(root.path(), None).unwrap(), Profile::thor());
        assert_eq!(detect(root.path(), Some("vm")).unwrap(), Profile::vm());
        assert!(detect(root.path(), Some("not-a-device")).is_err());
    }
}
