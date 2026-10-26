//! The desktop session's services ClamShell provides or follows: the tray, the clipboard, the account,
//! notifications, and icons for windows found without a desktop file.

pub(crate) mod account;
pub mod clipboard;
pub mod environment;
pub(crate) mod icons;
pub(crate) mod notifications;
pub mod priority;
pub mod tray;
