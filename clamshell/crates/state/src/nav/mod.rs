//! Pointing at things. This covers the pointer's focus and press ([`mod@pointer`]), finding a pressed
//! list item again after the list changed ([`items`]), which lists there are and where each is scrolled
//! ([`lists`]), and where the pad's input goes ([`pad`]).

pub(crate) mod items;
pub mod lists;
pub(crate) mod pad;
pub(crate) mod pointer;
