//! libretro: box art and screenshots from its thumbnails, the developer and year from its database, the
//! game found by its file's checksum, else by name.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use clamshell_core::Meta;
use clamshell_library::Details;

use super::titles::{Name, best_match, developer};
use super::{Shared, Wanted};
use crate::os::text;

const LIBRETRO: &str = "https://thumbnails.libretro.com";

const DATABASE: &str = "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat";

impl Shared {
    pub(super) fn libretro(&self, w: &Wanted, system: &str) -> Meta {
        // By checksum: the game's name in the database, as its thumbnails are named.
        let database = self.database(system);
        let crc = w.file.as_deref().and_then(checksum);
        let known = crc.and_then(|c| database.names.get(&c));
        let by_name = || self.listing(system, "Named_Boxarts").and_then(|names| best_match(&w.title, &names).map(str::to_owned));
        let get = |c: Option<u32>, of: &HashMap<u32, String>| c.and_then(|c| of.get(&c)).cloned().unwrap_or_default();
        let details = Details { year: crc.and_then(|c| database.years.get(&c)).copied(), developer: get(crc, &database.developers), ..Details::default() };
        let mut meta = Meta { details, ..Meta::default() };
        // The checksum's name, else (none, or no pictures under it) the title's.
        let mut tried = None;
        for name in [known.map(|n| thumbnail_name(n)), None] {
            let Some(name) = name.or_else(|| by_name().filter(|n| tried.as_ref() != Some(n))) else {
                continue;
            };
            let url = |kind: &str| format!("{LIBRETRO}/{}/{kind}/{}.png", text::percent_encode(system), text::percent_encode(&name));
            meta.cover = (!w.art).then(|| self.image(w, "cover", &url("Named_Boxarts"))).flatten();
            meta.details.shot = self.image(w, "shot", &url("Named_Snaps"));
            if meta.cover.is_some() || meta.details.shot.is_some() {
                break;
            }
            tried = Some(name);
        }
        meta
    }

    /// libretro's database of `system`: its No-Intro list's names, and the developers and years, by
    /// checksum; fetched once (empty where it couldn't be).
    pub(super) fn database(&self, system: &str) -> Arc<Database> {
        let Ok(mut databases) = self.databases.lock() else {
            return Arc::default();
        };
        if let Some(d) = databases.get(system) {
            return d.clone();
        }
        let dat = |dir: &str| {
            let url = format!("{DATABASE}/{dir}/{}.dat", text::percent_encode(system));
            self.list(&format!("{dir} - {system}.dat"), &url).unwrap_or_default()
        };
        let mut d = Database::default();
        for (crc, fields) in dat_games(&dat("no-intro")) {
            if let Some(name) = fields.get("name") {
                d.names.insert(crc, name.clone());
            }
        }
        for (crc, fields) in dat_games(&dat("developer")) {
            if let Some(dev) = fields.get("developer").map(|d| developer(d)).filter(|d| !d.is_empty()) {
                d.developers.insert(crc, dev);
            }
        }
        for (crc, fields) in dat_games(&dat("releaseyear")) {
            if let Some(year) = fields.get("releaseyear").and_then(|y| y.parse().ok()) {
                d.years.insert(crc, year);
            }
        }
        log::info!("meta: libretro's database of {system}: {} games", d.names.len());
        let d = Arc::new(d);
        databases.insert(system.to_owned(), d.clone());
        d
    }

    /// The names (without `.png`) in a system's folder of a kind, fetched once.
    pub(super) fn listing(&self, system: &str, kind: &str) -> Option<Arc<Vec<Name>>> {
        let id = format!("{system}/{kind}");
        let mut listings = crate::os::lock(&self.listings);
        if let Some(l) = listings.get(&id) {
            return l.clone();
        }
        let page = self.list(&format!("{kind} - {system}.html"), &format!("{LIBRETRO}/{}/{kind}/", text::percent_encode(system)));
        let names = page.map(|p| Arc::new(hrefs(&p).filter_map(|h| h.strip_suffix(".png").map(Name::new)).collect::<Vec<_>>()));
        if names.is_none() {
            log::warn!("meta: libretro has no {id}");
        }
        listings.insert(id, names.clone());
        names
    }
}

/// A system's games in libretro's database, by their files' checksums.
#[derive(Debug, Default)]
pub(super) struct Database {
    names: HashMap<u32, String>,
    developers: HashMap<u32, String>,
    years: HashMap<u32, u16>,
}

/// A clrmamepro DAT's games: each file's checksum (`rom ( ... crc XXXXXXXX ... )`) with its game's
/// fields (`name "..."`, `developer "..."`, ...; the first of each).
pub(super) fn dat_games(text: &str) -> Vec<(u32, HashMap<String, String>)> {
    let mut out = Vec::new();
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut crcs: Vec<u32> = Vec::new();
    let mut flush = |fields: &mut HashMap<String, String>, crcs: &mut Vec<u32>| {
        for crc in crcs.drain(..) {
            out.push((crc, fields.clone()));
        }
        fields.clear();
    };
    for line in text.lines().map(str::trim) {
        if line.starts_with("game (") {
            flush(&mut fields, &mut crcs);
        } else if let Some(rom) = line.strip_prefix("rom (") {
            let mut words = rom.split_whitespace();
            while let Some(w) = words.next() {
                if w == "crc"
                    && let Some(crc) = words.next().and_then(|c| u32::from_str_radix(c, 16).ok())
                {
                    crcs.push(crc);
                }
            }
        } else if let Some((key, rest)) = line.split_once(' ')
            && let Some(value) = rest.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        {
            fields.entry(key.to_owned()).or_insert_with(|| value.to_owned());
        }
    }
    flush(&mut fields, &mut crcs);
    out
}

/// A game's name as libretro's thumbnails name it: the characters a file name can't have as `_`.
pub(super) fn thumbnail_name(name: &str) -> String {
    name.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect()
}

/// The largest file whose checksum is worked out, in bytes: a cartridge's size, not a disc's.
pub(super) const CHECKSUM_MAX: u64 = 64 << 20;

/// A game file's checksum (CRC32): a zip's largest file's, from the zip's directory; a file up to
/// [`CHECKSUM_MAX`].
pub(super) fn checksum(file: &Path) -> Option<u32> {
    if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        return zip_tail(file).and_then(|(tail, base)| zip_crc(&tail, base));
    }
    if std::fs::metadata(file).ok()?.len() > CHECKSUM_MAX || !file.is_file() {
        return None;
    }
    let crc = || -> std::io::Result<u32> {
        use std::io::Read;
        let mut hasher = crc32fast::Hasher::new();
        let mut file = std::fs::File::open(file)?;
        let mut buf = vec![0; 1 << 16];
        loop {
            match file.read(&mut buf)? {
                0 => return Ok(hasher.finalize()),
                n => hasher.update(buf.get(..n).unwrap_or_default()),
            }
        }
    };
    crc().map_err(|e| log::warn!("checksum of {}: {e}", file.display())).ok()
}

/// The end of a zip (its last MiB: the central directory of a few files is there), and where in the file
/// it starts.
pub(super) fn zip_tail(file: &Path) -> Option<(Vec<u8>, usize)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    let base = len.saturating_sub(1 << 20);
    f.seek(SeekFrom::Start(base)).ok()?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail).ok()?;
    Some((tail, usize::try_from(base).ok()?))
}

/// The CRC32 of a zip's largest file, from its central directory: `zip` the end of the file, from
/// `base` in it.
pub(super) fn zip_crc(zip: &[u8], base: usize) -> Option<u32> {
    let u16_at = |at: usize| zip.get(at..at + 2).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(u16::from_le_bytes);
    let u32_at = |at: usize| zip.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes);
    // The end of the central directory: its signature, within the last 64 KiB (after it, a comment).
    let from = zip.len().saturating_sub(65_557);
    let end = (from..zip.len().saturating_sub(21)).rev().find(|&i| u32_at(i) == Some(0x0605_4b50))?;
    let count = usize::from(u16_at(end + 10)?);
    let mut at = usize::try_from(u32_at(end + 16)?).ok()?.checked_sub(base)?;
    let mut best: Option<(u32, u32)> = None;
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4b50 {
            return None;
        }
        let (crc, size) = (u32_at(at + 16)?, u32_at(at + 24)?);
        if best.is_none_or(|(_, s)| size > s) {
            best = Some((crc, size));
        }
        at += 46 + usize::from(u16_at(at + 28)?) + usize::from(u16_at(at + 30)?) + usize::from(u16_at(at + 32)?);
    }
    best.map(|(crc, _)| crc)
}

/// The links of a directory listing's page, decoded.
pub(super) fn hrefs(page: &str) -> impl Iterator<Item = String> + '_ {
    page.split("href=\"").skip(1).filter_map(|rest| rest.split('"').next()).map(text::percent_decode)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::library::meta::titles::developer;

    #[test]
    fn checksums_and_databases() {
        // A zip of two files (stored), made by hand: its directory's checksum of the larger.
        let mut zip = Vec::new();
        let mut directory = Vec::new();
        for (name, data) in [("a.txt", &b"hi"[..]), ("game.sfc", &b"123456789"[..])] {
            let offset = zip.len() as u32;
            let crc = crc32fast::hash(data);
            let header = |sig: u32, extra: &[u8]| {
                let mut h = sig.to_le_bytes().to_vec();
                h.extend_from_slice(extra);
                h
            };
            let mut local = header(0x0403_4b50, &[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&[0, 0]);
            local.extend_from_slice(name.as_bytes());
            local.extend_from_slice(data);
            zip.extend_from_slice(&local);
            let mut central = header(0x0201_4b50, &[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
            directory.extend_from_slice(&central);
        }
        let start = zip.len() as u32;
        zip.extend_from_slice(&directory);
        zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]);
        zip.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        zip.extend_from_slice(&start.to_le_bytes());
        zip.extend_from_slice(&[0, 0]);
        assert_eq!(zip_crc(&zip, 0), Some(0xCBF4_3926));
        let tail = zip.get(10..).unwrap().to_vec();
        assert_eq!(zip_crc(&tail, 10), Some(0xCBF4_3926), "its end only");
        assert_eq!(zip_crc(b"not a zip", 0), None);

        let dat = "clrmamepro (\n\tname \"x\"\n)\n\ngame (\n\tname \"Legend of Zelda, The (USA)\"\n\trom ( name \"z.sfc\" size 1 crc 777E4BC0 md5 00 )\n)\n\ngame (\n\tcomment \"Ys (USA)\"\n\tdeveloper \"Falcom\"\n\trom ( crc 0000000A )\n)\n";
        let games = dat_games(dat);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].0, 0x777E_4BC0);
        assert_eq!(games[0].1.get("name").map(String::as_str), Some("Legend of Zelda, The (USA)"));
        assert_eq!(games[1].1.get("developer").map(String::as_str), Some("Falcom"));
        assert_eq!(thumbnail_name("Battletoads & Double Dragon (USA)"), "Battletoads _ Double Dragon (USA)");
        assert_eq!((developer("Nintendo ???"), developer("???")), ("Nintendo".to_owned(), String::new()));
    }
}
