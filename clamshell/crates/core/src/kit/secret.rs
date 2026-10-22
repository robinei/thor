//! [`Secret`]: a password, kept out of logs.

use std::fmt;

/// A password: kept out of logs (effects and events are logged as they go).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}
