//! The fonts. Noto Sans is built in. Fonts for characters it lacks are found through fontconfig
//! (`fc-match`) on a thread of their own.

use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

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

/// How long fontconfig may take to answer. Building its cache the first time takes a few seconds; more
/// than this, it is taken as stuck, stopped, and the character drawn without a font of its own.
const FC_MATCH_LIMIT: Duration = Duration::from_secs(10);

/// Asks fontconfig for a sans-serif font with `ch`, and reads it unless `loaded` says it was read already.
pub(super) fn find_font(ch: char, loaded: &mut dyn FnMut(&str) -> bool) -> Found {
    let pattern = format!("sans-serif:charset={:x}", u32::from(ch));
    let path = run_limited(Command::new("fc-match").args(["--format=%{file}", &pattern]), FC_MATCH_LIMIT)
        .inspect_err(|e| log::warn!("fc-match {pattern}: {e}"))
        .ok()
        .and_then(|out| String::from_utf8(out).inspect_err(|e| log::warn!("fc-match {pattern}: {e}")).ok())
        .filter(|p| !p.is_empty());
    let font = path.as_deref().filter(|p| !loaded(p)).and_then(|path| {
        let bytes = std::fs::read(path).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| log::warn!("font {path}: {e}")).ok()?;
        // Keep it only if it has the character; fontconfig's best match may lack it.
        (font.lookup_glyph_index(ch) != 0).then_some(font)
    });
    Found { ch, path, font }
}

/// Runs `command` for at most `limit` and gives its standard output if it succeeded. Past the limit it is
/// killed. The output is read once it has ended, so it has to fit in the pipe, as fc-match's path does.
fn run_limited(command: &mut Command, limit: Duration) -> std::io::Result<Vec<u8>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            if let Err(e) = child.kill().and_then(|()| child.wait().map(drop)) {
                log::warn!("stopping a program: {e}");
            }
            return Err(std::io::Error::other(format!("no answer in {} s, stopped", limit.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if !status.success() {
        return Err(std::io::Error::other(status.to_string()));
    }
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        std::io::Read::read_to_end(&mut stdout, &mut out)?;
    }
    Ok(out)
}

/// Starts the font finding thread. It takes characters from `requests` and passes the fonts found for
/// them to `done`. A panic finding one (the panic hook has logged it) answers that character with no font.
pub fn spawn_font_finder(requests: Receiver<char>, done: impl Fn(Found) -> bool + Send + 'static) -> std::io::Result<()> {
    // The fonts read, kept through a restart of the thread so that none is read twice.
    let mut read: Vec<String> = Vec::new();
    crate::threads::lasting("fonts", move || {
        for ch in requests.iter() {
            let found =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| find_font(ch, &mut |path| read.iter().any(|p| p == path)))).unwrap_or_else(|_| {
                    log::error!("fonts: finding a font for {ch:?} panicked; drawn without one");
                    Found { ch, path: None, font: None }
                });
            if found.font.is_some() {
                read.extend(found.path.clone());
            }
            if !done(found) {
                return;
            }
        }
    })
}

/// The UI's font, Noto Sans Regular, built in so that text looks the same everywhere. It is unmodified and
/// under the SIL Open Font License 1.1 (fonts/OFL.txt).
pub(super) const NOTO_SANS: &[u8] = include_bytes!("../../fonts/NotoSans-Regular.ttf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_past_its_limit_is_stopped() {
        let start = Instant::now();
        assert!(run_limited(Command::new("sleep").arg("5"), Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(run_limited(Command::new("echo").arg("hi"), Duration::from_secs(5)).ok().as_deref(), Some(&b"hi\n"[..]));
        assert!(run_limited(&mut Command::new("false"), Duration::from_secs(5)).is_err());
    }
}
