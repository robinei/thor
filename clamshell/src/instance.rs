//! One session owns ClamShell's runtime folder ([`clamshell_system::paths::Paths::own_runtime`]). Keep the
//! locked file open until that session ends.
use std::fs::{File, OpenOptions};
use std::path::Path;

pub(crate) struct Lock {
    _file: File,
}

impl Lock {
    /// Locks `own_runtime/session.lock`, `own_runtime` being ClamShell's runtime folder.
    pub fn acquire(own_runtime: Option<&Path>) -> Result<Self, String> {
        let directory = own_runtime.ok_or("session lock: XDG_RUNTIME_DIR is missing")?;
        std::fs::create_dir_all(directory).map_err(|error| format!("session lock: {}: {error}", directory.display()))?;
        let path = directory.join("session.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("session lock: {}: {error}", path.display()))?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => format!("another ClamShell session already uses {}", directory.display()),
            std::fs::TryLockError::Error(error) => format!("session lock: {}: {error}", path.display()),
        })?;
        // Do not unlink it: another process may already have opened this inode to try its lock.
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn another_session_is_refused_until_the_owner_exits() {
        let runtime = tempfile::tempdir().unwrap();
        let first = Lock::acquire(Some(runtime.path())).unwrap();
        let error = Lock::acquire(Some(runtime.path())).err().unwrap();
        assert!(error.contains("another ClamShell session already uses"));
        drop(first);
        assert!(runtime.path().join("session.lock").is_file(), "a stale file is harmless and keeps a single lock inode");
        assert!(Lock::acquire(Some(runtime.path())).is_ok());
    }

    #[test]
    fn separate_runtime_directories_are_independent() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let _first = Lock::acquire(Some(first.path())).unwrap();
        let _second = Lock::acquire(Some(second.path())).unwrap();
    }

    #[test]
    fn missing_or_unusable_runtime_is_an_error_without_a_fallback() {
        assert!(Lock::acquire(None).err().unwrap().contains("XDG_RUNTIME_DIR is missing"));
        let runtime = tempfile::tempdir().unwrap();
        let file = runtime.path().join("not-a-directory");
        std::fs::write(&file, b"").unwrap();
        assert!(Lock::acquire(Some(&file)).is_err());
    }
}
