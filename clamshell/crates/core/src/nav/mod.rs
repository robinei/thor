//! Pointing at things: the pointer's focus and press ([`mod@pointer`]), presses on list items found again
//! after the list changed ([`items`]), which lists there are and their places ([`lists`]), and where
//! the pad's input goes ([`pad`]).

pub(crate) mod items;
pub mod lists;
pub(crate) mod pad;
pub(crate) mod pointer;
