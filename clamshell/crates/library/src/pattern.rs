//! Name patterns, as used by library.json's `match` and `items`. `*` matches any run of characters, `?`
//! matches one character, and anything else matches itself. Neither `*` nor `?` crosses `/`.
//!
//! A pattern with several parts, such as `*/*.cue`, is matched one part per folder level; clamshell-system
//! does that on disk. If the last part ends in `/`, it matches folders only.

/// Whether `name`, a single file or folder name, matches `pattern`, a single part of a pattern.
pub fn matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    // The classic two-pointer match: on a mismatch, the last `*` takes one more character.
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi += 1;
            }
            Some(&c) if c == '?' || Some(&c) == n.get(ni) => {
                pi += 1;
                ni += 1;
            }
            _ => match star {
                Some((sp, sn)) => {
                    pi = sp + 1;
                    ni = sn + 1;
                    star = Some((sp, sn + 1));
                }
                None => return false,
            },
        }
    }
    p.get(pi..).is_some_and(|rest| rest.iter().all(|&c| c == '*'))
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
    }

    #[test]
    fn parts_per_folder_level() {
        assert_eq!(parts("*/*.cue"), (vec!["*", "*.cue"], false));
        assert_eq!(parts("*/"), (vec!["*"], true));
        assert!(hidden(".git", "*") && !hidden(".git", ".*") && !hidden("a", "*"));
    }
}
