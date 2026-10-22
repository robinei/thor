//! The fonts. Noto Sans is built in. Fonts for characters it lacks are found through fontconfig
//! (`fc-match`) on a thread of their own.

use std::process::Command;
use std::sync::mpsc::Receiver;

pub(super) type Glyph = (fontdue::Metrics, Vec<u8>);

/// Which font draws a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Builtin,
    /// A font found for characters the built-in one lacks (`DrawCache`'s fallbacks).
    Fallback(usize),
    /// A font is still being looked for, so the character is not drawn yet.
    Waiting,
}

/// A font found for a character. `path` is fontconfig's choice, and `font` is the font read from it,
/// unless it was read before or lacks the character.
pub struct Found {
    pub ch: char,
    pub path: Option<String>,
    pub font: Option<fontdue::Font>,
}

/// Asks fontconfig for a sans-serif font with `ch`, and reads it unless `loaded` says it was read already.
pub(super) fn find_font(ch: char, loaded: &mut dyn FnMut(&str) -> bool) -> Found {
    let pattern = format!("sans-serif:charset={:x}", u32::from(ch));
    let path = Command::new("fc-match")
        .args(["--format=%{file}", &pattern])
        .output()
        .inspect_err(|e| log::warn!("fc-match {pattern}: {e}"))
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).inspect_err(|e| log::warn!("fc-match {pattern}: {e}")).ok())
        .filter(|p| !p.is_empty());
    let font = path.as_deref().filter(|p| !loaded(p)).and_then(|path| {
        let bytes = std::fs::read(path).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        // Keep it only if it has the character; fontconfig's best match may lack it.
        (font.lookup_glyph_index(ch) != 0).then_some(font)
    });
    Found { ch, path, font }
}

/// Starts the font finding thread. It takes characters from `requests` and passes the fonts found for
/// them to `done`.
pub fn spawn_font_finder(requests: Receiver<char>, done: impl Fn(Found) -> bool + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name("fonts".into()).spawn(move || {
        let mut read: Vec<String> = Vec::new();
        for ch in requests {
            let found = find_font(ch, &mut |path| read.iter().any(|p| p == path));
            if found.font.is_some() {
                read.extend(found.path.clone());
            }
            if !done(found) {
                return;
            }
        }
    })?;
    Ok(())
}

/// The UI's font, Noto Sans Regular, built in so that text looks the same everywhere. It is unmodified and
/// under the SIL Open Font License 1.1 (fonts/OFL.txt).
pub(super) const NOTO_SANS: &[u8] = include_bytes!("../../fonts/NotoSans-Regular.ttf");
