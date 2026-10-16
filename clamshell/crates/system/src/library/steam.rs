//! Installed Steam games, from Steam's own files: the library folders (`libraryfolders.vdf`), each
//! game's manifest (`appmanifest_<id>.acf`), and its portrait art in Steam's cache. The files' syntax is
//! clamshell-library's (`steam`). Installing and browsing stay in Steam.

use std::fs;
use std::path::{Path, PathBuf};

use clamshell_library::steam::{self, parse_vdf};
use clamshell_library::{Game, Launch};

/// Steam's data folder under `home`, if Steam is installed.
pub fn default_root(home: &Path) -> Option<PathBuf> {
    [home.join(".local/share/Steam"), home.join(".steam/steam")].into_iter().find(|p| p.join("steamapps").is_dir())
}

/// The installed games of every Steam library folder.
pub fn games(root: &Path) -> Vec<Game> {
    let mut libraries = vec![root.to_path_buf()];
    if let Ok(text) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        for path in steam::library_folders(&parse_vdf(&text)) {
            if !libraries.contains(&path) {
                libraries.push(path);
            }
        }
    }
    let mut games = Vec::new();
    for lib in libraries {
        let Ok(entries) = fs::read_dir(lib.join("steamapps")) else {
            continue;
        };
        let mut manifests: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf")))
            .collect();
        manifests.sort();
        for m in manifests {
            let Ok(text) = fs::read_to_string(&m) else {
                continue;
            };
            if let Some((appid, name)) = steam::manifest(&parse_vdf(&text)) {
                let mut game = Game::new(&format!("steam:{appid}"), name);
                game.launch = Launch::Steam { appid };
                game.box_front = art(&root.join("appcache/librarycache"), appid);
                games.push(game);
            }
        }
    }
    games
}

/// The game's portrait art in Steam's cache: `<appid>/library_600x900.jpg` (older), or
/// `<appid>/<hash>/library_capsule.jpg` (newer); the landscape header if there is no portrait.
fn art(cache: &Path, appid: u32) -> Option<PathBuf> {
    let dir = cache.join(appid.to_string());
    let flat = [dir.join("library_600x900.jpg"), cache.join(format!("{appid}_library_600x900.jpg"))];
    if let Some(p) = flat.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    let mut subdirs: Vec<PathBuf> = fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subdirs.sort();
    ["library_capsule.jpg", "library_600x900.jpg", "library_header.jpg", "header.jpg"]
        .iter()
        .find_map(|name| subdirs.iter().map(|d| d.join(name)).find(|p| p.is_file()))
        .or_else(|| Some(dir.join("header.jpg")).filter(|p| p.is_file()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn games_of_every_library_folder_with_their_art() {
        let root = std::env::temp_dir().join(format!("clamshell-steam-{}", std::process::id()));
        let other = root.join("other");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("steamapps")).unwrap();
        fs::create_dir_all(other.join("steamapps")).unwrap();
        fs::create_dir_all(root.join("appcache/librarycache/20/abc")).unwrap();
        fs::write(root.join("appcache/librarycache/20/abc/library_capsule.jpg"), "").unwrap();
        let folders = format!("\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}", root.display(), other.display());
        fs::write(root.join("steamapps/libraryfolders.vdf"), folders).unwrap();
        let manifest = |id: u32, name: &str| format!("\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"{name}\" \"StateFlags\" \"4\" }}");
        fs::write(root.join("steamapps/appmanifest_10.acf"), manifest(10, "Ten")).unwrap();
        fs::write(other.join("steamapps/appmanifest_20.acf"), manifest(20, "Twenty")).unwrap();
        let games = games(&root);
        let found: Vec<(&str, bool)> = games.iter().map(|g| (&*g.id, g.box_front.is_some())).collect();
        assert_eq!(found, vec![("steam:10", false), ("steam:20", true)]);
        assert_eq!(games[1].launch, Launch::Steam { appid: 20 });
        let _ = fs::remove_dir_all(&root);
    }
}
