//! SteamGridDB: a cover by name for games from desktop files. Needs `keys.steamgriddb`.

use clamshell_core::Meta;

use super::{Shared, Wanted};
use crate::os::{http, text};

impl Shared {
    pub(super) fn steamgriddb(&self, w: &Wanted) -> Meta {
        let (Some(key), false) = (self.keys.steamgriddb.as_deref(), w.art) else {
            return Meta::default();
        };
        let auth = format!("Authorization: Bearer {key}");
        let api = "https://www.steamgriddb.com/api/v2";
        let game = http::get_json(&format!("{api}/search/autocomplete/{}", text::percent_encode(&w.title)), &[&auth])
            .and_then(|v| v.get("data")?.get(0)?.get("id")?.as_u64());
        let grid = game.and_then(|g| http::get_json(&format!("{api}/grids/game/{g}?dimensions=600x900"), &[&auth]));
        let url = grid.and_then(|v| v.get("data")?.get(0)?.get("url")?.as_str().map(str::to_owned));
        Meta { cover: url.and_then(|u| self.image(w, "cover", &u)), ..Meta::default() }
    }
}
