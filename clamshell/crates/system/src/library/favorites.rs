//! Favourites in `favorites.txt`: an entry id per line, `#` comments; rewritten whole when one is
//! toggled.

use std::collections::HashSet;
use std::path::Path;

/// The favourite ids.
pub fn read(state_dir: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(state_dir.join("favorites.txt")) else {
        return HashSet::new();
    };
    text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).map(str::to_owned).collect()
}

/// Rewrites the file with these ids.
pub fn write<'a>(state_dir: &Path, ids: impl IntoIterator<Item = &'a str>) -> std::io::Result<()> {
    let mut text = String::from("# ClamShell favourites, one id per line\n");
    for id in ids {
        text.push_str(id);
        text.push('\n');
    }
    crate::paths::write(&state_dir.join("favorites.txt"), &text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("clamshell-fav-{}", std::process::id()));
        write(&dir, ["snes:Zelda.sfc", "steam:40800"]).unwrap();
        let favs = read(&dir);
        assert!(favs.contains("snes:Zelda.sfc") && favs.contains("steam:40800") && favs.len() == 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
