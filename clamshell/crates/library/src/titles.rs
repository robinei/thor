//! Titles from file and folder names. This drops extensions and ROM dump tags, and recognizes season
//! folders and the episode numbers in them.

/// The season number of a folder, from names like `Season 2`, `S02` or `Series 2`. `Specials` is season 0.
pub fn season_of(folder: &str) -> Option<u32> {
    let name = folder.trim().to_lowercase();
    if name == "specials" {
        return Some(0);
    }
    let number = ["season", "series", "s"].iter().find_map(|w| name.strip_prefix(w))?;
    number.trim().parse().ok()
}

/// An episode's title in the folder of season `season`, from its file name. `017 S01E18 Title` in season 1
/// becomes `18. Title`, and `S05E01E02 Title` becomes `1–2. Title`. Returns none if the name has no
/// episode of that season.
pub fn episode_title(stem: &str, season: u32) -> Option<String> {
    let chars: Vec<char> = stem.chars().collect();
    let digits = |from: usize| -> (u32, usize) {
        let end = (from..chars.len()).find(|&i| !chars.get(i).is_some_and(char::is_ascii_digit)).unwrap_or(chars.len());
        (chars.get(from..end).unwrap_or(&[]).iter().collect::<String>().parse().unwrap_or(u32::MAX), end)
    };
    for at in 0..chars.len() {
        if !matches!(chars.get(at), Some('S' | 's')) || at > 0 && chars.get(at - 1).is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        let (s, after_s) = digits(at + 1);
        if after_s == at + 1 || !matches!(chars.get(after_s), Some('E' | 'e')) {
            continue;
        }
        let (first, mut end) = digits(after_s + 1);
        if end == after_s + 1 || s != season {
            continue;
        }
        // A double episode: E01E02, E01-E02, E01-02.
        let mut last = first;
        let next = if chars.get(end) == Some(&'-') { end + 1 } else { end };
        let next = if matches!(chars.get(next), Some('E' | 'e')) {
            next + 1
        } else if next > end {
            next
        } else {
            usize::MAX
        };
        if next != usize::MAX {
            let (n, e) = digits(next);
            if e > next && n > first {
                (last, end) = (n, e);
            }
        }
        let rest: String = chars.get(end..).unwrap_or(&[]).iter().collect();
        let rest = untagged(rest.trim_start_matches([' ', '-', '.', '_']).trim());
        let number = if last > first { format!("{first}–{last}") } else { first.to_string() };
        return Some(if rest.is_empty() { format!("{number}.") } else { format!("{number}. {rest}") });
    }
    None
}

/// A file name without its extension.
pub fn stem(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_owned()
}

/// A title without the dump tags at its end, such as `(U) [!]`, `(USA) (Rev 1)`, `(Japan, USA) (En,Ja)`
/// or a translation's ` # SNES`. Other words in brackets stay, like `(Part 1)`, and so does a year, like
/// `Cosmos (1980)`.
pub fn untagged(title: &str) -> String {
    let mut t = title.split_once(" # ").map_or(title, |(t, _)| t).trim_end();
    while let Some(open) = t.strip_suffix([')', ']']).and_then(|rest| rest.rfind(['(', '['])) {
        let square = t.ends_with(']');
        let tag = t.get(open + 1..t.len() - 1).unwrap_or("");
        let rest = t.get(..open).unwrap_or("").trim_end();
        if rest.is_empty() || !(square || dump_tag(tag)) {
            break;
        }
        t = rest;
    }
    t.to_owned()
}

/// Whether a bracketed tag is a ROM dump tag: a region, languages, a revision or a release stage. Examples:
/// `U`, `JU`, `M5`, `V1.1`, `USA`, `Japan, USA`, `En,Ja`, `Rev 1`, `Beta`, `Unl`, `Hack`.
pub fn dump_tag(tag: &str) -> bool {
    const WORDS: [&str; 26] = [
        "usa",
        "europe",
        "japan",
        "world",
        "asia",
        "korea",
        "brazil",
        "germany",
        "france",
        "spain",
        "italy",
        "australia",
        "canada",
        "china",
        "rev",
        "beta",
        "proto",
        "prototype",
        "demo",
        "sample",
        "unl",
        "hack",
        "pirate",
        "virtual console",
        "en",
        "ja",
    ];
    let short_code = tag.len() <= 5
        && tag.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-')
        && tag.chars().any(|c| c.is_ascii_uppercase());
    let words = tag.split([',', '+']).map(str::trim).all(|w| {
        let w = w.to_ascii_lowercase();
        WORDS.iter().any(|k| w == *k || w.starts_with(&format!("{k} "))) || (w.len() == 2 && w.chars().all(|c| c.is_ascii_alphabetic()))
    });
    short_code || words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seasons_and_episodes_by_their_numbers() {
        assert_eq!((season_of("Season 2"), season_of("S02"), season_of("Series 10"), season_of("Specials")), (Some(2), Some(2), Some(10), Some(0)));
        assert_eq!((season_of("Extras"), season_of("Sxx")), (None, None));
        assert_eq!(episode_title("017 S01E18 A Voice in the Wilderness (Part 1)", 1).as_deref(), Some("18. A Voice in the Wilderness (Part 1)"));
        assert_eq!(episode_title("S01E01 Killeroo", 1).as_deref(), Some("1. Killeroo"));
        assert_eq!(episode_title("001 S00E01 The Gathering", 0).as_deref(), Some("1. The Gathering"));
        assert_eq!(episode_title("Show.S05E01E02.Double.Trouble", 5).as_deref(), Some("1–2. Double.Trouble"));
        assert_eq!(episode_title("S03E07-08 Two Parter", 3).as_deref(), Some("7–8. Two Parter"));
        assert_eq!(episode_title("S02E03 Wrong season", 1), None, "another season's: as named");
        assert_eq!(episode_title("Sunset Boulevard", 1), None);
    }

    #[test]
    fn titles_lose_their_tags_but_not_a_year() {
        assert_eq!(untagged("7th Saga, The (U) [!]"), "7th Saga, The");
        assert_eq!(untagged("Super Metroid (Japan, USA) (En,Ja)"), "Super Metroid");
        assert_eq!(untagged("Star Ocean # SNES"), "Star Ocean");
        assert_eq!(untagged("Cosmos (1980)"), "Cosmos (1980)");
        assert_eq!(untagged("(Untitled)"), "(Untitled)");
        assert_eq!(untagged("1. Silence"), "1. Silence");
        assert_eq!(untagged("017 S01E18 A Voice in the Wilderness (Part 1)"), "017 S01E18 A Voice in the Wilderness (Part 1)");
        assert_eq!(untagged("Super Battletank - War in the Gulf (U) (V1.1) [!]"), "Super Battletank - War in the Gulf");
        assert_eq!(untagged("Street Fighter II (USA) (Rev 1)"), "Street Fighter II");
        assert_eq!(untagged("Power Drive (U) (Beta) (M5)"), "Power Drive");
        assert_eq!(untagged("Doom (Final)"), "Doom (Final)");
    }
}
