//! Fetching what the library's files don't say: covers, screenshots, what an entry is about, the year it
//! came out, who made it. After the library is read, the entries it wants (see [`Wanted`]) are fetched
//! on a few threads (curl), their images kept in the art cache (~/.cache/clamshell/art), what was found
//! saved in the database (also what wasn't: see [`crate::store`]) and sent to the state as it arrives.
//!
//! - A content dir's games (`"metadata": "libretro:<system>"` in library.json, the system as libretro
//!   names its folder): box art and a screenshot from libretro's thumbnails, by name: the entry's title
//!   as libretro names the game, or the same once case, bracketed tags (`(USA)`), "the" and punctuation
//!   are left out; of several, the USA release, then World, then Europe.
//! - A content dir's films or series (`"tmdb:movie"`, `"tmdb:tv"`): poster, backdrop, overview and year
//!   from TMDB, by title (a `(1980)` at its end the year); needs `keys.tmdb` in library.json.
//! - Steam's games: the store's description, developer, year and first screenshot (no key).
//! - The desktop files' games: a cover from SteamGridDB, by name; needs `keys.steamgriddb`.
//!
//! Art beside an entry's file comes first: what is fetched never replaces it.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use clamshell_core::{Event, Meta};
use serde::Deserialize;
use serde_json::Value;

use crate::store::Store;
use crate::{Sink, worker};

/// Fetches at once.
const THREADS: usize = 4;
/// A miss (nothing found) is tried again after this long (seconds).
pub const RETRY_MISS: u64 = 30 * 86_400;

/// The services' keys, from library.json's `keys`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    #[serde(default)]
    pub tmdb: Option<String>,
    #[serde(default)]
    pub steamgriddb: Option<String>,
}

/// Where an entry's metadata comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// libretro's thumbnails of this system (its folder's name there).
    Libretro(String),
    /// TMDB, a film (`movie`) or a series (`tv`).
    Tmdb(String),
    /// Steam's store page of this app.
    Steam(u32),
    /// SteamGridDB, by name.
    SteamGridDb,
}

impl How {
    /// From library.json's `metadata` field: `libretro:<system>`, `tmdb:movie` or `tmdb:tv`.
    pub fn parse(field: &str) -> Option<How> {
        match field.split_once(':')? {
            ("libretro", system) if !system.is_empty() => Some(How::Libretro(system.to_owned())),
            ("tmdb", kind @ ("movie" | "tv")) => Some(How::Tmdb(kind.to_owned())),
            _ => None,
        }
    }

    /// As saved with what was found: fetched again when it changes.
    pub fn source(&self) -> String {
        match self {
            How::Libretro(system) => format!("libretro:{system}"),
            How::Tmdb(kind) => format!("tmdb:{kind}"),
            How::Steam(appid) => format!("steam:{appid}"),
            How::SteamGridDb => "steamgriddb".into(),
        }
    }
}

/// An entry to fetch for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub id: Arc<str>,
    /// Its key: the art cache's files are named by it.
    pub key: String,
    pub title: String,
    pub how: How,
    /// It has art of its own already (no cover wanted).
    pub art: bool,
}

/// What a fetch needs.
pub struct Fetch {
    pub wanted: Vec<Wanted>,
    pub keys: Keys,
    /// The art cache.
    pub art: PathBuf,
    /// The database.
    pub db: PathBuf,
}

/// Fetches on [`THREADS`] threads until the list is done (or the UI is gone).
pub fn spawn(fetch: Fetch, sink: impl Sink + Clone) -> std::io::Result<()> {
    if fetch.wanted.is_empty() {
        return Ok(());
    }
    log::info!("meta: fetching for {} entries", fetch.wanted.len());
    let queue = Arc::new(Mutex::new(fetch.wanted.into_iter().collect::<VecDeque<_>>()));
    let shared = Arc::new(Shared { keys: fetch.keys, art: fetch.art, db: fetch.db, listings: Mutex::new(HashMap::new()) });
    for _ in 0..THREADS {
        let (queue, shared, sink) = (queue.clone(), shared.clone(), sink.clone());
        worker::detached("meta", move || {
            let store = Store::open(&shared.db).map_err(|e| log::warn!("meta: {e}")).ok();
            loop {
                let Some(w) = queue.lock().ok().and_then(|mut q| q.pop_front()) else {
                    return;
                };
                let meta = shared.fetch(&w);
                if let Some(store) = &store
                    && let Err(e) = store.save_meta(&w.id, &w.how.source(), now(), &meta)
                {
                    log::warn!("meta: {}: {e}", w.id);
                }
                if meta != Meta::default() && sink.send(Event::Meta { id: w.id.clone(), meta }).is_err() {
                    return;
                }
            }
        })?;
    }
    Ok(())
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What the threads share: the keys, where images go, and libretro's file lists (fetched once per
/// system and kind, `None` if that failed).
struct Shared {
    keys: Keys,
    art: PathBuf,
    db: PathBuf,
    listings: Mutex<HashMap<String, Option<Arc<Vec<String>>>>>,
}

impl Shared {
    fn fetch(&self, w: &Wanted) -> Meta {
        let meta = match &w.how {
            How::Libretro(system) => self.libretro(w, system),
            How::Tmdb(kind) => self.tmdb(w, kind),
            How::Steam(appid) => self.steam(w, *appid),
            How::SteamGridDb => self.steamgriddb(w),
        };
        log::info!("meta: {}: {}", w.id, if meta == Meta::default() { "nothing found" } else { "found" });
        meta
    }

    /// The image at `url` into the art cache as `<key>-<what>.<its extension>`.
    fn image(&self, w: &Wanted, what: &str, url: &str) -> Option<PathBuf> {
        let file = self.art.join(format!("{}-{what}.{}", w.key, extension(url)));
        download(url, &[], &file).then_some(file)
    }

    fn libretro(&self, w: &Wanted, system: &str) -> Meta {
        let Some(names) = self.listing(system, "Named_Boxarts") else {
            return Meta::default();
        };
        let Some(name) = best_match(&w.title, &names) else {
            return Meta::default();
        };
        let url = |kind: &str| format!("{LIBRETRO}/{}/{kind}/{}.png", encode(system), encode(name));
        Meta {
            cover: (!w.art).then(|| self.image(w, "cover", &url("Named_Boxarts"))).flatten(),
            shot: self.image(w, "shot", &url("Named_Snaps")),
            ..Meta::default()
        }
    }

    /// The names (without `.png`) in a system's folder of a kind, fetched once.
    fn listing(&self, system: &str, kind: &str) -> Option<Arc<Vec<String>>> {
        let id = format!("{system}/{kind}");
        let mut listings = self.listings.lock().ok()?;
        if let Some(l) = listings.get(&id) {
            return l.clone();
        }
        let page = get(&format!("{LIBRETRO}/{}/{kind}/", encode(system)), &[]).map(|b| String::from_utf8_lossy(&b).into_owned());
        let names = page.map(|p| Arc::new(hrefs(&p).filter_map(|h| h.strip_suffix(".png").map(str::to_owned)).collect::<Vec<_>>()));
        if names.is_none() {
            log::warn!("meta: libretro has no {id}");
        }
        listings.insert(id, names.clone());
        names
    }

    fn tmdb(&self, w: &Wanted, kind: &str) -> Meta {
        let Some(key) = self.keys.tmdb.as_deref() else {
            return Meta::default();
        };
        let (title, year) = title_and_year(&w.title);
        // A read access token (a JWT) goes in a header; an API key in the query.
        let bearer = key.starts_with("eyJ").then(|| format!("Authorization: Bearer {key}"));
        let mut url = format!("https://api.themoviedb.org/3/search/{kind}?include_adult=false&query={}", encode(title));
        if let Some(y) = year {
            url.push_str(&format!("&{}={y}", if kind == "tv" { "first_air_date_year" } else { "year" }));
        }
        if bearer.is_none() {
            url.push_str(&format!("&api_key={}", encode(key)));
        }
        let headers: Vec<&str> = bearer.iter().map(String::as_str).collect();
        let Some(hit) = get_json(&url, &headers).and_then(|v| v.get("results")?.get(0).cloned()) else {
            return Meta::default();
        };
        let path = |field: &str| hit.get(field).and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
        let date = path("first_air_date").or_else(|| path("release_date")).unwrap_or_default();
        Meta {
            cover: path("poster_path").filter(|_| !w.art).and_then(|p| self.image(w, "cover", &format!("https://image.tmdb.org/t/p/w500{p}"))),
            shot: path("backdrop_path").and_then(|p| self.image(w, "shot", &format!("https://image.tmdb.org/t/p/w780{p}"))),
            summary: path("overview").unwrap_or_default(),
            year: year_in(&date),
            developer: String::new(),
        }
    }

    fn steam(&self, w: &Wanted, appid: u32) -> Meta {
        let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=english");
        let Some(data) = get_json(&url, &[]).and_then(|v| v.get(appid.to_string())?.get("data").cloned()) else {
            return Meta::default();
        };
        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(entities).unwrap_or_default();
        let shot = data.get("screenshots").and_then(|s| s.get(0)?.get("path_full")?.as_str().map(str::to_owned));
        let cover = format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg");
        Meta {
            cover: (!w.art).then(|| self.image(w, "cover", &cover)).flatten(),
            shot: shot.and_then(|url| self.image(w, "shot", &url)),
            summary: text(data.get("short_description")),
            year: year_in(&text(data.get("release_date").and_then(|r| r.get("date")))),
            developer: text(data.get("developers").and_then(|d| d.get(0))),
        }
    }

    fn steamgriddb(&self, w: &Wanted) -> Meta {
        let (Some(key), false) = (self.keys.steamgriddb.as_deref(), w.art) else {
            return Meta::default();
        };
        let auth = format!("Authorization: Bearer {key}");
        let api = "https://www.steamgriddb.com/api/v2";
        let game = get_json(&format!("{api}/search/autocomplete/{}", encode(&w.title)), &[&auth]).and_then(|v| v.get("data")?.get(0)?.get("id")?.as_u64());
        let grid = game.and_then(|g| get_json(&format!("{api}/grids/game/{g}?dimensions=600x900"), &[&auth]));
        let url = grid.and_then(|v| v.get("data")?.get(0)?.get("url")?.as_str().map(str::to_owned));
        Meta { cover: url.and_then(|u| self.image(w, "cover", &u)), ..Meta::default() }
    }
}

const LIBRETRO: &str = "https://thumbnails.libretro.com";

/// The body of `url` (a GET, redirects followed), if it answered 2xx.
fn get(url: &str, headers: &[&str]) -> Option<Vec<u8>> {
    let mut curl = Command::new("curl");
    curl.args(["-sfL", "--max-time", "30", "-A", "ClamShell"]);
    for h in headers {
        curl.args(["-H", h]);
    }
    let out = curl.arg(url).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn get_json(url: &str, headers: &[&str]) -> Option<Value> {
    serde_json::from_slice(&get(url, headers)?).ok()
}

/// `url` into `file` (written whole, then renamed into place); whether it worked.
fn download(url: &str, headers: &[&str], file: &Path) -> bool {
    let Some(body) = get(url, headers).filter(|b| !b.is_empty()) else {
        return false;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = file.with_extension("part");
    std::fs::write(&part, body).and_then(|()| std::fs::rename(&part, file)).map_err(|e| log::warn!("meta: {}: {e}", file.display())).is_ok()
}

/// An image URL's extension (jpg, png or webp), jpg where it says none of them.
fn extension(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    match path.rsplit('.').next() {
        Some("png") => "png",
        Some("webp") => "webp",
        _ => "jpg",
    }
}

/// `text` for a URL's path or query: everything but letters, digits and `-._~` percent-encoded.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encoding undone.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The links of a directory listing's page, decoded.
fn hrefs(page: &str) -> impl Iterator<Item = String> + '_ {
    page.split("href=\"").skip(1).filter_map(|rest| rest.split('"').next()).map(decode)
}

/// A title as compared with libretro's names: bracketed parts, "the", case and anything but letters and
/// digits left out.
fn normal(title: &str) -> String {
    let mut kept = String::with_capacity(title.len());
    let mut depth = 0u32;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    kept.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && *w != "the").collect()
}

/// libretro's name for `title`: the same name (its characters libretro can't have in a file name as
/// `_`), else the release whose name is the same once [`normal`]: USA, then World, then Europe, then
/// any other, a prototype or demo last.
fn best_match<'a>(title: &str, names: &'a [String]) -> Option<&'a str> {
    let exact: String = title.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect();
    if let Some(n) = names.iter().find(|n| **n == exact) {
        return Some(n);
    }
    let wanted = normal(title);
    if wanted.is_empty() {
        return None;
    }
    let rank = |n: &str| {
        let unfinished = ["(Beta", "(Proto", "(Demo", "(Sample", "(Pirate", "(Unl"].iter().any(|t| n.contains(t));
        let region = ["(USA", "(World", "(Europe"].iter().position(|r| n.contains(r)).unwrap_or(3);
        (unfinished, region, n.len())
    };
    names.iter().filter(|n| normal(n) == wanted).min_by_key(|n| rank(n)).map(String::as_str)
}

/// A title and the year at its end in brackets, if any: "Cosmos (1980)".
fn title_and_year(title: &str) -> (&str, Option<u16>) {
    let t = title.trim_end();
    if let Some(open) = t.strip_suffix(')').and_then(|t| t.rfind('(')) {
        let year = t.get(open + 1..t.len() - 1).and_then(|y| y.parse::<u16>().ok()).filter(|y| (1870..2200).contains(y));
        if year.is_some() {
            return (t.get(..open).unwrap_or(t).trim_end(), year);
        }
    }
    (t, None)
}

/// The first four-digit year in `text` ("2020-09-17", "17 Sep, 2020").
fn year_in(text: &str) -> Option<u16> {
    text.split(|c: char| !c.is_ascii_digit()).find(|w| w.len() == 4).and_then(|w| w.parse().ok())
}

/// HTML's common entities undone (Steam's texts have them).
fn entities(text: &str) -> String {
    text.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn libretro_names_by_title() {
        let list = names(&[
            "Legend of Zelda, The - A Link to the Past (Europe)",
            "Legend of Zelda, The - A Link to the Past (USA)",
            "Legend of Zelda, The - A Link to the Past (USA) (Beta)",
            "Mario _ Luigi (USA)",
            "Super Metroid (Japan, USA) (En,Ja)",
        ]);
        assert_eq!(
            best_match("Legend of Zelda, The - A Link to the Past (Europe)", &list),
            Some("Legend of Zelda, The - A Link to the Past (Europe)"),
            "the same name"
        );
        assert_eq!(
            best_match("The Legend of Zelda - A Link to the Past", &list),
            Some("Legend of Zelda, The - A Link to the Past (USA)"),
            "USA first, no beta"
        );
        assert_eq!(best_match("Mario & Luigi", &list), Some("Mario _ Luigi (USA)"));
        assert_eq!(best_match("super metroid", &list), Some("Super Metroid (Japan, USA) (En,Ja)"));
        assert_eq!(best_match("Zelda", &list), None, "nothing loosely");
        assert_eq!(best_match("(USA)", &list), None);
    }

    #[test]
    fn urls_titles_and_years() {
        assert_eq!(encode("Nintendo - SNES/A&B.png"), "Nintendo%20-%20SNES%2FA%26B.png");
        assert_eq!(decode("A%20B%26C.png"), "A B&C.png");
        let page = r#"<a href="?C=N;O=D">Name</a><a href="Mario%20_%20Luigi%20(USA).png">x</a><a href="Zelda.png">y</a>"#;
        assert_eq!(hrefs(page).collect::<Vec<_>>(), vec!["?C=N;O=D", "Mario _ Luigi (USA).png", "Zelda.png"]);
        assert_eq!(title_and_year("Cosmos (1980)"), ("Cosmos", Some(1980)));
        assert_eq!(title_and_year("Blade Runner 2049"), ("Blade Runner 2049", None));
        assert_eq!(title_and_year("Doom (Final)"), ("Doom (Final)", None));
        assert_eq!(year_in("17 Sep, 2020"), Some(2020));
        assert_eq!(year_in("2019-03-01"), Some(2019));
        assert_eq!(extension("https://x/a.PNG?t=1"), "png");
        assert_eq!(extension("https://x/a"), "jpg");
        assert_eq!(entities("Tom &amp; Jerry&#39;s"), "Tom & Jerry's");
        assert_eq!(How::parse("libretro:Sega - Mega Drive - Genesis").map(|h| h.source()).as_deref(), Some("libretro:Sega - Mega Drive - Genesis"));
        assert_eq!(How::parse("tmdb:anime"), None);
    }
}
