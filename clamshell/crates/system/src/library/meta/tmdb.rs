//! TMDB: films' and series' posters, backdrops, overviews and years, by title; needs `keys.tmdb`.

use clamshell_core::Meta;
use clamshell_library::Details;
use serde_json::Value;

use super::titles::{title_and_year, year_in};
use super::{Shared, Wanted};
use crate::os::{http, text};

impl Shared {
    pub(super) fn tmdb(&self, w: &Wanted, kind: &str) -> Meta {
        let Some(key) = self.keys.tmdb.as_deref() else {
            return Meta::default();
        };
        let (title, year) = title_and_year(&w.title);
        // A read access token (a JWT) goes in a header; an API key in the query.
        let bearer = key.starts_with("eyJ").then(|| format!("Authorization: Bearer {key}"));
        let headers: Vec<&str> = bearer.iter().map(String::as_str).collect();
        let search = |year: Option<u16>| {
            let mut url = format!("https://api.themoviedb.org/3/search/{kind}?include_adult=false&query={}", text::percent_encode(title));
            if let Some(y) = year {
                url.push_str(&format!("&{}={y}", if kind == "tv" { "first_air_date_year" } else { "year" }));
            }
            if bearer.is_none() {
                url.push_str(&format!("&api_key={}", text::percent_encode(key)));
            }
            http::get_json(&url, &headers).and_then(|v| v.get("results")?.get(0).cloned())
        };
        // By the title's year first; else (TMDB may date a series from another year than its pilot's) any.
        let Some(hit) = search(year).or_else(|| year.and_then(|_| search(None))) else {
            return Meta::default();
        };
        let path = |field: &str| hit.get(field).and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned);
        let date = path("first_air_date").or_else(|| path("release_date")).unwrap_or_default();
        Meta {
            cover: path("poster_path").filter(|_| !w.art).and_then(|p| self.image(w, "cover", &format!("https://image.tmdb.org/t/p/w500{p}"))),
            details: Details {
                shot: path("backdrop_path").and_then(|p| self.image(w, "shot", &format!("https://image.tmdb.org/t/p/w780{p}"))),
                summary: path("overview").unwrap_or_default(),
                year: year_in(&date),
                developer: String::new(),
            },
            duration: None,
        }
    }
}
