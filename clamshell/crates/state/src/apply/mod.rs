//! What needs more than one area, carried out in an `impl State` of its own file, by subject: what comes
//! in from the system threads ([`incoming`]), presses ([`press`]), the hardware keys ([`keys`]), the
//! lists' scrolling ([`scrolls`]), the second screen's tabs and the editor ([`tabs`]), the sliders
//! ([`sliders`]), launches ([`launching`]), the settings tab ([`settings_tab`]), the notifications and
//! the screens ([`notifying`]), the queries across areas ([`queries`]), and a system thread restarted
//! ([`restart`]).

pub(crate) mod incoming;
pub(crate) mod keys;
pub(crate) mod launching;
pub(crate) mod notifying;
pub(crate) mod press;
pub(crate) mod queries;
pub(crate) mod restart;
pub(crate) mod scrolls;
pub(crate) mod settings_tab;
pub(crate) mod sliders;
pub(crate) mod tabs;
