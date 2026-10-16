//! [`names!`]: a fieldless enum's variants listed once, with each one's name (its word in files and the
//! control pipe) and, where it has one, its label (how it reads on screen).

/// Gives a fieldless enum, from one list of its variants (in the order of `ALL`), each with its name and
/// optionally its label:
///
/// - `ALL`: every variant, in the list's order;
/// - `name()`: its word in files and the control pipe;
/// - `from_name(word)`: the first variant with that name;
/// - `label()`: how it reads on screen, when the list gives labels.
///
/// ```ignore
/// names!(Dir { Up: "up", "Up"; Down: "down", "Down"; Left: "left", "Left"; Right: "right", "Right"; });
/// ```
macro_rules! names {
    ($ty:ident { $($v:ident: $name:literal, $label:literal;)* }) => {
        $crate::names::names!(@common $ty { $($v: $name;)* });
        impl $ty {
            /// How it reads on screen.
            pub fn label(self) -> &'static str {
                match self {
                    $(Self::$v => $label,)*
                }
            }
        }
    };
    ($ty:ident { $($v:ident: $name:literal;)* }) => {
        $crate::names::names!(@common $ty { $($v: $name;)* });
    };
    (@common $ty:ident { $($v:ident: $name:literal;)* }) => {
        impl $ty {
            /// Every one, in order.
            pub const ALL: [Self; $crate::names::names!(@count $($v)*)] = [$(Self::$v),*];

            /// Its word in files and the control pipe.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$v => $name,)*
                }
            }

            /// The first with this name.
            pub fn from_name(word: &str) -> Option<Self> {
                Self::ALL.into_iter().find(|x| x.name() == word)
            }
        }
    };
    (@count) => { 0usize };
    (@count $head:ident $($tail:ident)*) => { 1usize + $crate::names::names!(@count $($tail)*) };
}

pub(crate) use names;
