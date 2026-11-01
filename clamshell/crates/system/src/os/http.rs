//! HTTP GETs, by curl, and files kept from them.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::os::{command, fs};

/// The body of a GET of `url`, following redirects, if the answer is 2xx. The URL and the headers go to
/// curl on its stdin as a config (`-K -`), because they may hold an API key and anyone can read a command
/// line in /proc.
pub(crate) fn get(url: &str, headers: &[&str]) -> Option<Vec<u8>> {
    let mut config = format!("url = {}\n", quoted(url));
    for h in headers {
        config.push_str(&format!("header = {}\n", quoted(h)));
    }
    let out = command::run("curl", &["-sfL", "--max-time", "30", "--max-filesize", "64M", "-A", "ClamShell", "-K", "-"], Some(config.as_bytes()), CURL_LIMIT)
        .inspect_err(|e| {
            log::warn!("{}: {e}", shown(url));
            network_failed();
        })
        .ok()?;
    if !out.status.success() {
        log::debug!("{}: curl {}", shown(url), out.status);
        // 22: the server answered with an error, such as not found. Anything else is the way there.
        if out.status.code() != Some(CURL_HTTP_ERROR) {
            network_failed();
        }
        return None;
    }
    Some(out.stdout)
}

/// curl's exit code for an answer of 400 or more (with `-f`).
const CURL_HTTP_ERROR: i32 = 22;

std::thread_local! {
    static NETWORK_FAILURES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts a GET on this thread as failed on the way (see [`network_failures`]), also for a list whose
/// fetch failed so, used again.
pub(crate) fn network_failed() {
    NETWORK_FAILURES.with(|n| n.set(n.get() + 1));
}

/// How many GETs on this thread failed on the way rather than with the server's answer: compared before
/// and after a fetch, it says whether nothing was found or nothing could be asked.
pub(crate) fn network_failures() -> u64 {
    NETWORK_FAILURES.with(std::cell::Cell::get)
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
pub(crate) fn get_json(url: &str, headers: &[&str]) -> Option<Value> {
    serde_json::from_slice(&get(url, headers)?).inspect_err(|e| log::warn!("{}: {e}", shown(url))).ok()
}

/// The contents of `file` if it is younger than `max_age`. Otherwise the contents are fetched with `fetch`
/// and kept in `file` (see [`fs::write_file`]). If the fetch fails, the older contents are used, if any.
pub(crate) fn kept(file: &Path, max_age: Duration, fetch: impl FnOnce() -> Option<Vec<u8>>) -> Option<Vec<u8>> {
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
pub(crate) fn download(url: &str, headers: &[&str], file: &Path) -> bool {
    let Some(body) = get(url, headers).filter(|b| !b.is_empty()) else {
        return false;
    };
    fs::write_file(file, &body).map_err(|e| log::warn!("{}: {e}", file.display())).is_ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// A GET that can't reach the server counts as the network failing, which a fetch then doesn't keep as
    /// a miss.
    #[test]
    fn a_get_that_cant_connect_is_a_network_failure() {
        let before = network_failures();
        assert_eq!(get("http://127.0.0.1:9/", &[]), None);
        assert_eq!(network_failures(), before + 1);
    }

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
