//! Paths in library.json: `~/` and `$VARIABLE`s expanded.

use super::*;

/// `~` and `~/...` against `home`.
pub(crate) fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        (None, Some(home)) if path == "~" => home.to_path_buf(),
        _ => PathBuf::from(path),
    }
}

/// Expands a dir as library.json gives it: environment variables first (see [`expand_vars`]), then `~/`.
/// Errs with the name of a variable that has no value and no default.
pub(crate) fn expand_dir(dir: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    expand_vars(dir, &|name| std::env::var(name).ok()).map(|d| expand(&d, home))
}

/// Expands `$NAME`, `${NAME}` and `${NAME:-default}` in `text` using `lookup`. The default is used when
/// the variable is unset or empty, and is expanded itself. A `$` followed by anything else stays as it is.
/// Errs with the name of a variable that has no value and no default.
pub(super) fn expand_vars(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let name_len =
        |s: &str| s.char_indices().find(|&(i, c)| !(c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))).map_or(s.len(), |(i, _)| i);
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(rest.get(..at).unwrap_or(""));
        let after = rest.get(at + 1..).unwrap_or("");
        if let Some(inner) = after.strip_prefix('{') {
            // Find the matching brace, since a default may contain `${...}` itself.
            let mut depth = 1;
            let end = inner.char_indices().find(|&(_, c)| {
                depth += match c {
                    '{' => 1,
                    '}' => -1,
                    _ => 0,
                };
                depth == 0
            });
            let Some((end, _)) = end else {
                out.push_str(rest.get(at..).unwrap_or(""));
                return Ok(out);
            };
            let body = inner.get(..end).unwrap_or("");
            let (name, default) = match body.split_once(":-") {
                Some((name, default)) => (name, Some(default)),
                None => (body, None),
            };
            match (lookup(name).filter(|v| !v.is_empty()), default) {
                (Some(value), _) => out.push_str(&value),
                (None, Some(default)) => out.push_str(&expand_vars(default, lookup)?),
                (None, None) => return Err(name.to_owned()),
            }
            rest = inner.get(end + 1..).unwrap_or("");
        } else {
            let len = name_len(after);
            if len == 0 {
                out.push('$');
            } else {
                let name = after.get(..len).unwrap_or("");
                out.push_str(&lookup(name).ok_or_else(|| name.to_owned())?);
            }
            rest = after.get(len..).unwrap_or("");
        }
    }
    out.push_str(rest);
    Ok(out)
}
