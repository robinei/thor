//! Steam's store: a game's description, developer, year and first screenshot, by its app id.

use clamshell_library::Details;
use clamshell_state::Meta;
use serde_json::Value;

use super::titles::{entities, year_in};
use super::{Shared, Wanted};
use crate::os::http;

impl Shared {
    pub(super) fn steam(&self, w: &Wanted, appid: u32) -> Meta {
        let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=english");
        let Some(data) = http::get_json(&url, &[]).and_then(|v| v.get(appid.to_string())?.get("data").cloned()) else {
            return Meta::default();
        };
        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(entities).unwrap_or_default();
        let shot = data.get("screenshots").and_then(|s| s.get(0)?.get("path_full")?.as_str().map(str::to_owned));
        let cover = format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg");
        Meta {
            cover: (!w.art).then(|| self.image(w, "cover", &cover)).flatten(),
            details: Details {
                shot: shot.and_then(|url| self.image(w, "shot", &url)),
                summary: text(data.get("short_description")),
                year: year_in(&text(data.get("release_date").and_then(|r| r.get("date")))),
                developer: text(data.get("developers").and_then(|d| d.get(0))),
            },
            duration: None,
        }
    }
}
