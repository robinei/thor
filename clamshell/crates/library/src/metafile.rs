//! The Pegasus metadata line syntax (docs/pegasus-format.md in the GPD repo, 2.1 and 2.2): `key: value`
//! lines, indented continuation lines, `.` for a paragraph break, `#` comments at column 0, blank lines
//! ending an entry.

/// One `key: value` entry: the key lowercased; each value line, `None` for a `.` paragraph break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub values: Vec<Option<String>>,
}

impl Entry {
    /// The first value line (single-value keys ignore the rest).
    pub fn first(&self) -> &str {
        self.values.first().and_then(|v| v.as_deref()).unwrap_or("")
    }

    /// Each value line as one item (list keys).
    pub fn items(&self) -> impl Iterator<Item = &str> {
        self.values.iter().filter_map(|v| v.as_deref())
    }

    /// Lines joined with spaces, `.` as a blank line between paragraphs (descriptions, launch commands).
    pub fn merged(&self) -> String {
        let mut out = String::new();
        let mut after_break = true;
        for v in &self.values {
            match v {
                Some(line) => {
                    if !after_break {
                        out.push(' ');
                    }
                    out.push_str(line);
                    after_break = false;
                }
                None => {
                    out.push_str("\n\n");
                    after_break = true;
                }
            }
        }
        out.trim().to_owned()
    }

    /// `merged`, with the escape `\n` turned into a newline (and `\\n` into a literal `\n`), as Pegasus
    /// does for descriptions and summaries.
    pub fn text(&self) -> String {
        let merged = self.merged();
        let mut out = String::with_capacity(merged.len());
        let mut chars = merged.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.peek() {
                    Some('n') => {
                        chars.next();
                        out.push('\n');
                    }
                    Some('\\') => {
                        chars.next();
                        if chars.peek() == Some(&'n') {
                            chars.next();
                            out.push_str("\\n");
                        } else {
                            out.push('\\');
                        }
                    }
                    _ => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}

/// The entries of a metadata file, in order.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    let close = |current: &mut Option<Entry>, entries: &mut Vec<Entry>| {
        if let Some(e) = current.take()
            && !e.values.is_empty()
        {
            entries.push(e);
        }
    };
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.trim().is_empty() {
            close(&mut current, &mut entries);
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            if let Some(e) = current.as_mut() {
                let v = line.trim();
                e.values.push(if v == "." { None } else { Some(v.to_owned()) });
            }
            continue;
        }
        close(&mut current, &mut entries);
        let trimmed = line.trim();
        match trimmed.find(':') {
            Some(pos) if pos > 0 => {
                let key = trimmed.get(..pos).unwrap_or("").trim().to_lowercase();
                let value = trimmed.get(pos + 1..).unwrap_or("").trim();
                let values = if value.is_empty() { Vec::new() } else { vec![Some(value.to_owned())] };
                current = Some(Entry { key, values });
            }
            _ => {}
        }
    }
    close(&mut current, &mut entries);
    entries
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn entries_continuations_and_comments() {
        let e = parse("# comment\ncollection: PC Games\nlaunch: \"{file.path}\"\n\ngame: Diablo: Hellfire\ndescription: A\n  B\n  .\n  C\n");
        assert_eq!(e.len(), 4);
        assert_eq!((e[0].key.as_str(), e[0].first()), ("collection", "PC Games"));
        assert_eq!(e[2].first(), "Diablo: Hellfire");
        assert_eq!(e[3].merged(), "A B\n\nC");
    }

    #[test]
    fn keys_are_lowercased_and_empty_keys_dropped() {
        let e = parse("Assets.BoxFront: a.png\nempty:\nfile: x\n");
        assert_eq!(e.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["assets.boxfront", "file"]);
    }

    #[test]
    fn a_hash_after_indentation_is_text() {
        let e = parse("description: a\n  # not a comment\n");
        assert_eq!(e[0].merged(), "a # not a comment");
    }

    #[test]
    fn newline_escapes_in_text() {
        let e = parse("description: one\\ntwo \\\\n three\n");
        assert_eq!(e[0].text(), "one\ntwo \\n three");
    }
}
