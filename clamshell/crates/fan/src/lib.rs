//! The fan's mode, as ClamShell writes it and the daemon (`clamshell-fan`) reads it. ClamShell's Fan
//! setting chooses the mode (`clamshell_state::settings::fan`) and writes it to [`MODE_FILE`], which the
//! image makes writable by the session's user. Without that file, the daemon runs [`Mode::Auto`]. The
//! daemon has its own curves and steps for each mode.

/// Where the mode is written, as its name from [`Mode::name`].
pub const MODE_FILE: &str = "/run/clamshell-fan";

/// The fan's curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    Auto,
    Quiet,
    Max,
}

impl Mode {
    /// The mode's word in the mode file.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Quiet => "quiet",
            Mode::Max => "max",
        }
    }

    /// Parses a mode file's contents. Anything else gives `None`.
    pub fn parse(text: &str) -> Option<Mode> {
        [Mode::Auto, Mode::Quiet, Mode::Max].into_iter().find(|m| m.name() == text.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_by_name() {
        for mode in [Mode::Auto, Mode::Quiet, Mode::Max] {
            assert_eq!(Mode::parse(&format!("{}\n", mode.name())), Some(mode));
        }
        assert_eq!(Mode::parse(""), None);
    }
}
