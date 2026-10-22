//! [`TempDir`]: a folder for a test (the `test-support` feature).

use std::path::{Path, PathBuf};

/// A new, empty folder under the system's temp dir, removed when dropped (when the test fails too).
pub struct TempDir(PathBuf);

impl TempDir {
    /// Named after `name` and the process, so tests running at once don't share one. (A test's helper:
    /// failing to make the folder fails the test.)
    #[allow(clippy::panic)]
    pub fn new(name: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("clamshell-{name}-{}", std::process::id()));
        if let Err(e) = std::fs::create_dir_all(&path) {
            panic!("{}: {e}", path.display());
        }
        TempDir(path)
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
