//! Turning a Pegasus launch command into a program, arguments and working directory, as Pegasus does
//! (docs/pegasus-format.md in the GPD repo, 4.1): its own tokenizer (no shell), `{file.*}` placeholders
//! filled in per token, a command containing a slash taken relative to the game's metadata dir, and the
//! working dir from `workdir`, else the command's dir, else the game file's dir.

use std::path::{Path, PathBuf};

use crate::pegasus::clean;

/// A Pegasus launch command as the metadata gives it, still raw (tokenised and filled in at launch): the
/// command, its working dir, and the dir relative paths in them are against (the metadata file's).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchSpec {
    pub command: String,
    pub workdir: String,
    pub basedir: PathBuf,
}

impl LaunchSpec {
    /// Fills in what this one leaves empty from `from` (a game takes its collection's).
    pub fn inherit(&mut self, from: &LaunchSpec) {
        if self.command.is_empty() {
            self.command.clone_from(&from.command);
        }
        if self.workdir.is_empty() {
            self.workdir.clone_from(&from.workdir);
        }
        if self.basedir.as_os_str().is_empty() {
            self.basedir.clone_from(&from.basedir);
        }
    }

    /// The program, arguments and working dir for launching `file`; `None` if the command is empty.
    pub fn resolve(&self, file: &Path) -> Option<Command> {
        resolve(&self.command, &self.workdir, &self.basedir, file)
    }
}

/// A launch command resolved for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub workdir: PathBuf,
}

/// Pegasus' tokenizer: whitespace separates; a token starting with `'` or `"` runs to the next same
/// quote (quotes removed); quotes inside an unquoted token are literal; tokens are trimmed.
pub fn tokenize(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let Some(&c) = chars.get(i) else {
            break;
        };
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c == '\'' || c == '"' {
            i += 1;
            while chars.get(i).is_some_and(|&d| d != c) {
                i += 1;
            }
            // Past the closing quote, if there is one.
            let end = (i + 1).min(chars.len());
            let token: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
            let closed = token.len() > 1 && token.ends_with(c);
            let inner = if closed { token.get(1..token.len() - 1).unwrap_or("") } else { token.get(1..).unwrap_or("") };
            tokens.push(inner.trim().to_owned());
            i = end;
        } else {
            while chars.get(i).is_some_and(|d| !d.is_whitespace()) {
                i += 1;
            }
            let token: String = chars.get(start..i).unwrap_or(&[]).iter().collect();
            tokens.push(token.trim().to_owned());
        }
    }
    tokens
}

/// Fills in the `{file.*}` placeholders for `file`.
fn substitute(token: &str, file: &Path) -> String {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let base = match name.rfind('.') {
        Some(i) if i > 0 => name.get(..i).unwrap_or(name),
        _ => name,
    };
    let dir = file.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    token.replace("{file.path}", &file.to_string_lossy()).replace("{file.name}", name).replace("{file.basename}", base).replace("{file.dir}", &dir)
}

/// The program, arguments and working dir for launching `file` with `command` (from a game whose
/// metadata is in `basedir`); `None` if the command is empty.
fn resolve(command: &str, workdir: &str, basedir: &Path, file: &Path) -> Option<Command> {
    let mut tokens = tokenize(command).into_iter().map(|t| substitute(&t, file));
    let first = tokens.next().filter(|t| !t.is_empty())?;
    let args: Vec<String> = tokens.collect();
    let has_slash = first.contains('/') || first.contains('\\');
    let program = if has_slash { clean(&basedir.join(&first)) } else { PathBuf::from(&first) };
    let workdir = if !workdir.is_empty() {
        clean(&basedir.join(substitute(workdir, file)))
    } else if has_slash {
        program.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        file.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    Some(Command { program, args, workdir })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_like_pegasus() {
        assert_eq!(tokenize(r#"retroarch -f -L core.so "{file.path}""#), vec!["retroarch", "-f", "-L", "core.so", "{file.path}"]);
        assert_eq!(tokenize(r#"--opt="a b""#), vec![r#"--opt="a"#, r#"b""#]);
        assert_eq!(tokenize(r#""a b"c"#), vec!["a b", "c"]);
        assert_eq!(tokenize("''"), vec![""]);
        assert_eq!(tokenize(r#"" x ""#), vec!["x"]);
    }

    #[test]
    fn a_script_runs_in_its_own_dir() {
        let c = resolve(r#""{file.path}""#, "", Path::new("/g/pc"), Path::new("/g/pc/diablo.sh")).unwrap();
        assert_eq!(c, Command { program: "/g/pc/diablo.sh".into(), args: vec![], workdir: "/g/pc".into() });
    }

    #[test]
    fn a_program_from_path_runs_in_the_game_files_dir() {
        let file = Path::new("/g/snes/Chrono Trigger (U) [!].smc");
        let c = resolve(r#"retroarch -f -L /usr/lib/libretro/snes9x_libretro.so "{file.path}""#, "", Path::new("/g/snes"), file).unwrap();
        assert_eq!(c.program, PathBuf::from("retroarch"));
        assert_eq!(c.args.last().map(String::as_str), Some("/g/snes/Chrono Trigger (U) [!].smc"));
        assert_eq!(c.workdir, PathBuf::from("/g/snes"));
    }

    #[test]
    fn relative_commands_and_workdirs_are_against_the_metadata_dir() {
        let c = resolve("bin/run {file.basename}", "work", Path::new("/g/x"), Path::new("/g/x/roms/a.b.iso")).unwrap();
        assert_eq!(c.program, PathBuf::from("/g/x/bin/run"));
        assert_eq!(c.args, vec!["a.b"]);
        assert_eq!(c.workdir, PathBuf::from("/g/x/work"));
        assert!(resolve("  ", "", Path::new("/"), Path::new("/f")).is_none());
    }

    #[test]
    fn a_game_inherits_what_it_leaves_empty() {
        let mut game = LaunchSpec { command: String::new(), workdir: "w".into(), basedir: PathBuf::new() };
        game.inherit(&LaunchSpec { command: "run".into(), workdir: "cw".into(), basedir: "/c".into() });
        assert_eq!(game, LaunchSpec { command: "run".into(), workdir: "w".into(), basedir: "/c".into() });
    }
}
