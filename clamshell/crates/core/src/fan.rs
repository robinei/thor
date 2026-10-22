//! The fan's mode, as the Fan setting chooses it and the fan daemon (`clamshell-fan`, whose curves and
//! steps are its own) reads it: written to [`MODE_FILE`] (which the image makes writable by the session's
//! user); without one, the daemon runs [`Mode::Auto`].

/// Where the mode is written: its name, as [`Mode::name`] has it.
pub const MODE_FILE: &str = "/run/clamshell-fan";

crate::settings::choices!(
    /// The fan's curve: [`crate::settings::Setting::Fan`].
    Mode for Fan, default Auto {
        Auto = 0, "Fan auto", Stock;
        Quiet = 1, "Fan quiet", Changes;
        Max = 2, "Fan max", Costs;
    }
);

impl Mode {
    /// Its word in the mode file.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Quiet => "quiet",
            Mode::Max => "max",
        }
    }

    /// A mode file's contents; anything else: none.
    pub fn parse(text: &str) -> Option<Mode> {
        [Mode::Auto, Mode::Quiet, Mode::Max].into_iter().find(|m| m.name() == text.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_by_setting_and_by_name() {
        use crate::settings::Choice;
        assert_eq!(Mode::of(0), Mode::Auto);
        assert_eq!(Mode::of(1), Mode::Quiet);
        assert_eq!(Mode::of(2), Mode::Max);
        for mode in [Mode::Auto, Mode::Quiet, Mode::Max] {
            assert_eq!(Mode::parse(&format!("{}\n", mode.name())), Some(mode));
        }
        assert_eq!(Mode::parse(""), None);
    }
}
