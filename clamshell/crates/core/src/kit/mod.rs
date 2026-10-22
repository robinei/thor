//! Generic parts that the features build on and that know nothing about them: the place of a scrolled
//! list, the `names!` macro, saving after a pause, secrets kept out of logs, and temporary directories for
//! tests.

pub(crate) mod debounce;
pub mod list;
pub(crate) mod names;
pub mod secret;
#[cfg(feature = "test-support")]
pub mod testdir;

/// Replaces `slot` with `value` and returns whether that changed it.
pub(crate) fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    let changed = *slot != value;
    *slot = value;
    changed
}
