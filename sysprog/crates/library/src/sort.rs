//! Sorting titles the way Pegasus does (ICU collation for en_US, docs/pegasus-format.md 2.10): case
//! ignored, and whitespace before punctuation before digits before letters, so "Cave Story+" comes
//! before "Caves of Qud" and "Game Saves" before "Games". A key built once per title.

/// The key to sort `title` by: per character, its class, then the character lowercased.
pub fn key(title: &str) -> Vec<(u8, char)> {
    title
        .chars()
        .map(|c| {
            let class = if c.is_whitespace() {
                0
            } else if c.is_alphabetic() {
                3
            } else if c.is_numeric() {
                2
            } else {
                1
            };
            (class, c.to_lowercase().next().unwrap_or(c))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut titles: Vec<&str>) -> Vec<&str> {
        titles.sort_by(|a, b| key(a).cmp(&key(b)).then(a.cmp(b)));
        titles
    }

    #[test]
    fn matches_pegasus_order() {
        assert_eq!(sorted(vec!["Caves of Qud", "Cave Story+"]), vec!["Cave Story+", "Caves of Qud"]);
        assert_eq!(sorted(vec!["Games", "Game Saves"]), vec!["Game Saves", "Games"]);
        assert_eq!(sorted(vec!["the Witcher", "Diablo", "diablo: Hellfire"]), vec!["Diablo", "diablo: Hellfire", "the Witcher"]);
    }
}
