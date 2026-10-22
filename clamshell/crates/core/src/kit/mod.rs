//! Generic parts the features build on, knowing nothing of them: scrolled lists' places, the `names!`
//! macro, saving after a pause, secrets kept out of logs, temporary directories for tests.

pub(crate) mod debounce;
pub mod list;
pub(crate) mod names;
pub mod secret;
#[cfg(feature = "test-support")]
pub mod testdir;

/// Replaces `slot` with `value`; whether that changed it.
pub(crate) fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    let changed = *slot != value;
    *slot = value;
    changed
}
