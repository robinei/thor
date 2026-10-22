//! Sorting titles, and the order of a list of entries ([`list_order`]).
//!
//! Case is ignored. Whitespace sorts before punctuation, punctuation before digits and digits before
//! letters, so "Cave Story+" comes before "Caves of Qud" and "Game Saves" before "Games". Numbers sort by
//! value, ignoring leading zeros, so "2." comes before "10.". The sort key is built once per title.

use std::cmp::Ordering;

use crate::{BrowseKind, Entry};

/// The key to sort `title` by. Each character becomes its class and its lowercase form. A run of digits
/// becomes its length without leading zeros, followed by the digits.
pub fn key(title: &str) -> Vec<(u8, char)> {
    let mut key = Vec::with_capacity(title.len());
    let mut chars = title.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut digits = vec![c];
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                digits.push(d);
                chars.next();
            }
            let first = digits.iter().position(|&d| d != '0').unwrap_or(digits.len() - 1);
            let digits = digits.get(first..).unwrap_or_default();
            key.push((2, char::from_u32(digits.len() as u32).unwrap_or('\0')));
            key.extend(digits.iter().map(|&d| (2, d)));
            continue;
        }
        let class = if c.is_whitespace() {
            0
        } else if c.is_alphabetic() {
            3
        } else if c.is_numeric() {
            2
        } else {
            1
        };
        key.push((class, c.to_lowercase().next().unwrap_or(c)));
    }
    key
}

/// The order of a tab's or a folder's entries: folders first, then by title. The top folders of browsed
/// dirs come after the other folders, in the order library.json and the mounts give them.
pub fn list_order(games: &[Entry], a: usize, b: usize) -> Ordering {
    let key = |i: usize| {
        games.get(i).map(|g| match g.browsed.as_ref().map(|b| &b.kind) {
            Some(BrowseKind::Root | BrowseKind::Trash | BrowseKind::Removable(_)) => (false, true, None),
            _ => (!g.is_group(), false, Some((&g.sort_key, &g.title))),
        })
    };
    key(a).cmp(&key(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut titles: Vec<&str>) -> Vec<&str> {
        titles.sort_by(|a, b| key(a).cmp(&key(b)).then(a.cmp(b)));
        titles
    }

    #[test]
    fn letters_and_marks_in_their_order() {
        assert_eq!(sorted(vec!["Caves of Qud", "Cave Story+"]), vec!["Cave Story+", "Caves of Qud"]);
        assert_eq!(sorted(vec!["Games", "Game Saves"]), vec!["Game Saves", "Games"]);
        assert_eq!(sorted(vec!["the Witcher", "Diablo", "diablo: Hellfire"]), vec!["Diablo", "diablo: Hellfire", "the Witcher"]);
    }

    #[test]
    fn numbers_by_their_value() {
        assert_eq!(sorted(vec!["10. Ten", "2. Two", "1. One"]), vec!["1. One", "2. Two", "10. Ten"]);
        assert_eq!(sorted(vec!["Season 10", "Season 9", "Season 01"]), vec!["Season 01", "Season 9", "Season 10"]);
        assert_eq!(sorted(vec!["Doom 2", "Doom"]), vec!["Doom", "Doom 2"]);
    }
}
