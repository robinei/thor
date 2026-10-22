//! Environment variables as the editor's text holds them, as `KEY=value` words.

/// Parses `KEY=value` words (see [`Words`]) into variables, appended to `out`.
pub fn parse_env(text: &str, out: &mut Vec<(String, String)>) {
    out.extend(vars(text).map(|(k, v)| (k.to_owned(), v.to_owned())));
}

/// How many variables [`parse_env`] finds in `text`, without allocating, for drawing.
pub fn count_env(text: &str) -> usize {
    vars(text).count()
}

/// The `KEY=value` words of `text` with a key, split at their first `=`.
fn vars(text: &str) -> impl Iterator<Item = (&str, &str)> {
    Words { rest: text }.filter_map(|word| word.split_once('=')).filter(|(k, _)| !k.is_empty())
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

    #[test]
    fn counting_variables_agrees_with_parsing_them() {
        for text in ["", "A=1", "A=1 B=2", "  A=1   'B=x y' C", "=1 D=", "\"E=a b\" F=c", "'unclosed=x y", "x 'y' z=1=2", "' =x'", "'  A =x'"] {
            let mut vars = Vec::new();
            parse_env(text, &mut vars);
            assert_eq!(count_env(text), vars.len(), "{text:?}");
        }
    }
}
