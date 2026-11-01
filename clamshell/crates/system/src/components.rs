//! Installed graphics components, read on the system or launch thread, never while drawing: the
//! packages' versions for the Vulkan driver menu ([`read`], from pacman's database), and at each launch
//! only whether the newest driver's files are there ([`launch_facts`]).
use clamshell_state::settings::{LaunchFacts, Values, VulkanDriver};
use clamshell_state::status::Installed;
use std::path::Path;

/// turnip-devel's manifest. Its name has the architecture the driver is built for, which is aarch64 only
/// (an Adreno driver, `packages/turnip-devel`): elsewhere there is no such package and the file is never
/// there.
pub const TURNIP_ICD: &str = "/usr/lib/turnip-devel/share/vulkan/icd.d/freedreno_icd.aarch64.json";
const TURNIP_LIBRARY: &str = "/usr/lib/turnip-devel/libvulkan_freedreno.so";

fn field<'a>(description: &'a str, wanted: &str) -> Option<&'a str> {
    let mut lines = description.lines();
    while let Some(line) = lines.next() {
        if line == wanted {
            return lines.next().filter(|value| !value.is_empty());
        }
    }
    None
}

pub fn read(root: &Path) -> Installed {
    let directory = root.join("var/lib/pacman/local");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                log::debug!("graphics packages: {}: {error}", directory.display());
            } else {
                log::warn!("graphics packages: {}: {error}; versions unavailable", directory.display());
            }
            return Installed::default();
        }
    };
    let (mut mesa, mut system, mut turnip) = (None, None, None);
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!("graphics packages: {error}");
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !["mesa-", "vulkan-freedreno-", "turnip-devel-"].iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        let path = entry.path().join("desc");
        let description = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                log::warn!("graphics packages: {}: {error}", path.display());
                continue;
            }
        };
        let (Some(name), Some(version)) = (field(&description, "%NAME%"), field(&description, "%VERSION%")) else {
            log::warn!("graphics packages: {}: missing package name or version", path.display());
            continue;
        };
        match name {
            "mesa" => mesa = Some(version.into()),
            "vulkan-freedreno" => system = Some(version.into()),
            "turnip-devel" => turnip = Some(version.into()),
            _ => {}
        }
    }
    if turnip.is_some() && !turnip_files(root) {
        log::warn!("turnip-devel is registered but its driver or manifest is missing; the newer driver is unavailable");
        turnip = None;
    }
    Installed { mesa: system.or(mesa), turnip_devel: turnip }
}

/// Whether turnip-devel's manifest and library are both under `root`.
fn turnip_files(root: &Path) -> bool {
    [TURNIP_ICD, TURNIP_LIBRARY].iter().all(|file| root.join(file.trim_start_matches('/')).is_file())
}

/// The launch's facts. The driver is looked for again at each launch, by its files alone (pacman's
/// database is the menu's, [`read`]), so that removing its package can't leave a stale loader path.
pub fn launch_facts<'a>(values: &Values, fex: Option<&'a Path>, ignore: Option<&'a str>) -> LaunchFacts<'a> {
    launch_facts_at(Path::new("/"), values, fex, ignore)
}
pub(crate) fn launch_facts_at<'a>(root: &Path, values: &Values, fex: Option<&'a Path>, ignore: Option<&'a str>) -> LaunchFacts<'a> {
    let turnip_devel = if values.choice::<VulkanDriver>() == VulkanDriver::Newest {
        if turnip_files(root) {
            Some(Path::new(TURNIP_ICD))
        } else {
            log::warn!("turnip-devel's driver or manifest is missing; launching with the system Vulkan driver");
            None
        }
    } else {
        None
    };
    LaunchFacts { fex, ignore, turnip_devel }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn package(root: &Path, name: &str, version: &str) {
        let directory = root.join("var/lib/pacman/local").join(format!("{name}-{version}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("desc"), format!("%NAME%\n{name}\n\n%VERSION%\n{version}\n\n")).unwrap();
    }
    fn driver(root: &Path) {
        for file in [TURNIP_ICD, TURNIP_LIBRARY] {
            let path = root.join(file.trim_start_matches('/'));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture").unwrap();
        }
    }
    #[test]
    fn package_versions_are_exact_and_presence_requires_both_manifest_and_library() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read(root.path()), Installed::default());
        package(root.path(), "mesa", "26.2.6-1");
        package(root.path(), "vulkan-freedreno", "1:26.2.6-2");
        package(root.path(), "turnip-devel", "27.0.0_devel-1");
        assert_eq!(read(root.path()), Installed { mesa: Some("1:26.2.6-2".into()), turnip_devel: None });
        driver(root.path());
        assert_eq!(read(root.path()).turnip_devel.as_deref(), Some("27.0.0_devel-1"));
        std::fs::remove_file(root.path().join(TURNIP_LIBRARY.trim_start_matches('/'))).unwrap();
        assert_eq!(read(root.path()).turnip_devel, None);
    }
    #[test]
    fn package_removal_between_launches_uses_system_without_hiding_user_overrides() {
        use clamshell_state::settings::{Setting, launch_env};
        let root = tempfile::tempdir().unwrap();
        package(root.path(), "turnip-devel", "27.0-1");
        driver(root.path());
        let mut values = Values::default();
        values.set(Setting::VulkanDriver, VulkanDriver::Newest as u16);
        let mut env = Vec::new();
        launch_env(&values, launch_facts_at(root.path(), &values, None, None), &[], &mut env);
        assert_eq!(env, vec![("VK_DRIVER_FILES".to_owned(), TURNIP_ICD.to_owned())]);
        std::fs::remove_file(root.path().join(TURNIP_ICD.trim_start_matches('/'))).unwrap();
        env.clear();
        launch_env(&values, launch_facts_at(root.path(), &values, None, None), &[], &mut env);
        assert!(env.is_empty());
        let user = vec![("VK_DRIVER_FILES".to_owned(), "/chosen/driver.json".to_owned())];
        launch_env(&values, launch_facts_at(root.path(), &values, None, None), &user, &mut env);
        assert_eq!(env, user);
    }

    /// A launch looks only for the driver's files: pacman's database is read for the menu's versions,
    /// not at every launch.
    #[test]
    fn a_launch_finds_the_newest_driver_by_its_files_alone() {
        let root = tempfile::tempdir().unwrap();
        driver(root.path());
        let mut values = Values::default();
        values.set(clamshell_state::settings::Setting::VulkanDriver, VulkanDriver::Newest as u16);
        assert_eq!(launch_facts_at(root.path(), &values, None, None).turnip_devel, Some(Path::new(TURNIP_ICD)), "no package entry read");
        assert_eq!(read(root.path()).turnip_devel, None, "the menu's version is the package's");
    }
}
