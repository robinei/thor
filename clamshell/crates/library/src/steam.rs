//! Steam's own files, the parts that need no disk: Valve's KeyValues text (VDF), the library folders
//! listed in `libraryfolders.vdf`, and what a game's manifest (`appmanifest_<id>.acf`) says. Steam's
//! tools (Proton, the Linux runtimes, the redistributables) are left out. Finding the files and the art
//! is clamshell-policy's (`library::steam`).

use std::path::PathBuf;

/// A VDF value: a string, or a block of key-value pairs.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Block(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Block(items) => items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Value::Str(_) => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    pub fn blocks(&self) -> impl Iterator<Item = &Value> {
        let items: &[(String, Value)] = match self {
            Value::Block(items) => items,
            Value::Str(_) => &[],
        };
        items.iter().map(|(_, v)| v).filter(|v| matches!(v, Value::Block(_)))
    }
}

/// Parses Valve's KeyValues text ("key" "value", "key" { ... }, `//` comments); the whole file as one
/// block.
pub fn parse_vdf(text: &str) -> Value {
    parse_block(&mut tokenize(text).into_iter().peekable())
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => {}
                        },
                        other => s.push(other),
                    }
                }
                tokens.push(Token::Str(s));
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    tokens
}

fn parse_block(tokens: &mut std::iter::Peekable<std::vec::IntoIter<Token>>) -> Value {
    let mut items = Vec::new();
    while let Some(t) = tokens.next() {
        match t {
            Token::Str(key) => match tokens.next() {
                Some(Token::Str(v)) => items.push((key, Value::Str(v))),
                Some(Token::Open) => items.push((key, parse_block(tokens))),
                _ => break,
            },
            Token::Close => break,
            Token::Open => {}
        }
    }
    Value::Block(items)
}

/// The library folders `libraryfolders.vdf` lists, in its order.
pub fn library_folders(vdf: &Value) -> Vec<PathBuf> {
    vdf.get("libraryfolders").map(|f| f.blocks().filter_map(|b| b.str("path")).map(PathBuf::from).collect()).unwrap_or_default()
}

/// Steam's own tools, which aren't games.
fn is_tool(name: &str) -> bool {
    name.starts_with("Proton ") || name.starts_with("Steam Linux Runtime") || name.starts_with("Steamworks Common")
}

/// An installed game a manifest describes: its app id and name; `None` for a tool, a game not fully
/// installed, or a manifest that doesn't say.
pub fn manifest(vdf: &Value) -> Option<(u32, &str)> {
    let app = vdf.get("AppState")?;
    let appid: u32 = app.str("appid")?.parse().ok()?;
    let name = app.str("name")?;
    // StateFlags bit 4: fully installed.
    let installed = app.str("StateFlags").and_then(|f| f.parse::<u32>().ok()).is_some_and(|f| f & 4 != 0);
    (installed && !is_tool(name)).then_some((appid, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"InstalledDepots\"\n\t{\n\t\t\"1245621\"\n\t\t{\n\t\t\t\"size\"\t\t\"1\"\n\t\t}\n\t}\n}\n";

    #[test]
    fn reads_a_manifest() {
        assert_eq!(manifest(&parse_vdf(MANIFEST)), Some((1_245_620, "ELDEN RING")));
    }

    #[test]
    fn tools_and_uninstalled_apps_are_left_out() {
        let tool = MANIFEST.replace("ELDEN RING", "Proton Experimental");
        assert_eq!(manifest(&parse_vdf(&tool)), None);
        let partial = MANIFEST.replace("\"StateFlags\"\t\t\"4\"", "\"StateFlags\"\t\t\"1026\"");
        assert_eq!(manifest(&parse_vdf(&partial)), None);
    }

    #[test]
    fn library_folders_in_order() {
        let vdf = parse_vdf("\"libraryfolders\" { \"0\" { \"path\" \"/a\" } \"1\" { \"path\" \"/b\" } } // end");
        assert_eq!(library_folders(&vdf), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
