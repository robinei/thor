//! The desktop session's services ClamShell provides or follows: the tray, the clipboard, the account,
//! and icons for windows found without a desktop file.

pub(crate) mod account;
pub mod clipboard;
pub mod environment;
pub(crate) mod icons;
pub mod priority;
pub mod tray;
