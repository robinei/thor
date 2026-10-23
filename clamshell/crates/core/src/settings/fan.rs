//! The Fan setting's values: the fan daemon's modes ([`Mode`], from `clamshell-fan`'s library), which
//! `clamshell-system` writes to [`MODE_FILE`] for the daemon.

pub use clamshell_fan_mode::{MODE_FILE, Mode};

crate::settings::choices!(impl Mode for Fan, default Auto {
    Auto = 0, "Fan auto", Stock;
    Quiet = 1, "Fan quiet", Changes;
    Max = 2, "Fan max", Costs;
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_by_setting() {
        use crate::settings::Choice;
        assert_eq!(Mode::of(0), Mode::Auto);
        assert_eq!(Mode::of(1), Mode::Quiet);
        assert_eq!(Mode::of(2), Mode::Max);
    }
}
