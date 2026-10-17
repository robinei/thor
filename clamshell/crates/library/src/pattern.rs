//! Name patterns, as library.json's `match` and `items` give them: `*` for any run of characters, `?` for
//! one, anything else itself; neither crosses `/`. A pattern of several parts (`*/*.cue`) is matched a
//! part per folder level (by clamshell-system, on disk); a last part ending in `/` matches folders only.

/// Whether `name` (one file or folder name) matches `pattern` (one part of a pattern).
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

/// Whether a name is hidden (starts with a dot): patterns skip hidden names unless their part starts
/// with a dot too.
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
