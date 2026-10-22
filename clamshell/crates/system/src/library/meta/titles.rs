//! Matching titles to names as others write them: libretro's names, ignoring case, tags, "the",
//! punctuation and Roman numerals. Also reading years and developers from the stores' text.

use super::libretro::thumbnail_name;

/// A title in the form compared with libretro's names. Bracketed parts, a ` # ...` tail, "the", "and",
/// case and anything but letters and digits are dropped, and Roman numerals from II to X become numbers.
pub(super) fn normal(title: &str) -> String {
    // A ` # ...` tail marks a translation, as in "Star Ocean # SNES".
    let title = title.split_once(" # ").map_or(title, |(t, _)| t);
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
    let numeral = |w: &str| ["ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"].iter().position(|n| *n == w).map(|i| (i + 2).to_string());
    kept.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && *w != "the" && *w != "and")
        .map(|w| numeral(w).unwrap_or_else(|| w.to_owned()))
        .collect()
}

/// A title without its subtitle (what follows ` - `), if it has one.
pub(super) fn main_title(title: &str) -> Option<&str> {
    title.split_once(" - ").map(|(main, _)| main)
}

/// A libretro name, and its [`normal`] form both whole and without its subtitle.
#[derive(Debug, Clone)]
pub(super) struct Name {
    name: String,
    normal: String,
    main: Option<String>,
}

impl Name {
    pub(super) fn new(name: &str) -> Name {
        Name { name: name.to_owned(), normal: normal(name), main: main_title(name).map(normal) }
    }
}

/// libretro's name for `title`.
///
/// An exact match comes first, with characters libretro can't have in a file name replaced by `_`.
/// Otherwise it picks among the releases whose [`normal`] form is the same. Failing that, if only one game
/// fits, it picks among those that are the same apart from libretro's subtitle. Among releases, USA comes
/// first, then World, then Europe, then any other, with prototypes and demos last.
pub(super) fn best_match<'a>(title: &str, names: &'a [Name]) -> Option<&'a str> {
    let exact = thumbnail_name(title);
    if let Some(n) = names.iter().find(|n| n.name == exact) {
        return Some(&n.name);
    }
    let wanted = normal(title);
    if wanted.is_empty() {
        return None;
    }
    let rank = |n: &&Name| {
        let n = &n.name;
        let unfinished = ["(Beta", "(Proto", "(Demo", "(Sample", "(Pirate", "(Unl"].iter().any(|t| n.contains(t));
        let region = ["(USA", "(World", "(Europe"].iter().position(|r| n.contains(r)).unwrap_or(3);
        (unfinished, region, n.len())
    };
    let best = |found: Vec<&'a Name>| found.into_iter().min_by_key(rank).map(|n| n.name.as_str());
    let same: Vec<&Name> = names.iter().filter(|n| n.normal == wanted).collect();
    if !same.is_empty() {
        return best(same);
    }
    // libretro's names with a subtitle the title doesn't have, if they are all one game.
    let loose: Vec<&Name> = names.iter().filter(|n| n.main.as_ref() == Some(&wanted)).collect();
    let games: std::collections::HashSet<&str> = loose.iter().map(|n| n.normal.as_str()).collect();
    if games.len() == 1 { best(loose) } else { None }
}

/// A title and the year at its end in brackets, if any: "Cosmos (1980)".
pub(super) fn title_and_year(title: &str) -> (&str, Option<u16>) {
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
pub(super) fn year_in(text: &str) -> Option<u16> {
    text.split(|c: char| !c.is_ascii_digit()).find(|w| w.len() == 4).and_then(|w| w.parse().ok())
}

/// A developer as libretro's database gives it, without its "???" (unknown) parts.
pub(super) fn developer(text: &str) -> String {
    text.split_whitespace().filter(|w| !w.chars().all(|c| c == '?')).collect::<Vec<_>>().join(" ")
}

/// Decodes HTML's common entities, which Steam's texts contain.
pub(super) fn entities(text: &str) -> String {
    text.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<Name> {
        list.iter().map(|s| Name::new(s)).collect()
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
        let list = names(&[
            "Mega Man 7 (USA)",
            "Musya - The Classic Japanese Tale of Horror (USA)",
            "Street Fighter II (USA)",
            "Final Fantasy - Mystic Quest (USA)",
            "Final Fantasy II (USA)",
            "Final Fantasy III (USA)",
            "Super Mario World (USA)",
            "Super Mario World 2 - Yoshi's Island (USA)",
        ]);
        assert_eq!(best_match("Mega Man VII (U)", &list), Some("Mega Man 7 (USA)"), "a numeral as a number");
        assert_eq!(best_match("Musya (U) [!]", &list), Some("Musya - The Classic Japanese Tale of Horror (USA)"), "its subtitle left out");
        assert_eq!(best_match("Street Fighter II - The World Warrior (U) [!]", &list), None, "a subtitle of its own: maybe another game");
        assert_eq!(best_match("Final Fantasy (U)", &list), Some("Final Fantasy - Mystic Quest (USA)"), "the one game it can be");
        assert_eq!(best_match("Rocky and Bullwinkle", &names(&["Rocky & Bullwinkle (USA)"])), Some("Rocky & Bullwinkle (USA)"), "and, &");
        assert_eq!(best_match("Star Ocean # SNES", &names(&["Star Ocean (Japan)"])), Some("Star Ocean (Japan)"), "a translation's tail");
        assert_eq!(best_match("Super Mario World 2 (U)", &list), Some("Super Mario World 2 - Yoshi's Island (USA)"));
        let two = names(&["Toys - Let the Toy Wars Begin! (USA)", "Toys - Something Else (USA)"]);
        assert_eq!(best_match("Toys", &two), None, "two games it could be: neither");
    }
}
