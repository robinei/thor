//! Favourites in Pegasus' `favorites.txt` (docs/pegasus-format.md in the GPD repo, 6): one game file path
//! per line, `#` comments; rewritten whole, in Pegasus' format, when one is toggled.

use std::collections::HashSet;
use std::path::Path;

use crate::pegasus::clean;

/// The favourite paths (relative lines are against the config dir).
pub fn read(config_dir: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(config_dir.join("favorites.txt")) else {
        return HashSet::new();
    };
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| if l.contains(':') && !l.starts_with('/') { l.to_owned() } else { clean(&config_dir.join(l)).to_string_lossy().into_owned() })
        .collect()
}

/// Rewrites the file with these paths, as Pegasus does.
pub fn write<'a>(config_dir: &Path, paths: impl IntoIterator<Item = &'a str>) -> std::io::Result<()> {
    let mut text = String::from("# List of favorites, one path per line\n");
    for p in paths {
        text.push_str(p);
        text.push('\n');
    }
    std::fs::write(config_dir.join("favorites.txt"), text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("clamshell-fav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, ["/g/pc/diablo.sh", "steam:40800"]).unwrap();
        let favs = read(&dir);
        assert!(favs.contains("/g/pc/diablo.sh") && favs.contains("steam:40800") && favs.len() == 2);
        assert!(std::fs::read_to_string(dir.join("favorites.txt")).unwrap().starts_with("# List of favorites, one path per line\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
