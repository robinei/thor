//! library.json read, and written one place, runner or source a line.

use super::*;

/// The config at `file`. It is the default if the file is missing or doesn't parse; in the second case,
/// what is wrong with it is returned too.
pub(crate) fn read_config(file: &Path) -> (Config, Option<String>) {
    let Ok(text) = fs::read_to_string(file).inspect_err(|e| {
        crate::os::log_unread("library", file, e);
    }) else {
        return (Config::default(), None);
    };
    match serde_json::from_str(&text) {
        Ok(config) => (config, None),
        Err(e) => {
            log::warn!("library: {}: {e}", file.display());
            (Config::default(), Some(format!("library.json: {e}")))
        }
    }
}

/// Writes `config` to `file`, one place, runner or source a line as the starter is written, whole (see
/// `os::fs::write_file`): a crash mid-write leaves the last library.json.
pub(crate) fn write_config(file: &Path, config: &Config) -> std::io::Result<()> {
    crate::os::fs::write_file(file, config_text(config)?.as_bytes())
}

/// `config` as library.json's text: one place, runner or source a line.
pub(super) fn config_text(config: &Config) -> std::io::Result<String> {
    let line = |v: &dyn erased::Line| v.line();
    let list = |name: &str, items: Vec<String>| {
        let lines: Vec<String> = items.iter().map(|i| format!("    {i}")).collect();
        format!("  \"{name}\": [\n{}\n  ]", lines.join(",\n"))
    };
    let places = config.places.iter().map(|p| line(p)).collect::<Result<Vec<_>, _>>()?;
    let runners = config.runners.iter().map(|r| line(r)).collect::<Result<Vec<_>, _>>()?;
    let sources = config.sources.iter().map(|s| line(s)).collect::<Result<Vec<_>, _>>()?;
    let keys = line(&config.keys)?;
    Ok(format!("{{\n{},\n{},\n{},\n  \"keys\": {keys}\n}}\n", list("places", places), list("runners", runners), list("sources", sources)))
}

mod erased {
    /// A value as one line of JSON, with spaces inside its braces.
    pub(super) trait Line {
        fn line(&self) -> std::io::Result<String>;
    }

    impl<T: serde::Serialize> Line for T {
        fn line(&self) -> std::io::Result<String> {
            let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
            Ok(spaced(&text))
        }
    }

    /// `{"a":1,"b":[2,3]}` as `{ "a": 1, "b": [2, 3] }`, leaving strings as they are.
    fn spaced(json: &str) -> String {
        let mut out = String::with_capacity(json.len() * 5 / 4);
        let (mut in_string, mut escaped) = (false, false);
        let mut chars = json.chars().peekable();
        while let Some(c) = chars.next() {
            if in_string {
                out.push(c);
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => {
                    in_string = true;
                    out.push(c);
                }
                '{' if chars.peek() == Some(&'}') => out.push('{'),
                '{' => out.push_str("{ "),
                '}' if out.ends_with('{') => out.push('}'),
                '}' => out.push_str(" }"),
                ':' => out.push_str(": "),
                ',' => out.push_str(", "),
                c => out.push(c),
            }
        }
        out
    }
}
