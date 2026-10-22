//! Environment variables as the editor's text holds them: `KEY=value` words.

/// `KEY=value` words (see [`Words`]) as variables.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    vars(text).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

/// How many variables [`parse_env`] finds in `text`, without allocating (for drawing).
pub fn count_env(text: &str) -> usize {
    vars(text).count()
}

/// The `KEY=value` words of `text` with a key, split at their first `=`.
fn vars(text: &str) -> impl Iterator<Item = (&str, &str)> {
    Words { rest: text }.filter_map(|word| word.split_once('=')).filter(|(k, _)| !k.is_empty())
}

/// The words of a text: whitespace separates; a word starting with `'` or `"` runs to the next same quote
/// (quotes removed); quotes inside an unquoted word are literal; words are trimmed.
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
            assert_eq!(count_env(text), parse_env(text).len(), "{text:?}");
        }
    }
}
