//! Installed Steam games, from Steam's own files: the library folders (`libraryfolders.vdf`), each
//! game's manifest (`appmanifest_<id>.acf`), and its portrait art in Steam's cache. Steam's tools
//! (Proton, the Linux runtimes, the redistributables) are left out. Installing and browsing stay in
//! Steam.

use std::fs;
use std::path::{Path, PathBuf};

use crate::Game;

/// A VDF value: a string, or a block of key-value pairs.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Block(Vec<(String, Value)>),
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Block(items) => items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Value::Str(_) => None,
        }
    }

    fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    fn blocks(&self) -> impl Iterator<Item = &Value> {
        let items: &[(String, Value)] = match self {
            Value::Block(items) => items,
            Value::Str(_) => &[],
        };
        items.iter().map(|(_, v)| v).filter(|v| matches!(v, Value::Block(_)))
    }
}

/// Parses Valve's KeyValues text ("key" "value", "key" { ... }, `//` comments); the whole file as one
/// block.
fn parse_vdf(text: &str) -> Value {
    parse_block(&mut tokenize(text).into_iter().peekable())
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => {}
                        },
                        other => s.push(other),
                    }
                }
                tokens.push(Token::Str(s));
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    tokens
}

fn parse_block(tokens: &mut std::iter::Peekable<std::vec::IntoIter<Token>>) -> Value {
    let mut items = Vec::new();
    while let Some(t) = tokens.next() {
        match t {
            Token::Str(key) => match tokens.next() {
                Some(Token::Str(v)) => items.push((key, Value::Str(v))),
                Some(Token::Open) => items.push((key, parse_block(tokens))),
                _ => break,
            },
            Token::Close => break,
            Token::Open => {}
        }
    }
    Value::Block(items)
}

/// Steam's data folder, if Steam is installed.
pub fn default_root() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [home.join(".local/share/Steam"), home.join(".steam/steam")].into_iter().find(|p| p.join("steamapps").is_dir())
}

/// Steam's own tools, which aren't games.
fn is_tool(name: &str) -> bool {
    name.starts_with("Proton ") || name.starts_with("Steam Linux Runtime") || name.starts_with("Steamworks Common")
}

/// The installed games of every Steam library folder.
pub fn games(root: &Path) -> Vec<Game> {
    let mut libraries = vec![root.to_path_buf()];
    if let Ok(text) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        let vdf = parse_vdf(&text);
        if let Some(folders) = vdf.get("libraryfolders") {
            for folder in folders.blocks() {
                if let Some(path) = folder.str("path").map(PathBuf::from)
                    && !libraries.contains(&path)
                {
                    libraries.push(path);
                }
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
            if let Some(game) = from_manifest(&parse_vdf(&text), root) {
                games.push(game);
            }
        }
    }
    games
}

fn from_manifest(vdf: &Value, root: &Path) -> Option<Game> {
    let app = vdf.get("AppState")?;
    let appid: u32 = app.str("appid")?.parse().ok()?;
    let name = app.str("name")?;
    // StateFlags bit 4: fully installed.
    let installed = app.str("StateFlags").and_then(|f| f.parse::<u32>().ok()).is_some_and(|f| f & 4 != 0);
    if !installed || is_tool(name) {
        return None;
    }
    let mut game = Game::new(&format!("steam:{appid}"), name);
    game.launch = crate::Launch::Steam { appid };
    game.box_front = art(&root.join("appcache/librarycache"), appid);
    Some(game)
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

    const MANIFEST: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"InstalledDepots\"\n\t{\n\t\t\"1245621\"\n\t\t{\n\t\t\t\"size\"\t\t\"1\"\n\t\t}\n\t}\n}\n";

    #[test]
    fn reads_a_manifest() {
        let g = from_manifest(&parse_vdf(MANIFEST), Path::new("/nonexistent")).unwrap();
        assert_eq!((&*g.id, g.title.as_str()), ("steam:1245620", "ELDEN RING"));
        assert_eq!(g.launch, crate::Launch::Steam { appid: 1_245_620 });
    }

    #[test]
    fn tools_and_uninstalled_apps_are_left_out() {
        let tool = MANIFEST.replace("ELDEN RING", "Proton Experimental");
        assert!(from_manifest(&parse_vdf(&tool), Path::new("/")).is_none());
        let partial = MANIFEST.replace("\"StateFlags\"\t\t\"4\"", "\"StateFlags\"\t\t\"1026\"");
        assert!(from_manifest(&parse_vdf(&partial), Path::new("/")).is_none());
    }

    #[test]
    fn library_folders() {
        let vdf = parse_vdf("\"libraryfolders\" { \"0\" { \"path\" \"/a\" } \"1\" { \"path\" \"/b\" } } // end");
        let paths: Vec<&str> = vdf.get("libraryfolders").unwrap().blocks().filter_map(|b| b.str("path")).collect();
        assert_eq!(paths, vec!["/a", "/b"]);
    }
}
