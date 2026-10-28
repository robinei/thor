//! Name patterns, as used by library.json's `match` and `items`. `*` matches any run of characters, `?`
//! matches one character, and anything else matches itself. Neither `*` nor `?` crosses `/`.
//!
//! A pattern with several parts, such as `*/*.cue`, is matched one part per folder level; clamshell-system
//! does that on disk. If the last part ends in `/`, it matches folders only.

/// Whether `name`, a single file or folder name, matches `pattern`, a single part of a pattern.
pub fn matches(pattern: &str, name: &str) -> bool {
    glob(pattern, name, |a, b| a == b)
}

/// [`matches()`], with letters matching in either case.
pub fn matches_ignoring_case(pattern: &str, name: &str) -> bool {
    glob(pattern, name, |a, b| a == b || a.to_lowercase().eq(b.to_lowercase()))
}

/// The classic two-pointer match, walking the strings by byte offsets so nothing is allocated: on a
/// mismatch, the last `*` takes one more character.
fn glob(pattern: &str, name: &str, same: impl Fn(char, char) -> bool) -> bool {
    let at = |s: &str, i: usize| s.get(i..).and_then(|rest| rest.chars().next());
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while let Some(n) = at(name, ni) {
        match at(pattern, pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi += 1;
            }
            Some(c) if c == '?' || same(c, n) => {
                pi += c.len_utf8();
                ni += n.len_utf8();
            }
            _ => match star {
                Some((sp, sn)) => {
                    // The `*` takes the character at `sn`.
                    let taken = at(name, sn).map_or(1, char::len_utf8);
                    pi = sp + 1;
                    ni = sn + taken;
                    star = Some((sp, sn + taken));
                }
                None => return false,
            },
        }
    }
    pattern.get(pi..).is_some_and(|rest| rest.chars().all(|c| c == '*'))
}

/// A pattern's parts, folder level by level, and whether its last part matches folders only.
pub fn parts(pattern: &str) -> (Vec<&str>, bool) {
    let folders = pattern.ends_with('/');
    (pattern.trim_end_matches('/').split('/').filter(|p| !p.is_empty()).collect(), folders)
}

/// Whether a name counts as hidden for `part`. Names starting with a dot are skipped unless the pattern
/// part starts with a dot too.
pub fn hidden(name: &str, part: &str) -> bool {
    name.starts_with('.') && !part.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_marks() {
        assert!(matches("*.sfc", "Zelda.sfc"));
        assert!(!matches("*.sfc", "Zelda.smc"));
        assert!(matches("*", "anything"));
        assert!(matches("Season ?", "Season 1") && !matches("Season ?", "Season 10"));
        assert!(matches("*a*b", "xaxxb") && !matches("*a*b", "xaxxbc"));
        assert!(matches("", "") && !matches("", "x"));
        assert!(matches("Pokémon ?", "Pokémon é") && matches("*ö*", "Bjö rk") && !matches("?", "éé"), "characters, not bytes");
    }

    #[test]
    fn letters_in_either_case() {
        assert!(matches_ignoring_case("*.SFC", "Zelda.sfc") && matches_ignoring_case("ÉTÉ*", "été 1"));
        assert!(!matches("*.SFC", "Zelda.sfc"));
    }

    #[test]
    fn parts_per_folder_level() {
        assert_eq!(parts("*/*.cue"), (vec!["*", "*.cue"], false));
        assert_eq!(parts("*/"), (vec!["*"], true));
        assert!(hidden(".git", "*") && !hidden(".git", ".*") && !hidden("a", "*"));
    }
}
