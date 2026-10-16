//! Pegasus Frontend's library format (docs/pegasus-format.md in the GPD repo), the parts that need no
//! disk: which files are metadata files, file names as titles, the values of `players` and `release`,
//! and which keys mean what (Pegasus accepts several spellings of most). Reading the files and the game
//! dirs is clamshell-system's (`library::pegasus`).

use std::path::{Component, Path, PathBuf};

/// The path with `.` and `..` resolved lexically (no symlinks), as Qt's `cleanPath`.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether a file of a game dir is a metadata file, by its name.
pub fn is_metafile(name: &str) -> bool {
    matches!(name, "metadata.pegasus.txt" | "metadata.txt") || name.ends_with(".metadata.pegasus.txt") || name.ends_with(".metadata.txt")
}

/// Whether a path is hidden (its name starts with a dot).
pub fn hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

/// Everything before the last dot (Qt's completeBaseName).
pub fn base_name(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name.rfind('.') {
        Some(i) if i > 0 => name.get(..i).unwrap_or(name).to_owned(),
        _ => name.to_owned(),
    }
}

/// A file's name as a title: base name with `_` and `.` as spaces.
pub fn pretty_name(path: &Path) -> String {
    base_name(path).replace(['_', '.'], " ")
}

/// A file's extension, lowercased (what a collection's `extensions` are compared with).
pub fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

/// Whether a media file's base name makes it box art (`media/<game>/boxFront.png`).
pub fn is_box_front_name(name: &str) -> bool {
    matches!(name, "boxfront" | "boxFront" | "box_front" | "boxart2D" | "boxart2d")
}

/// Whether a media file is an image we can show.
pub fn is_image(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("png" | "jpg" | "webp" | "apng"))
}

/// `1-4` gives 4; `2` gives 2; anything else: none.
pub fn parse_players(v: &str) -> Option<u8> {
    let mut parts = v.splitn(2, '-');
    let a: u8 = parts.next()?.parse().ok()?;
    let b: u8 = match parts.next() {
        Some(b) => b.parse().ok()?,
        None => a,
    };
    Some(a.max(b).max(1))
}

/// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
pub fn parse_release(v: &str) -> Option<(u16, u8, u8)> {
    let mut parts = v.split('-');
    let y: u16 = parts.next().filter(|p| p.len() == 4)?.parse().ok()?;
    let m: u8 = parts.next().map_or(Some(1), |p| p.parse().ok())?;
    let d: u8 = parts.next().map_or(Some(1), |p| p.parse().ok())?;
    Some((y.max(1), m.clamp(1, 12), d.clamp(1, 31)))
}

/// What a key of a `game:` entry sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameKey {
    Files,
    Launch,
    Workdir,
    Developers,
    Publishers,
    Genres,
    Players,
    Release,
    Summary,
    Description,
    SortBy,
    /// `x-shelf-kind: tools`: a tool, not a game.
    Kind,
    BoxFront,
}

/// The keys of a `game:` entry and their spellings (keys are lowercased when read).
const GAME_KEYS: &[(&str, GameKey)] = &[
    ("file", GameKey::Files),
    ("files", GameKey::Files),
    ("launch", GameKey::Launch),
    ("command", GameKey::Launch),
    ("workdir", GameKey::Workdir),
    ("cwd", GameKey::Workdir),
    ("developer", GameKey::Developers),
    ("developers", GameKey::Developers),
    ("publisher", GameKey::Publishers),
    ("publishers", GameKey::Publishers),
    ("genre", GameKey::Genres),
    ("genres", GameKey::Genres),
    ("players", GameKey::Players),
    ("release", GameKey::Release),
    ("summary", GameKey::Summary),
    ("description", GameKey::Description),
    ("sorttitle", GameKey::SortBy),
    ("sortname", GameKey::SortBy),
    ("sort_title", GameKey::SortBy),
    ("sort_name", GameKey::SortBy),
    ("sort-title", GameKey::SortBy),
    ("sort-name", GameKey::SortBy),
    ("sortby", GameKey::SortBy),
    ("sort_by", GameKey::SortBy),
    ("sort-by", GameKey::SortBy),
    ("x-shelf-kind", GameKey::Kind),
    ("assets.boxfront", GameKey::BoxFront),
    ("assets.box_front", GameKey::BoxFront),
    ("assets.boxart2d", GameKey::BoxFront),
    ("asset.boxfront", GameKey::BoxFront),
    ("asset.box_front", GameKey::BoxFront),
    ("asset.boxart2d", GameKey::BoxFront),
];

impl GameKey {
    pub fn parse(key: &str) -> Option<GameKey> {
        GAME_KEYS.iter().find(|(k, _)| *k == key).map(|(_, g)| *g)
    }
}

/// What a key of a `collection:` entry sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKey {
    ShortName,
    Launch,
    Workdir,
    Extensions,
    Summary,
    SortBy,
    /// `x-shelf-kind: tools`: Utilities, tools rather than games.
    Kind,
}

/// The keys of a `collection:` entry and their spellings.
const COLLECTION_KEYS: &[(&str, CollectionKey)] = &[
    ("shortname", CollectionKey::ShortName),
    ("launch", CollectionKey::Launch),
    ("command", CollectionKey::Launch),
    ("workdir", CollectionKey::Workdir),
    ("cwd", CollectionKey::Workdir),
    ("extension", CollectionKey::Extensions),
    ("extensions", CollectionKey::Extensions),
    ("summary", CollectionKey::Summary),
    ("sortby", CollectionKey::SortBy),
    ("sort_by", CollectionKey::SortBy),
    ("sort-by", CollectionKey::SortBy),
    ("x-shelf-kind", CollectionKey::Kind),
];

impl CollectionKey {
    pub fn parse(key: &str) -> Option<CollectionKey> {
        COLLECTION_KEYS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c)
    }
}

/// The extensions of an `extensions:` value: comma-separated, lowercased.
pub fn extensions(value: &str) -> impl Iterator<Item = String> + '_ {
    value.split(',').map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_and_release() {
        assert_eq!(parse_players("1-4"), Some(4));
        assert_eq!(parse_players("2"), Some(2));
        assert_eq!(parse_players("two"), None);
        assert_eq!(parse_release("1996-12-31"), Some((1996, 12, 31)));
        assert_eq!(parse_release("1997"), Some((1997, 1, 1)));
        assert_eq!(parse_release("97"), None);
    }

    #[test]
    fn clean_paths() {
        assert_eq!(clean(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn keys_by_any_spelling() {
        assert_eq!(GameKey::parse("sort-title"), Some(GameKey::SortBy));
        assert_eq!(GameKey::parse("cwd"), Some(GameKey::Workdir));
        assert_eq!(GameKey::parse("asset.boxart2d"), Some(GameKey::BoxFront));
        assert_eq!(GameKey::parse("assets.logo"), None);
        assert_eq!(CollectionKey::parse("extension"), Some(CollectionKey::Extensions));
        assert_eq!(CollectionKey::parse("sorttitle"), None, "games only");
        assert_eq!(extensions("smc, SFC,,").collect::<Vec<_>>(), vec!["smc", "sfc"]);
    }

    #[test]
    fn names() {
        assert_eq!(base_name(Path::new("/g/a.b.iso")), "a.b");
        assert_eq!(base_name(Path::new("/g/.hidden")), ".hidden");
        assert_eq!(pretty_name(Path::new("/g/Super_Metroid.sfc")), "Super Metroid");
        assert!(is_metafile("snes.metadata.pegasus.txt") && !is_metafile("notes.txt"));
    }
}
