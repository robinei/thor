//! HTTP GETs, by curl, and files kept from them.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::os::{command, fs};

/// The body of a GET of `url`, following redirects, if the answer is 2xx. The URL and the headers go to
/// curl on its stdin as a config (`-K -`), because they may hold an API key and anyone can read a command
/// line in /proc.
pub fn get(url: &str, headers: &[&str]) -> Option<Vec<u8>> {
    let mut config = format!("url = {}\n", quoted(url));
    for h in headers {
        config.push_str(&format!("header = {}\n", quoted(h)));
    }
    let out = command::run("curl", &["-sfL", "--max-time", "30", "--max-filesize", "64M", "-A", "ClamShell", "-K", "-"], Some(config.as_bytes()), CURL_LIMIT)
        .inspect_err(|e| log::warn!("{}: {e}", shown(url)))
        .ok()?;
    if !out.status.success() {
        log::debug!("{}: curl {}", shown(url), out.status);
        return None;
    }
    Some(out.stdout)
}

/// `url` as a log line shows it, without its query, which may hold an API key.
fn shown(url: &str) -> &str {
    url.split_once('?').map_or(url, |(base, _)| base)
}

/// How long curl may take: its own `--max-time` and a little more.
const CURL_LIMIT: Duration = Duration::from_secs(35);

/// `text` as a quoted string for a curl config.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The body of `url` (see [`get`]) as JSON.
pub fn get_json(url: &str, headers: &[&str]) -> Option<Value> {
    serde_json::from_slice(&get(url, headers)?).inspect_err(|e| log::warn!("{}: {e}", shown(url))).ok()
}

/// The contents of `file` if it is younger than `max_age`. Otherwise the contents are fetched with `fetch`
/// and kept in `file` (see [`fs::write_file`]). If the fetch fails, the older contents are used, if any.
pub fn kept(file: &Path, max_age: Duration, fetch: impl FnOnce() -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let age = std::fs::metadata(file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if age.is_some_and(|a| a < max_age) {
        match std::fs::read(file) {
            Ok(kept) => return Some(kept),
            Err(e) => log::warn!("{}: {e}", file.display()),
        }
    }
    match fetch().filter(|b| !b.is_empty()) {
        Some(body) => {
            if let Err(e) = fs::write_file(file, &body) {
                log::warn!("{}: {e}", file.display());
            }
            Some(body)
        }
        None => std::fs::read(file)
            .inspect_err(|e| {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("{}: {e}", file.display());
                }
            })
            .ok(),
    }
}

/// Downloads `url` into `file` (see [`fs::write_file`]). Returns whether it worked.
pub fn download(url: &str, headers: &[&str], file: &Path) -> bool {
    let Some(body) = get(url, headers).filter(|b| !b.is_empty()) else {
        return false;
    };
    fs::write_file(file, &body).map_err(|e| log::warn!("{}: {e}", file.display())).is_ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_kept_fetched_again_when_old_and_used_old_when_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let file = dir.join("Named_Boxarts - Nintendo - SNES.html");
        let day = std::time::Duration::from_secs(86_400);
        assert_eq!(kept(&file, day, || None), None, "nothing kept, nothing fetched");
        assert_eq!(kept(&file, day, || Some(b"one".to_vec())).as_deref(), Some(&b"one"[..]));
        assert_eq!(kept(&file, day, || panic!("not fetched again")).as_deref(), Some(&b"one"[..]), "kept");
        let now = std::time::Duration::ZERO;
        assert_eq!(kept(&file, now, || Some(b"two".to_vec())).as_deref(), Some(&b"two"[..]), "old: fetched again");
        assert_eq!(kept(&file, now, || None).as_deref(), Some(&b"two"[..]), "the fetch failing: the old one");
    }

    #[test]
    fn curl_config_strings() {
        assert_eq!(quoted("https://x/?q=a b"), r#""https://x/?q=a b""#);
        assert_eq!(quoted("say \"hi\" \\ there\n"), r#""say \"hi\" \\ there\n""#);
    }
}
