//! The two screens, and something per screen.

use std::ops::{Index, IndexMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    /// The main screen: shelf.
    Top,
    /// The second screen: the bar and its tabs.
    Bottom,
}

impl Screen {
    pub const BOTH: [Screen; 2] = [Screen::Top, Screen::Bottom];

    pub fn other(self) -> Screen {
        match self {
            Screen::Top => Screen::Bottom,
            Screen::Bottom => Screen::Top,
        }
    }
}

/// A `T` for each screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Screens<T> {
    pub top: T,
    pub bottom: T,
}

impl<T> Screens<T> {
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Screen, &mut T)> {
        [(Screen::Top, &mut self.top), (Screen::Bottom, &mut self.bottom)].into_iter()
    }
}

impl<T> Index<Screen> for Screens<T> {
    type Output = T;

    fn index(&self, screen: Screen) -> &T {
        match screen {
            Screen::Top => &self.top,
            Screen::Bottom => &self.bottom,
        }
    }
}

impl<T> IndexMut<Screen> for Screens<T> {
    fn index_mut(&mut self, screen: Screen) -> &mut T {
        match screen {
            Screen::Top => &mut self.top,
            Screen::Bottom => &mut self.bottom,
        }
    }
}
