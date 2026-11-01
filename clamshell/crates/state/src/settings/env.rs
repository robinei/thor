//! Environment variables as the editor's text holds them, as `KEY=value` words.

/// Parses `KEY=value` words (see [`Words`]) into variables, appended to `out`.
pub(crate) fn parse_env(text: &str, out: &mut Vec<(String, String)>) {
    out.extend(vars(text).map(|(k, v)| (k.to_owned(), v.to_owned())));
}

/// How many variables [`parse_env`] finds in `text`, without allocating, for drawing.
pub fn count_env(text: &str) -> usize {
    vars(text).count()
}

/// The `KEY=value` words of `text` whose key is a variable's name ([`env_name`]), split at their first
/// `=`. A word with another key is left out, so the tile counts only the variables a launch gets.
fn vars(text: &str) -> impl Iterator<Item = (&str, &str)> {
    Words { rest: text }.filter_map(|word| word.split_once('=')).filter(|(k, _)| env_name(k))
}

/// Whether `name` can be a launched program's environment variable: `[A-Za-z_][A-Za-z0-9_]*`, what
/// systemd-run and a shell take. systemd-run refuses a launch with any other, so every launch's
/// variables are checked by this one rule: ClamShell's own, the Env tile's and Steam's hook's.
pub fn env_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The words of a text. Words are separated by whitespace and trimmed. A word starting with `'` or `"`
/// runs to the next matching quote, and the quotes are removed. Quotes inside an unquoted word are
/// literal.
struct Words<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let text = self.rest.trim_start();
        let first = text.chars().next()?;
        let (word, rest) = if first == '\'' || first == '"' {
            let inner = text.get(first.len_utf8()..).unwrap_or_default();
            match inner.find(first) {
                Some(end) => (inner.get(..end).unwrap_or_default(), inner.get(end + first.len_utf8()..).unwrap_or_default()),
                None => (inner, ""),
            }
        } else {
            let end = text.find(char::is_whitespace).unwrap_or(text.len());
            (text.get(..end).unwrap_or_default(), text.get(end..).unwrap_or_default())
        };
        self.rest = rest;
        Some(word.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word whose key isn't a variable's name is left out, and counted as none.
    #[test]
    fn only_variables_names_are_taken() {
        let mut vars = Vec::new();
        parse_env("MY-VAR=1 OK=2 1X=3 _a1=4 BASH_FUNC_x%%=5 É=6", &mut vars);
        assert_eq!(vars, [("OK".to_owned(), "2".to_owned()), ("_a1".to_owned(), "4".to_owned())]);
        assert_eq!(count_env("MY-VAR=1 OK=2"), 1);
        assert!(env_name("SDL_GAMECONTROLLER_IGNORE_DEVICES") && !env_name("") && !env_name("A B"));
    }

    #[test]
    fn counting_variables_agrees_with_parsing_them() {
        for text in ["", "A=1", "A=1 B=2", "  A=1   'B=x y' C", "=1 D=", "\"E=a b\" F=c", "'unclosed=x y", "x 'y' z=1=2", "' =x'", "'  A =x'"] {
            let mut vars = Vec::new();
            parse_env(text, &mut vars);
            assert_eq!(count_env(text), vars.len(), "{text:?}");
        }
    }
}
