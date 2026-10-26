//! The XDG data directories and the icon themes in them.

use std::path::{Path, PathBuf};

/// The XDG data dirs, the user's first.
pub(crate) fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let user = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    user.into_iter().chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).collect()
}

/// The icon themes to look in, in order. hicolor is the fallback every app installs into.
const THEMES: [&str; 3] = ["hicolor", "Adwaita", "breeze"];
/// Bitmap sizes, largest first. Icons are drawn at about 120 pixels.
const SIZES: [u32; 8] = [512, 256, 192, 128, 96, 64, 48, 32];

/// An icon's image file. An absolute path is used as it is. Otherwise the largest image in the themes is
/// found, scalable ones first, and then the pixmaps folders are tried.
pub(crate) fn find_icon(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    for dir in dirs {
        for theme in THEMES {
            let base = dir.join("icons").join(theme);
            let mut candidates = vec![base.join("scalable/apps").join(format!("{name}.svg"))];
            for size in SIZES {
                for ext in ["png", "svg"] {
                    candidates.push(base.join(format!("{size}x{size}/apps/{name}.{ext}")));
                    // breeze's layout: apps/<size>/<name>.svg
                    candidates.push(base.join(format!("apps/{size}/{name}.{ext}")));
                }
            }
            if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
                return Some(found);
            }
        }
    }
    dirs.iter().flat_map(|d| ["svg", "png"].map(|ext| d.join("pixmaps").join(format!("{name}.{ext}")))).find(|c| c.is_file())
}
